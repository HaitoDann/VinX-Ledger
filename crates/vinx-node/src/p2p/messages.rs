use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_core::{Block, BlockSignature, Transaction};
use zstd;

/// Messages plus courts que ce seuil sont envoyés bruts (overhead de compression > gain).
const COMPRESSION_THRESHOLD: usize = 512;
const FLAG_RAW: u8 = 0x00;
const FLAG_ZSTD: u8 = 0x01;

/// Hard ceiling on a **decoded** P2P message (ADR 0022). Bounds the memory a single
/// message may allocate, no matter how small the compressed frame is: the zstd stream is
/// stopped once it produces this many bytes, defeating decompression ("zip") bombs where a
/// few kilobytes on the wire would otherwise expand to gigabytes. Sized to hold one full
/// block plus a sync batch (see `SYNC_RESPONSE_BUDGET_BYTES`), with headroom.
pub const MAX_DECODED_BYTES: usize = 16 * 1024 * 1024; // 16 MiB

/// Maximum blocks a single `SyncResponse` may carry (ADR 0022). A hard count cap that
/// complements the byte budget below.
pub const MAX_SYNC_RESPONSE_BLOCKS: u32 = 512;

/// Cumulative serialized-byte budget for a `SyncResponse`. The server stops adding blocks
/// once the next one would push the batch past this — while always sending at least one
/// block, so sync always makes progress even if a single block is unusually large.
pub const SYNC_RESPONSE_BUDGET_BYTES: usize = 8 * 1024 * 1024; // 8 MiB

/// Given the serialized byte size of each candidate block, in ascending height order,
/// returns how many the server should include so the batch stays within `budget` bytes and
/// `max_count` blocks — always taking at least one block when any are available, to
/// guarantee forward progress (ADR 0022). Pure so the batching policy is unit-testable.
pub fn sync_batch_len(sizes: &[usize], budget: usize, max_count: usize) -> usize {
    let mut total = 0usize;
    let mut n = 0usize;
    for &s in sizes.iter().take(max_count) {
        if n > 0 && total.saturating_add(s) > budget {
            break;
        }
        total = total.saturating_add(s);
        n += 1;
    }
    n
}

/// Messages exchanged over the GossipSub P2P network.
/// Wire format: Borsh (deterministic, compact, no schema needed).
#[derive(Clone, Debug, Serialize, Deserialize, BorshSerialize, BorshDeserialize)]
pub enum P2pMessage {
    /// A new block produced by the leader of that height.
    NewBlock(Block),
    /// A new transaction submitted by a user.
    NewTransaction(Transaction),
    /// A co-signature for an already-announced block.
    BlockCoSignature {
        height: u64,
        signature: BlockSignature,
    },
    /// Request blocks starting from `from_height` (sent when a node detects it's behind).
    SyncRequest { from_height: u64, limit: u32 },
    /// Response to SyncRequest with the requested block range.
    SyncResponse { blocks: Vec<Block> },
}

impl P2pMessage {
    pub fn encode(&self) -> Vec<u8> {
        let raw = borsh::to_vec(self).unwrap_or_default();
        if raw.len() >= COMPRESSION_THRESHOLD {
            if let Ok(compressed) = zstd::encode_all(&raw[..], 1) {
                let mut out = Vec::with_capacity(1 + compressed.len());
                out.push(FLAG_ZSTD);
                out.extend_from_slice(&compressed);
                return out;
            }
        }
        let mut out = Vec::with_capacity(1 + raw.len());
        out.push(FLAG_RAW);
        out.extend_from_slice(&raw);
        out
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let (&flag, payload) = bytes.split_first()?;
        let raw = match flag {
            FLAG_RAW => {
                // A raw frame is its own decoded form — reject oversize outright.
                if payload.len() > MAX_DECODED_BYTES {
                    return None;
                }
                payload.to_vec()
            }
            // ADR 0022: bound the decompressor so a malicious frame can never allocate more
            // than MAX_DECODED_BYTES, regardless of its (tiny) compressed size.
            FLAG_ZSTD => decompress_bounded(payload, MAX_DECODED_BYTES)?,
            _ => return None,
        };
        borsh::from_slice(&raw).ok()
    }

    /// GossipSub topic name for this message type.
    pub fn topic(&self) -> &'static str {
        match self {
            P2pMessage::NewBlock(_) => "vinx/blocks/1",
            P2pMessage::NewTransaction(_) => "vinx/txs/1",
            P2pMessage::BlockCoSignature { .. } => "vinx/sigs/1",
            P2pMessage::SyncRequest { .. } | P2pMessage::SyncResponse { .. } => "vinx/sync/1",
        }
    }
}

