use crate::bft::Proposal;
use borsh::{BorshDeserialize, BorshSerialize};
use vinx_core::{Block, CommitCert, SignedVote, Transaction};
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

/// Messages exchanged over the GossipSub P2P network (ADR 0082).
/// Wire format: Borsh (deterministic, compact, no schema needed), zstd above a threshold.
#[derive(Clone, Debug, BorshSerialize, BorshDeserialize)]
pub enum P2pMessage {
    /// A new transaction submitted by a user.
    NewTransaction(Transaction),
    /// A signed block proposal for `(height, round)`.
    Proposal(Proposal),
    /// A signed prevote or precommit.
    Vote(SignedVote),
    /// A block with the certificate that committed it — how a node that missed the
    /// proposal (or joined late) catches up on the latest height.
    Committed { block: Block, cert: CommitCert },
    /// Request committed blocks starting from `from_height` (a node that is behind).
    SyncRequest { from_height: u64, limit: u32 },
    /// Committed blocks with their certificates, in ascending height order.
    SyncResponse { rows: Vec<(Block, CommitCert)> },
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
            P2pMessage::NewTransaction(_) => "vinx/txs/1",
            P2pMessage::Proposal(_) | P2pMessage::Vote(_) => "vinx/consensus/1",
            P2pMessage::Committed { .. } => "vinx/blocks/1",
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
    use vinx_core::{BlockHeader, VoteKind};
    use vinx_crypto::{Address, KeyPair};

    fn dummy_addr() -> Address {
        Address::from_public_key(&KeyPair::generate().public_key())
    }

    fn dummy_block() -> Block {
        Block {
            header: BlockHeader {
                height: 1,
                round: 0,
                prev_hash: [0u8; 32],
                timestamp: 1_000,
                validator: dummy_addr(),
                tx_count: 0,
                state_root: [0u8; 32],
                base_fee: 0,
                receipts_root: [0u8; 32],
                last_commit_hash: [0u8; 32],
            },
            transactions: vec![],
            last_commit: None,
        }
    }

    fn dummy_cert() -> CommitCert {
        CommitCert {
            height: 1,
            round: 0,
            block_hash: [1u8; 32],
            bitmap: vec![0b111],
            aggregate: vec![0u8; 96],
        }
    }

    #[test]
    fn test_consensus_messages_roundtrip() {
        let vote = SignedVote {
            kind: VoteKind::Precommit,
            height: 9,
            round: 2,
            value: Some([3u8; 32]),
            validator: dummy_addr(),
            signature: vec![7u8; 96],
        };
        match P2pMessage::decode(&P2pMessage::Vote(vote.clone()).encode()).unwrap() {
            P2pMessage::Vote(v) => assert_eq!(v, vote),
            _ => panic!("expected Vote"),
        }
        let prop = Proposal {
            height: 1,
            round: 0,
            pol_round: Some(0),
            block: dummy_block(),
            signature: vec![1u8; 96],
        };
        match P2pMessage::decode(&P2pMessage::Proposal(prop.clone()).encode()).unwrap() {
            P2pMessage::Proposal(p) => assert_eq!(p, prop),
            _ => panic!("expected Proposal"),
        }
        let msg = P2pMessage::Committed {
            block: dummy_block(),
            cert: dummy_cert(),
        };
        assert!(matches!(
            P2pMessage::decode(&msg.encode()).unwrap(),
            P2pMessage::Committed { .. }
        ));
    }

    #[test]
    fn test_sync_roundtrip_and_compression() {
        let rows: Vec<(Block, CommitCert)> = (0..20)
            .map(|i| {
                let mut b = dummy_block();
                b.header.height = i;
                (b, dummy_cert())
            })
            .collect();
        let msg = P2pMessage::SyncResponse { rows };
        let encoded = msg.encode();
        assert_eq!(encoded[0], FLAG_ZSTD, "large message should be compressed");
        match P2pMessage::decode(&encoded).unwrap() {
            P2pMessage::SyncResponse { rows } => assert_eq!(rows.len(), 20),
            _ => panic!("expected SyncResponse"),
        }
        let req = P2pMessage::SyncRequest {
            from_height: 42,
            limit: 100,
        };
        let encoded = req.encode();
        assert_eq!(encoded[0], FLAG_RAW, "small message should stay raw");
        assert!(matches!(
            P2pMessage::decode(&encoded).unwrap(),
            P2pMessage::SyncRequest {
                from_height: 42,
                limit: 100
            }
        ));
    }

    #[test]
    fn test_decode_garbage_returns_none() {
        assert!(P2pMessage::decode(b"not valid borsh").is_none());
    }

    #[test]
    fn test_topic_names() {
        assert_eq!(
            P2pMessage::NewTransaction(Transaction::new_transfer(
                &KeyPair::generate(),
                dummy_addr(),
                vinx_core::Amount::from_vinx(1),
                vinx_core::Amount::from_vinx(1),
                0
            ))
            .topic(),
            "vinx/txs/1"
        );
        assert_eq!(
            P2pMessage::Committed {
                block: dummy_block(),
                cert: dummy_cert()
            }
            .topic(),
            "vinx/blocks/1"
        );
        assert_eq!(
            P2pMessage::SyncRequest {
                from_height: 0,
                limit: 1
            }
            .topic(),
            "vinx/sync/1"
        );
    }

    // ─── ADR 0022: anti-DoS decode & sync bounds ─────────────────────────────

    #[test]
    fn test_decompression_bomb_is_rejected() {
        let bomb_raw = vec![0u8; MAX_DECODED_BYTES + 1_000_000];
        let compressed = zstd::encode_all(&bomb_raw[..], 19).unwrap();
        assert!(compressed.len() < 100_000);
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
        framed.extend(std::iter::repeat_n(0u8, MAX_DECODED_BYTES + 1));
        assert!(P2pMessage::decode(&framed).is_none());
    }

    #[test]
    fn test_sync_batch_len_respects_budget_and_count() {
        assert_eq!(sync_batch_len(&[4, 4, 4], 10, 100), 2);
        assert_eq!(sync_batch_len(&[1, 1, 1, 1, 1], 1_000, 3), 3);
        assert_eq!(sync_batch_len(&[100, 1], 10, 100), 1);
        assert_eq!(sync_batch_len(&[], 10, 10), 0);
    }
}
