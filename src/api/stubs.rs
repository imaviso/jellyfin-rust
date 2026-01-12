// Stub endpoints for client compatibility - features not yet implemented

use axum::{routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

/// Routes for /QuickConnect
pub fn quick_connect_routes() -> Router<Arc<AppState>> {
    Router::new().route("/Enabled", get(quick_connect_enabled))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct QuickConnectState {
    pub enabled: bool,
}

/// GET /QuickConnect/Enabled - Check if QuickConnect is enabled
/// Currently always returns false as QuickConnect is not implemented
async fn quick_connect_enabled() -> Json<QuickConnectState> {
    Json(QuickConnectState { enabled: false })
}