/// Decompresses a zstd frame, refusing to produce more than `max` bytes (ADR 0022).
/// Reads at most `max + 1` decompressed bytes: reaching that means the frame expands beyond
/// the ceiling, so it is rejected. Memory is bounded to `max + 1` regardless of the frame.
fn decompress_bounded(payload: &[u8], max: usize) -> Option<Vec<u8>> {
    use std::io::Read;
    let decoder = zstd::stream::read::Decoder::new(payload).ok()?;
    let mut limited = decoder.take(max as u64 + 1);
    let mut out = Vec::new();
    limited.read_to_end(&mut out).ok()?;
    if out.len() > max {
        return None;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use vinx_core::{Block, BlockHeader, BlockSignature};
    use vinx_crypto::{Address, KeyPair};

    fn dummy_addr() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn dummy_block() -> Block {
        Block {
            header: BlockHeader {
                height: 1,
                prev_hash: [0u8; 32],
                timestamp: 1_000,
                validator: dummy_addr(),
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
            },
            transactions: vec![],
            signatures: vec![],
        }
    }

    #[test]
    fn test_block_message_roundtrip() {
        let msg = P2pMessage::NewBlock(dummy_block());
        let decoded = P2pMessage::decode(&msg.encode()).unwrap();
        assert!(matches!(decoded, P2pMessage::NewBlock(_)));
    }

    #[test]
    fn test_signature_message_roundtrip() {
        let kp = KeyPair::generate();
        let block = dummy_block();
        let hash = block.hash();
        let msg = P2pMessage::BlockCoSignature {
            height: 1,
            signature: BlockSignature {
                validator: dummy_addr(),
                pub_key: kp.public_key(),
                signature: kp.sign(&hash),
            },
        };
        let decoded = P2pMessage::decode(&msg.encode()).unwrap();
        assert!(matches!(
            decoded,
            P2pMessage::BlockCoSignature { height: 1, .. }
        ));
    }

    #[test]
    fn test_sync_request_roundtrip() {
        let msg = P2pMessage::SyncRequest {
            from_height: 42,
            limit: 100,
        };
        let decoded = P2pMessage::decode(&msg.encode()).unwrap();
        assert!(matches!(
            decoded,
            P2pMessage::SyncRequest {
                from_height: 42,
                limit: 100
            }
        ));
    }

    #[test]
    fn test_sync_response_roundtrip() {
        let msg = P2pMessage::SyncResponse {
            blocks: vec![dummy_block()],
        };
        let decoded = P2pMessage::decode(&msg.encode()).unwrap();
        if let P2pMessage::SyncResponse { blocks } = decoded {
            assert_eq!(blocks.len(), 1);
        } else {
            panic!("expected SyncResponse");
        }
    }

    #[test]
    fn test_decode_garbage_returns_none() {
        assert!(P2pMessage::decode(b"not valid bincode").is_none());
    }

    #[test]
    fn test_compression_roundtrip_large_message() {
        // SyncResponse with many blocks triggers compression
        let blocks: Vec<Block> = (0..20)
            .map(|i| {
                let mut b = dummy_block();
                b.header.height = i;
                b
            })
            .collect();
        let msg = P2pMessage::SyncResponse { blocks };
        let encoded = msg.encode();
        assert_eq!(encoded[0], FLAG_ZSTD, "large message should be compressed");
        let decoded = P2pMessage::decode(&encoded).unwrap();
        assert!(matches!(decoded, P2pMessage::SyncResponse { .. }));
    }

    #[test]
    fn test_small_message_stays_raw() {
        let msg = P2pMessage::SyncRequest {
            from_height: 1,
            limit: 10,
        };
        let encoded = msg.encode();
        assert_eq!(encoded[0], FLAG_RAW, "small message should stay raw");
        let decoded = P2pMessage::decode(&encoded).unwrap();
        assert!(matches!(
            decoded,
            P2pMessage::SyncRequest {
                from_height: 1,
                limit: 10
            }
        ));
    }

    #[test]
    fn test_topic_names() {
        assert_eq!(P2pMessage::NewBlock(dummy_block()).topic(), "vinx/blocks/1");
        assert_eq!(
            P2pMessage::SyncRequest {
                from_height: 0,
                limit: 1
            }
            .topic(),
            "vinx/sync/1"
        );
        assert_eq!(
            P2pMessage::SyncResponse { blocks: vec![] }.topic(),
            "vinx/sync/1"
        );
    }

    // ─── ADR 0022: anti-DoS decode & sync bounds ─────────────────────────────

    #[test]
    fn test_decompression_bomb_is_rejected() {
        // A tiny zstd frame that expands far beyond MAX_DECODED_BYTES must be refused
        // without allocating the full expansion.
        let bomb_raw = vec![0u8; MAX_DECODED_BYTES + 1_000_000]; // highly compressible zeros
        let compressed = zstd::encode_all(&bomb_raw[..], 19).unwrap();
        assert!(
            compressed.len() < 100_000,
            "the bomb must be small on the wire (got {} bytes)",
            compressed.len()
        );
        let mut framed = vec![FLAG_ZSTD];
        framed.extend_from_slice(&compressed);
        assert!(
            P2pMessage::decode(&framed).is_none(),
            "an over-ceiling zstd frame must be rejected"
        );
    }

    #[test]
    fn test_oversized_raw_frame_is_rejected() {
        let mut framed = vec![FLAG_RAW];
        framed.extend(std::iter::repeat(0u8).take(MAX_DECODED_BYTES + 1));
        assert!(P2pMessage::decode(&framed).is_none());
    }

    #[test]
    fn test_valid_message_below_ceiling_still_decodes() {
        // Guard against a ceiling so tight it breaks honest traffic.
        let msg = P2pMessage::SyncResponse {
            blocks: (0..8).map(|_| dummy_block()).collect(),
        };
        assert!(P2pMessage::decode(&msg.encode()).is_some());
    }

    #[test]
    fn test_sync_batch_len_respects_budget_and_count() {
        // Budget stops the batch: 3 blocks of 4 bytes fit in a 10-byte budget → 2 taken
        // (4 + 4 = 8, adding the third would exceed 10).
        assert_eq!(sync_batch_len(&[4, 4, 4], 10, 100), 2);
        // Count cap dominates when the budget is generous.
        assert_eq!(sync_batch_len(&[1, 1, 1, 1, 1], 1_000, 3), 3);
        // Always take at least one, even if the first block alone blows the budget.
        assert_eq!(sync_batch_len(&[100, 1], 10, 100), 1);
        // Empty input yields an empty batch.
        assert_eq!(sync_batch_len(&[], 10, 10), 0);
    }
}
