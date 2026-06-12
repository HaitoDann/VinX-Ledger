pub mod handlers;
pub mod rate_limit;
pub mod types;
pub mod ui;

use axum::{
    middleware,
    routing::{get, post},
    Router,
};
use std::sync::Arc;

use crate::Node;
use rate_limit::{rate_limit, RateLimiter};

pub fn router(node: Arc<Node>) -> Router {
    let limiter = RateLimiter::new();
    Router::new()
        .route("/", get(ui::index))
        .route("/health", get(handlers::health))
        .route("/chain/height", get(handlers::get_height))
        .route("/chain/sync", get(handlers::get_chain_sync))
        .route("/account/:address", get(handlers::get_account))
        .route("/account/:address/txs", get(handlers::get_account_txs))
        .route("/account/:address/proof", get(handlers::get_account_proof))
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
        .route("/ws", get(handlers::ws_events))
        .route("/admin/compact", post(handlers::post_compact))
        .route("/faucet/request", post(handlers::faucet_request))
        .layer(middleware::from_fn_with_state(limiter, rate_limit))
        .with_state(node)
}
