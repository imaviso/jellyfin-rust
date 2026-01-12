// User Views endpoint - returns library sections for the home screen

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{models::Library, services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/", get(get_user_views))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserViewsQuery {
    pub user_id: Option<String>,
    pub include_external_content: Option<bool>,
    pub include_hidden: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserViewsResponse {
    pub items: Vec<UserViewDto>,
    pub total_record_count: i32,
    pub start_index: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserViewDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub collection_type: Option<String>,
    pub server_id: String,
    pub is_folder: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub etag: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub date_created: Option<String>,

    pub can_delete: bool,
    pub can_download: bool,

    pub sort_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub external_urls: Option<Vec<String>>,

    pub path: Option<String>,
    pub enable_media_source_display: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<i32>,

    pub display_preferences_id: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_aspect_ratio: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tags: Option<ImageTagsView>,
}

#[derive(Debug, Serialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ImageTagsView {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
}

async fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<crate::models::User, (StatusCode, String)> {
    let (_, _, _, token) = parse_emby_auth_header(headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))
}

/// GET /UserViews
/// Returns the library views (sections) for the home screen
async fn get_user_views(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(_query): Query<UserViewsQuery>,
) -> Result<Json<UserViewsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get all libraries
    let libraries: Vec<Library> = sqlx::query_as("SELECT * FROM libraries ORDER BY name")
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut items = Vec::new();

    for lib in libraries {
        // Count items in this library
        let child_count: (i32,) = sqlx::query_as(
            "SELECT COUNT(*) FROM media_items WHERE library_id = ? AND parent_id IS NULL",
        )
        .bind(&lib.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

        // Map library type to collection type
        let collection_type = match lib.library_type.as_str() {
            "tvshows" | "TvShows" => Some("tvshows".to_string()),
            "movies" | "Movies" => Some("movies".to_string()),
            "music" | "Music" => Some("music".to_string()),
            _ => Some("mixed".to_string()),
        };

        items.push(UserViewDto {
            id: lib.id.clone(),
            name: lib.name.clone(),
            item_type: "CollectionFolder".to_string(),
            collection_type,
            server_id: "jellyfin-rust-server".to_string(),
            is_folder: true,
            etag: None,
            date_created: Some(lib.created_at.clone()),
            can_delete: false,
            can_download: false,
            sort_name: Some(lib.name.clone()),
            external_urls: None,
            path: Some(lib.path.clone()),
            enable_media_source_display: false,
            child_count: Some(child_count.0),
            display_preferences_id: lib.id.clone(),
            primary_image_aspect_ratio: None,
            image_tags: Some(ImageTagsView::default()),
        });
    }

    let total = items.len() as i32;

    Ok(Json(UserViewsResponse {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}
