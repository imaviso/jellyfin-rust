// Favorites API endpoints
// Allows users to mark/unmark items as favorites

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::AppState;

/// Query parameters for favorite operations
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FavoriteQuery {
    pub user_id: Option<String>,
}

/// User item data response (returned after favorite toggle)
#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub rating: Option<f64>,
    pub played_percentage: Option<f64>,
    pub unplayed_item_count: Option<i32>,
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub likes: Option<bool>,
    pub last_played_date: Option<String>,
    pub played: bool,
    pub key: String,
    pub item_id: String,
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:itemId", post(add_favorite))
        .route("/:itemId", delete(remove_favorite))
}

/// POST /UserFavoriteItems/{itemId}
/// Mark an item as favorite
async fn add_favorite(
    State(state): State<Arc<AppState>>,
    Path(item_id): Path<String>,
    Query(query): Query<FavoriteQuery>,
) -> Result<Json<UserItemDataDto>, StatusCode> {
    let user_id = query.user_id.ok_or(StatusCode::BAD_REQUEST)?;

    // Verify item exists
    let item_exists: Option<(String,)> = sqlx::query_as("SELECT id FROM media_items WHERE id = ?")
        .bind(&item_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    if item_exists.is_none() {
        return Err(StatusCode::NOT_FOUND);
    }

    // Insert or ignore if already favorited
    sqlx::query("INSERT OR IGNORE INTO user_favorites (user_id, item_id) VALUES (?, ?)")
        .bind(&user_id)
        .bind(&item_id)
        .execute(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get playback progress for response
    let progress: Option<(i64, bool, i32, Option<String>)> = sqlx::query_as(
        "SELECT position_ticks, played, play_count, last_played FROM playback_progress WHERE user_id = ? AND item_id = ?",
    )
    .bind(&user_id)
    .bind(&item_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (position_ticks, played, play_count, last_played) = progress.unwrap_or((0, false, 0, None));

    Ok(Json(UserItemDataDto {
        rating: None,
        played_percentage: None,
        unplayed_item_count: None,
        playback_position_ticks: position_ticks,
        play_count,
        is_favorite: true,
        likes: None,
        last_played_date: last_played,
        played,
        key: format!("{}-{}", user_id, item_id),
        item_id,
    }))
}

/// DELETE /UserFavoriteItems/{itemId}
/// Remove an item from favorites
async fn remove_favorite(
    State(state): State<Arc<AppState>>,
    Path(item_id): Path<String>,
    Query(query): Query<FavoriteQuery>,
) -> Result<Json<UserItemDataDto>, StatusCode> {
    let user_id = query.user_id.ok_or(StatusCode::BAD_REQUEST)?;

    // Delete the favorite
    sqlx::query("DELETE FROM user_favorites WHERE user_id = ? AND item_id = ?")
        .bind(&user_id)
        .bind(&item_id)
        .execute(&state.db)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Get playback progress for response
    let progress: Option<(i64, bool, i32, Option<String>)> = sqlx::query_as(
        "SELECT position_ticks, played, play_count, last_played FROM playback_progress WHERE user_id = ? AND item_id = ?",
    )
    .bind(&user_id)
    .bind(&item_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let (position_ticks, played, play_count, last_played) = progress.unwrap_or((0, false, 0, None));

    Ok(Json(UserItemDataDto {
        rating: None,
        played_percentage: None,
        unplayed_item_count: None,
        playback_position_ticks: position_ticks,
        play_count,
        is_favorite: false,
        likes: None,
        last_played_date: last_played,
        played,
        key: format!("{}-{}", user_id, item_id),
        item_id,
    }))
}

/// Check if an item is a favorite for a user
pub async fn is_favorite(pool: &sqlx::SqlitePool, user_id: &str, item_id: &str) -> bool {
    sqlx::query_scalar::<_, i32>("SELECT 1 FROM user_favorites WHERE user_id = ? AND item_id = ?")
        .bind(user_id)
        .bind(item_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .is_some()
}
