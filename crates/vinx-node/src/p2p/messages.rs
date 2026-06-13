use serde::{Deserialize, Serialize};
use vinx_core::{Block, BlockSignature, Transaction};

/// Messages exchanged over the GossipSub P2P network.
#[derive(Clone, Debug, Serialize, Deserialize)]
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
        bincode::serialize(self).unwrap_or_default()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        bincode::deserialize(bytes).ok()
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
