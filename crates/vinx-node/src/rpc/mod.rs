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
        .route("/account/:address", get(handlers::get_account))
        .route("/tx/submit", post(handlers::submit_tx))
        .route("/block/:height", get(handlers::get_block))
        .route("/mempool/size", get(handlers::get_mempool))
        .route("/validators", get(handlers::get_validators))
        .route("/protocol/version", get(handlers::get_protocol_status))
        .with_state(node)
}
