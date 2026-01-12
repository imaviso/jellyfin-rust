use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{services::auth, AppState};

use super::sessions;
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Playing", post(on_playback_start))
        .route("/Playing/Progress", post(on_playback_progress))
        .route("/Playing/Stopped", post(on_playback_stopped))
        .route("/Logout", post(logout))
}

pub fn user_played_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:itemId", post(mark_played))
        .route("/:itemId", delete(mark_unplayed))
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStartInfo {
    pub item_id: String,
    #[serde(default)]
    pub position_ticks: Option<i64>,
    pub media_source_id: Option<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub play_method: Option<String>,
    pub play_session_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackProgressInfo {
    pub item_id: String,
    #[serde(default)]
    pub position_ticks: i64,
    pub is_paused: Option<bool>,
    pub is_muted: Option<bool>,
    pub volume_level: Option<i32>,
    pub play_method: Option<String>,
    pub play_session_id: Option<String>,
    pub repeat_mode: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackStopInfo {
    pub item_id: String,
    #[serde(default)]
    pub position_ticks: i64,
    pub media_source_id: Option<String>,
    pub play_session_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub played: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played_date: Option<String>,
    pub item_id: String,
}

/// POST /Sessions/Playing - Called when playback starts
async fn on_playback_start(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(info): Json<PlaybackStartInfo>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Extract device info from auth header
    let (client, device_name, device_id, _) = parse_emby_auth_header(&headers).unwrap_or((
        "Unknown".to_string(),
        "Unknown".to_string(),
        "unknown".to_string(),
        None,
    ));

    let position = info.position_ticks.unwrap_or(0);
    let now = chrono::Utc::now().to_rfc3339();

    tracing::info!(
        "Playback started: user={}, item={}, position={}",
        user.id,
        info.item_id,
        position
    );

    // Upsert playback progress
    sqlx::query(
        r#"
        INSERT INTO playback_progress (user_id, item_id, position_ticks, last_played)
        VALUES (?, ?, ?, ?)
        ON CONFLICT (user_id, item_id) DO UPDATE SET
            position_ticks = excluded.position_ticks,
            last_played = excluded.last_played
        "#,
    )
    .bind(&user.id)
    .bind(&info.item_id)
    .bind(position)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Update active session
    let _ = sessions::update_session_playback(
        &state.db,
        &user.id,
        &device_id,
        &device_name,
        &client,
        &info.item_id,
        position,
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/Playing/Progress - Called periodically during playback
async fn on_playback_progress(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(info): Json<PlaybackProgressInfo>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Extract device info from auth header
    let (_, _, device_id, _) = parse_emby_auth_header(&headers).unwrap_or((
        "Unknown".to_string(),
        "Unknown".to_string(),
        "unknown".to_string(),
        None,
    ));

    let now = chrono::Utc::now().to_rfc3339();
    let is_paused = info.is_paused.unwrap_or(false);

    tracing::debug!(
        "Playback progress: user={}, item={}, position={}",
        user.id,
        info.item_id,
        info.position_ticks
    );

    // Update position
    sqlx::query(
        r#"
        INSERT INTO playback_progress (user_id, item_id, position_ticks, last_played)
        VALUES (?, ?, ?, ?)
        ON CONFLICT (user_id, item_id) DO UPDATE SET
            position_ticks = excluded.position_ticks,
            last_played = excluded.last_played
        "#,
    )
    .bind(&user.id)
    .bind(&info.item_id)
    .bind(info.position_ticks)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Update active session
    let _ = sessions::update_session_progress(
        &state.db,
        &user.id,
        &device_id,
        info.position_ticks,
        is_paused,
    )
    .await;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/Playing/Stopped - Called when playback stops
async fn on_playback_stopped(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(info): Json<PlaybackStopInfo>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Extract device info from auth header
    let (_, _, device_id, _) = parse_emby_auth_header(&headers).unwrap_or((
        "Unknown".to_string(),
        "Unknown".to_string(),
        "unknown".to_string(),
        None,
    ));

    let now = chrono::Utc::now().to_rfc3339();

    tracing::info!(
        "Playback stopped: user={}, item={}, position={}",
        user.id,
        info.item_id,
        info.position_ticks
    );

    // Get the media item to check runtime for auto-marking as played
    let runtime: Option<(Option<i64>,)> =
        sqlx::query_as("SELECT runtime_ticks FROM media_items WHERE id = ?")
            .bind(&info.item_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Check if we should mark as played (watched > 90% of content)
    let should_mark_played = if let Some((Some(runtime_ticks),)) = runtime {
        runtime_ticks > 0 && info.position_ticks > (runtime_ticks * 90 / 100)
    } else {
        false
    };

    // Update progress
    sqlx::query(
        r#"
        INSERT INTO playback_progress (user_id, item_id, position_ticks, played, play_count, last_played)
        VALUES (?, ?, ?, ?, 1, ?)
        ON CONFLICT (user_id, item_id) DO UPDATE SET
            position_ticks = excluded.position_ticks,
            played = CASE WHEN ? THEN 1 ELSE played END,
            play_count = CASE WHEN ? THEN play_count + 1 ELSE play_count END,
            last_played = excluded.last_played
        "#,
    )
    .bind(&user.id)
    .bind(&info.item_id)
    .bind(if should_mark_played { 0 } else { info.position_ticks }) // Reset to 0 if played
    .bind(should_mark_played)
    .bind(&now)
    .bind(should_mark_played)
    .bind(should_mark_played)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Clear session playback state
    let _ = sessions::clear_session_playback(&state.db, &user.id, &device_id).await;

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/Logout - End the current session
async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    // Try to get the token and delete the session
    if let Some((_, _, _, Some(token))) = parse_emby_auth_header(&headers) {
        // Delete the session from database
        sqlx::query("DELETE FROM sessions WHERE token = ?")
            .bind(&token)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        tracing::info!("Session logged out");
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Users/{userId}/PlayedItems/{itemId} - Mark item as played
async fn mark_played(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(String, String)>,
) -> Result<Json<UserItemDataDto>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Verify the user is modifying their own data
    if user.id != user_id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot modify other user's data".to_string(),
        ));
    }

    let now = chrono::Utc::now().to_rfc3339();

    // Mark as played
    sqlx::query(
        r#"
        INSERT INTO playback_progress (user_id, item_id, position_ticks, played, play_count, last_played)
        VALUES (?, ?, 0, 1, 1, ?)
        ON CONFLICT (user_id, item_id) DO UPDATE SET
            position_ticks = 0,
            played = 1,
            play_count = play_count + 1,
            last_played = excluded.last_played
        "#,
    )
    .bind(&user_id)
    .bind(&item_id)
    .bind(&now)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Return updated user data
    let progress = get_user_item_data(&state, &user_id, &item_id).await?;
    Ok(Json(progress))
}

/// DELETE /Users/{userId}/PlayedItems/{itemId} - Mark item as unplayed
async fn mark_unplayed(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((user_id, item_id)): Path<(String, String)>,
) -> Result<Json<UserItemDataDto>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Verify the user is modifying their own data
    if user.id != user_id && !user.is_admin {
        return Err((
            StatusCode::FORBIDDEN,
            "Cannot modify other user's data".to_string(),
        ));
    }

    // Mark as unplayed
    sqlx::query(
        r#"
        INSERT INTO playback_progress (user_id, item_id, position_ticks, played, play_count)
        VALUES (?, ?, 0, 0, 0)
        ON CONFLICT (user_id, item_id) DO UPDATE SET
            position_ticks = 0,
            played = 0,
            play_count = 0
        "#,
    )
    .bind(&user_id)
    .bind(&item_id)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Return updated user data
    let progress = get_user_item_data(&state, &user_id, &item_id).await?;
    Ok(Json(progress))
}

/// Helper to get user item data
async fn get_user_item_data(
    state: &AppState,
    user_id: &str,
    item_id: &str,
) -> Result<UserItemDataDto, (StatusCode, String)> {
    #[derive(sqlx::FromRow)]
    struct ProgressRow {
        position_ticks: i64,
        played: bool,
        play_count: i32,
        last_played: Option<String>,
    }

    let progress: Option<ProgressRow> = sqlx::query_as(
        "SELECT position_ticks, played, play_count, last_played FROM playback_progress WHERE user_id = ? AND item_id = ?",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Check if favorited
    let is_favorite = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM user_favorites WHERE user_id = ? AND item_id = ?",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(&state.db)
    .await
    .ok()
    .flatten()
    .is_some();

    match progress {
        Some(p) => Ok(UserItemDataDto {
            playback_position_ticks: p.position_ticks,
            play_count: p.play_count,
            is_favorite,
            played: p.played,
            last_played_date: p.last_played,
            item_id: item_id.to_string(),
        }),
        None => Ok(UserItemDataDto {
            playback_position_ticks: 0,
            play_count: 0,
            is_favorite,
            played: false,
            last_played_date: None,
            item_id: item_id.to_string(),
        }),
    }
}

/// Get playback progress for an item - used by items API
pub async fn get_playback_progress(
    db: &sqlx::SqlitePool,
    user_id: &str,
    item_id: &str,
) -> Option<(i64, bool, i32, Option<String>)> {
    #[derive(sqlx::FromRow)]
    struct ProgressRow {
        position_ticks: i64,
        played: bool,
        play_count: i32,
        last_played: Option<String>,
    }

    let progress: Option<ProgressRow> = sqlx::query_as(
        "SELECT position_ticks, played, play_count, last_played FROM playback_progress WHERE user_id = ? AND item_id = ?",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(db)
    .await
    .ok()?;

    progress.map(|p| (p.position_ticks, p.played, p.play_count, p.last_played))
}
