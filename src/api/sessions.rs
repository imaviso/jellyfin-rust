// Sessions API - Active playback session tracking for multi-device support

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{models::MediaItem, services::auth, AppState};

use super::items::{BaseItemDto, ImageTags, UserItemDataDto};
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_sessions))
        .route("/:sessionId/Playing/:command", post(send_playback_command))
        .route("/:sessionId/System/:command", post(send_system_command))
        .route("/:sessionId/Message", post(send_message))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionsQuery {
    pub controllable_by_user_id: Option<String>,
    pub device_id: Option<String>,
    pub active_within_seconds: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfo {
    pub id: String,
    pub user_id: String,
    pub user_name: String,
    pub client: String,
    pub device_name: String,
    pub device_id: String,
    pub device_type: Option<String>,
    pub application_version: Option<String>,
    pub last_activity_date: String,
    pub is_active: bool,
    pub supports_remote_control: bool,
    pub supports_media_control: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub now_playing_item: Option<BaseItemDto>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub play_state: Option<PlayState>,

    pub playable_media_types: Vec<String>,
    pub supported_commands: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PlayState {
    pub position_ticks: i64,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume_level: i32,
    pub play_method: String,
    pub repeat_mode: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackCommandBody {
    pub seek_position_ticks: Option<i64>,
    pub controlling_user_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MessageBody {
    pub text: String,
    pub header: Option<String>,
    pub timeout_ms: Option<i64>,
}

#[derive(Debug, sqlx::FromRow)]
struct SessionRow {
    id: String,
    user_id: String,
    device_id: String,
    device_name: String,
    client: String,
    client_version: Option<String>,
    now_playing_item_id: Option<String>,
    now_playing_position_ticks: Option<i64>,
    is_paused: i32,
    is_muted: i32,
    volume_level: Option<i32>,
    play_method: Option<String>,
    play_state: Option<String>,
    last_activity: String,
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

/// GET /Sessions - Get all active sessions
async fn get_sessions(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SessionsQuery>,
) -> Result<Json<Vec<SessionInfo>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Build query with optional filters
    let active_seconds = query.active_within_seconds.unwrap_or(960); // Default: ~16 minutes
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(active_seconds as i64);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

    let mut sql = String::from(
        "SELECT id, user_id, device_id, device_name, client, client_version, \
         now_playing_item_id, now_playing_position_ticks, is_paused, is_muted, \
         volume_level, play_method, play_state, last_activity \
         FROM active_sessions WHERE last_activity > ?",
    );

    if let Some(ref device_id) = query.device_id {
        sql.push_str(&format!(
            " AND device_id = '{}'",
            device_id.replace('\'', "''")
        ));
    }

    if let Some(ref user_id) = query.controllable_by_user_id {
        // For now, users can control their own sessions
        sql.push_str(&format!(" AND user_id = '{}'", user_id.replace('\'', "''")));
    }

    sql.push_str(" ORDER BY last_activity DESC");

    let sessions: Vec<SessionRow> = sqlx::query_as(&sql)
        .bind(&cutoff_str)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get user names in batch
    let user_ids: Vec<&str> = sessions.iter().map(|s| s.user_id.as_str()).collect();
    let user_names = batch_get_user_names(&state.db, &user_ids).await;

    // Get now playing items in batch
    let item_ids: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s.now_playing_item_id.as_deref())
        .collect();
    let now_playing_items = batch_get_items(&state.db, &item_ids).await;

    // Build response
    let mut result = Vec::with_capacity(sessions.len());
    for session in sessions {
        let user_name = user_names
            .get(&session.user_id)
            .cloned()
            .unwrap_or_else(|| "Unknown".to_string());

        let now_playing_item = session
            .now_playing_item_id
            .as_ref()
            .and_then(|id| now_playing_items.get(id))
            .cloned();

        let play_state = if session.now_playing_item_id.is_some() {
            Some(PlayState {
                position_ticks: session.now_playing_position_ticks.unwrap_or(0),
                can_seek: true,
                is_paused: session.is_paused != 0,
                is_muted: session.is_muted != 0,
                volume_level: session.volume_level.unwrap_or(100),
                play_method: session
                    .play_method
                    .unwrap_or_else(|| "DirectPlay".to_string()),
                repeat_mode: "RepeatNone".to_string(),
            })
        } else {
            None
        };

        result.push(SessionInfo {
            id: session.id,
            user_id: session.user_id,
            user_name,
            client: session.client.clone(),
            device_name: session.device_name,
            device_id: session.device_id,
            device_type: Some(detect_device_type(&session.client)),
            application_version: session.client_version,
            last_activity_date: session.last_activity,
            is_active: true,
            supports_remote_control: true,
            supports_media_control: true,
            now_playing_item,
            play_state,
            playable_media_types: vec!["Video".to_string(), "Audio".to_string()],
            supported_commands: vec![
                "PlayState".to_string(),
                "Seek".to_string(),
                "PlayNext".to_string(),
                "PlayLast".to_string(),
            ],
        });
    }

    Ok(Json(result))
}

/// POST /Sessions/:sessionId/Playing/:command - Send playback command
async fn send_playback_command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session_id, command)): Path<(String, String)>,
    body: Option<Json<PlaybackCommandBody>>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Verify session exists
    let session_exists: Option<(String,)> =
        sqlx::query_as("SELECT id FROM active_sessions WHERE id = ?")
            .bind(&session_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if session_exists.is_none() {
        return Err((StatusCode::NOT_FOUND, "Session not found".to_string()));
    }

    // Handle different commands
    match command.to_lowercase().as_str() {
        "stop" => {
            sqlx::query(
                "UPDATE active_sessions SET now_playing_item_id = NULL, play_state = 'stopped' WHERE id = ?",
            )
            .bind(&session_id)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        "pause" => {
            sqlx::query(
                "UPDATE active_sessions SET is_paused = 1, play_state = 'paused' WHERE id = ?",
            )
            .bind(&session_id)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        "unpause" | "play" => {
            sqlx::query(
                "UPDATE active_sessions SET is_paused = 0, play_state = 'playing' WHERE id = ?",
            )
            .bind(&session_id)
            .execute(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        "seek" => {
            if let Some(Json(body)) = body {
                if let Some(ticks) = body.seek_position_ticks {
                    sqlx::query(
                        "UPDATE active_sessions SET now_playing_position_ticks = ? WHERE id = ?",
                    )
                    .bind(ticks)
                    .bind(&session_id)
                    .execute(&state.db)
                    .await
                    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
                }
            }
        }
        "mute" => {
            sqlx::query("UPDATE active_sessions SET is_muted = 1 WHERE id = ?")
                .bind(&session_id)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        "unmute" => {
            sqlx::query("UPDATE active_sessions SET is_muted = 0 WHERE id = ?")
                .bind(&session_id)
                .execute(&state.db)
                .await
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        }
        _ => {
            tracing::debug!("Unhandled playback command: {}", command);
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/:sessionId/System/:command - Send system command
async fn send_system_command(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((session_id, command)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    tracing::debug!(
        "System command {} for session {}: {}",
        command,
        session_id,
        command
    );

    // System commands are typically client-side (GoHome, GoToSettings, etc.)
    // We just acknowledge them
    Ok(StatusCode::NO_CONTENT)
}

/// POST /Sessions/:sessionId/Message - Send message to session
async fn send_message(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(session_id): Path<String>,
    Json(body): Json<MessageBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    tracing::debug!(
        "Message to session {}: {} - {}",
        session_id,
        body.header.as_deref().unwrap_or("Message"),
        body.text
    );

    // In a real implementation, this would push to a WebSocket connection
    // For now, we just acknowledge
    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Session management helpers
// ============================================================================

/// Update or create a session when playback starts
pub async fn update_session_playback(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    device_id: &str,
    device_name: &str,
    client: &str,
    item_id: &str,
    position_ticks: i64,
) -> anyhow::Result<String> {
    let session_id = format!("{}_{}", user_id, device_id);
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    sqlx::query(
        r#"
        INSERT INTO active_sessions (id, user_id, device_id, device_name, client, 
            now_playing_item_id, now_playing_position_ticks, play_state, last_activity)
        VALUES (?, ?, ?, ?, ?, ?, ?, 'playing', ?)
        ON CONFLICT(user_id, device_id) DO UPDATE SET
            now_playing_item_id = excluded.now_playing_item_id,
            now_playing_position_ticks = excluded.now_playing_position_ticks,
            play_state = 'playing',
            is_paused = 0,
            last_activity = excluded.last_activity
        "#,
    )
    .bind(&session_id)
    .bind(user_id)
    .bind(device_id)
    .bind(device_name)
    .bind(client)
    .bind(item_id)
    .bind(position_ticks)
    .bind(&now)
    .execute(pool)
    .await?;

    Ok(session_id)
}

/// Update session progress
pub async fn update_session_progress(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    device_id: &str,
    position_ticks: i64,
    is_paused: bool,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let play_state = if is_paused { "paused" } else { "playing" };

    sqlx::query(
        r#"
        UPDATE active_sessions 
        SET now_playing_position_ticks = ?, is_paused = ?, play_state = ?, last_activity = ?
        WHERE user_id = ? AND device_id = ?
        "#,
    )
    .bind(position_ticks)
    .bind(is_paused as i32)
    .bind(play_state)
    .bind(&now)
    .bind(user_id)
    .bind(device_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Clear session playback when stopped
pub async fn clear_session_playback(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    device_id: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    sqlx::query(
        r#"
        UPDATE active_sessions 
        SET now_playing_item_id = NULL, now_playing_position_ticks = 0, 
            play_state = 'stopped', last_activity = ?
        WHERE user_id = ? AND device_id = ?
        "#,
    )
    .bind(&now)
    .bind(user_id)
    .bind(device_id)
    .execute(pool)
    .await?;

    Ok(())
}

/// Clean up stale sessions (older than given seconds)
pub async fn cleanup_stale_sessions(
    pool: &sqlx::SqlitePool,
    older_than_secs: i64,
) -> anyhow::Result<i32> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(older_than_secs);
    let cutoff_str = cutoff.format("%Y-%m-%d %H:%M:%S").to_string();

    let result = sqlx::query("DELETE FROM active_sessions WHERE last_activity < ?")
        .bind(&cutoff_str)
        .execute(pool)
        .await?;

    Ok(result.rows_affected() as i32)
}

// ============================================================================
// Helper functions
// ============================================================================

fn detect_device_type(client: &str) -> String {
    let client_lower = client.to_lowercase();
    if client_lower.contains("android") {
        "Android".to_string()
    } else if client_lower.contains("ios")
        || client_lower.contains("iphone")
        || client_lower.contains("ipad")
    {
        "iOS".to_string()
    } else if client_lower.contains("web") || client_lower.contains("browser") {
        "Web".to_string()
    } else if client_lower.contains("roku") {
        "Roku".to_string()
    } else if client_lower.contains("tv")
        || client_lower.contains("firetv")
        || client_lower.contains("androidtv")
    {
        "TV".to_string()
    } else if client_lower.contains("fladder") {
        "Mobile".to_string()
    } else {
        "Unknown".to_string()
    }
}

async fn batch_get_user_names(
    pool: &sqlx::SqlitePool,
    user_ids: &[&str],
) -> std::collections::HashMap<String, String> {
    if user_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    let placeholders: Vec<&str> = user_ids.iter().map(|_| "?").collect();
    let query = format!(
        "SELECT id, name FROM users WHERE id IN ({})",
        placeholders.join(",")
    );

    let mut query_builder = sqlx::query_as::<_, (String, String)>(&query);
    for id in user_ids {
        query_builder = query_builder.bind(*id);
    }

    query_builder
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

async fn batch_get_items(
    pool: &sqlx::SqlitePool,
    item_ids: &[&str],
) -> std::collections::HashMap<String, BaseItemDto> {
    if item_ids.is_empty() {
        return std::collections::HashMap::new();
    }

    let placeholders: Vec<&str> = item_ids.iter().map(|_| "?").collect();
    let query = format!(
        "SELECT * FROM media_items WHERE id IN ({})",
        placeholders.join(",")
    );

    let mut query_builder = sqlx::query_as::<_, MediaItem>(&query);
    for id in item_ids {
        query_builder = query_builder.bind(*id);
    }

    let items: Vec<MediaItem> = query_builder.fetch_all(pool).await.unwrap_or_default();

    items
        .into_iter()
        .map(|item| {
            let is_folder = matches!(
                item.item_type.as_str(),
                "Series" | "Season" | "Folder" | "CollectionFolder"
            );
            let media_type = match item.item_type.as_str() {
                "Episode" | "Movie" => Some("Video".to_string()),
                "Audio" => Some("Audio".to_string()),
                _ => None,
            };

            (
                item.id.clone(),
                BaseItemDto {
                    id: item.id.clone(),
                    name: item.name.clone(),
                    item_type: item.item_type.clone(),
                    server_id: "jellyfin-rust-server".to_string(),
                    parent_id: item.parent_id.clone(),
                    overview: item.overview.clone(),
                    year: item.year,
                    production_year: item.year,
                    index_number: item.index_number,
                    parent_index_number: item.parent_index_number,
                    runtime_ticks: item.runtime_ticks,
                    community_rating: item.community_rating,
                    path: item.path.clone(),
                    premiere_date: item.premiere_date.clone(),
                    sort_name: item.sort_name.clone(),
                    series_id: None,
                    series_name: None,
                    season_id: None,
                    season_name: None,
                    is_folder,
                    child_count: None,
                    media_type,
                    collection_type: None,
                    user_data: UserItemDataDto::default(),
                    image_tags: None,
                    provider_ids: None,
                    media_sources: None,
                    can_download: item.path.is_some(),
                    supports_media_source_display: item.item_type == "Episode"
                        || item.item_type == "Movie",
                },
            )
        })
        .collect()
}
