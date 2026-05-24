pub mod chain;
pub mod config;
pub mod mempool;
pub mod node;
pub mod producer;
pub mod rpc;

pub use config::NodeConfig;
pub use node::Node;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum NodeError {
    #[error("Block production error: {0}")]
    BlockProduction(String),
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
