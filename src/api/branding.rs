// Branding API endpoints
// Returns server branding configuration for Jellyfin clients

use axum::{http::StatusCode, response::IntoResponse, routing::get, Json, Router};
use serde::Serialize;
use std::sync::Arc;

use crate::AppState;

/// Branding configuration options
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct BrandingOptions {
    /// Custom login disclaimer text (displayed on login page)
    pub login_disclaimer: Option<String>,

    /// Custom CSS for theming the web interface
    pub custom_css: Option<String>,

    /// Whether to show a splashscreen on startup
    pub splashscreen_enabled: bool,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Configuration", get(get_branding_configuration))
        .route("/Css", get(get_branding_css))
        .route("/Css.css", get(get_branding_css))
}

/// GET /Branding/Configuration
/// Returns the server's branding options
async fn get_branding_configuration() -> Json<BrandingOptions> {
    Json(BrandingOptions::default())
}

/// GET /Branding/Css or /Branding/Css.css
/// Returns custom CSS for theming (empty by default)
async fn get_branding_css() -> impl IntoResponse {
    (
        StatusCode::OK,
        [("Content-Type", "text/css; charset=utf-8")],
        "",
    )
}
