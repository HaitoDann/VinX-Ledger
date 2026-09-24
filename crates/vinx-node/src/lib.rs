pub mod bft;
pub mod chain;
pub mod checkpoints;
pub mod config;
pub mod execution;
pub mod mempool;
pub mod node;
pub mod p2p;
pub mod rpc;
pub mod storage;
pub mod sync;

pub use config::NodeConfig;
pub use node::Node;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("Block production error: {0}")]
    BlockProduction(String),
    #[error("Consensus error: {0}")]
    Consensus(String),
    #[error("P2P error: {0}")]
    P2p(String),
    #[error("RPC error: {0}")]
    Rpc(String),
    #[error("Config error: {0}")]
    Config(String),
}

impl From<vinx_core::CoreError> for NodeError {
    fn from(e: vinx_core::CoreError) -> Self {
        NodeError::BlockProduction(e.to_string())
    }
}
