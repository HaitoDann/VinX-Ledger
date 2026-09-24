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
    let limiter = RateLimiter::new(node.metrics.clone());
    Router::new()
        .route("/", get(ui::index))
        .route("/admin", get(ui::admin))
        .route("/health", get(handlers::health))
        .route("/chain/height", get(handlers::get_height))
        .route("/chain/sync", get(handlers::get_chain_sync))
        .route("/chain/commits", get(handlers::get_chain_commits))
        .route("/chain/snapshot", get(handlers::get_chain_snapshot))
        .route("/account/:address", get(handlers::get_account))
        .route("/account/:address/txs", get(handlers::get_account_txs))
        .route("/account/:address/proof", get(handlers::get_account_proof))
        .route("/tx/submit", post(handlers::submit_tx))
        .route("/tx/batch", post(handlers::submit_tx_batch))
        .route("/tx/estimate", get(handlers::get_fee_estimate))
        .route("/tx/:hash", get(handlers::get_tx_by_hash))
        .route("/tx/:hash/receipt", get(handlers::get_tx_receipt))
        .route("/block/:height", get(handlers::get_block))
        .route("/mempool/size", get(handlers::get_mempool))
        .route("/validators", get(handlers::get_validators))
        .route(
            "/validators/request",
            post(handlers::post_validator_request),
        )
        .route("/validators/pending", get(handlers::get_validator_requests))
        .route("/protocol/version", get(handlers::get_protocol_status))
        .route("/metrics", get(handlers::get_metrics))
        .route("/network/stats", get(handlers::get_network_stats))
        .route(
            "/snapshot",
            get(handlers::get_snapshot).post(handlers::post_snapshot),
        )
        .route("/events", get(handlers::sse_events))
        .route("/ws", get(handlers::ws_events))
        .route("/admin/compact", post(handlers::post_compact))
        .route("/faucet/request", post(handlers::faucet_request))
        .layer(middleware::from_fn_with_state(limiter, rate_limit))
        .with_state(node)
}
