pub mod handlers;
pub mod types;
pub mod ui;

use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::Node;

pub fn router(node: Arc<Node>) -> Router {
    Router::new()
        .route("/", get(ui::index))
        .route("/health", get(handlers::health))
        .route("/chain/height", get(handlers::get_height))
        .route("/chain/sync", get(handlers::get_chain_sync))
        .route("/account/:address", get(handlers::get_account))
        .route("/account/:address/txs", get(handlers::get_account_txs))
        .route("/tx/submit", post(handlers::submit_tx))
        .route("/tx/:hash", get(handlers::get_tx_by_hash))
        .route("/block/:height", get(handlers::get_block))
        .route("/mempool/size", get(handlers::get_mempool))
        .route("/validators", get(handlers::get_validators))
        .route("/protocol/version", get(handlers::get_protocol_status))
        .route("/metrics", get(handlers::get_metrics))
        .route("/network/stats", get(handlers::get_network_stats))
        .route("/snapshot", get(handlers::get_snapshot))
        .route("/events", get(handlers::sse_events))
        .route("/admin/compact", post(handlers::post_compact))
        .with_state(node)
}
