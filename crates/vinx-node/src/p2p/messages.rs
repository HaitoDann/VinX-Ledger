use borsh::{BorshDeserialize, BorshSerialize};
use serde::{Deserialize, Serialize};
use vinx_core::{Block, BlockSignature, Transaction};
use zstd;

/// Messages plus courts que ce seuil sont envoyés bruts (overhead de compression > gain).
const COMPRESSION_THRESHOLD: usize = 512;
const FLAG_RAW: u8 = 0x00;
const FLAG_ZSTD: u8 = 0x01;

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
            FLAG_RAW => payload.to_vec(),
            FLAG_ZSTD => zstd::decode_all(payload).ok()?,
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
}
