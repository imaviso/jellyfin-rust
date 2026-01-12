// MediaSegments API - Intro/Outro/Recap markers for skip functionality

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:itemId", get(get_segments))
        .route("/:itemId", post(create_segment))
        .route("/:itemId/:segmentId", delete(delete_segment))
}

/// Segment types as defined by Jellyfin
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum MediaSegmentType {
    Intro,
    Outro,
    Recap,
    Preview,
    Commercial,
}

impl MediaSegmentType {
    fn as_str(&self) -> &'static str {
        match self {
            MediaSegmentType::Intro => "Intro",
            MediaSegmentType::Outro => "Outro",
            MediaSegmentType::Recap => "Recap",
            MediaSegmentType::Preview => "Preview",
            MediaSegmentType::Commercial => "Commercial",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "Intro" => Some(MediaSegmentType::Intro),
            "Outro" => Some(MediaSegmentType::Outro),
            "Recap" => Some(MediaSegmentType::Recap),
            "Preview" => Some(MediaSegmentType::Preview),
            "Commercial" => Some(MediaSegmentType::Commercial),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSegmentDto {
    pub id: String,
    pub item_id: String,
    #[serde(rename = "Type")]
    pub segment_type: String,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSegmentsResponse {
    pub items: Vec<MediaSegmentDto>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetSegmentsQuery {
    pub include_segment_types: Option<String>, // Comma-separated segment types
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateSegmentRequest {
    #[serde(rename = "Type")]
    pub segment_type: String,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

#[derive(Debug, sqlx::FromRow)]
struct SegmentRow {
    id: String,
    item_id: String,
    segment_type: String,
    start_ticks: i64,
    end_ticks: i64,
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

/// GET /MediaSegments/:itemId - Get segments for an item
async fn get_segments(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Query(query): Query<GetSegmentsQuery>,
) -> Result<Json<MediaSegmentsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Build query based on segment type filter
    let segments: Vec<SegmentRow> = if let Some(ref types) = query.include_segment_types {
        let type_list: Vec<&str> = types.split(',').map(|s| s.trim()).collect();
        let placeholders: Vec<String> = type_list
            .iter()
            .map(|t| format!("'{}'", t.replace('\'', "''")))
            .collect();

        let sql = format!(
            "SELECT id, item_id, segment_type, start_ticks, end_ticks FROM media_segments WHERE item_id = ? AND segment_type IN ({}) ORDER BY start_ticks",
            placeholders.join(",")
        );

        sqlx::query_as(&sql)
            .bind(&item_id)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    } else {
        sqlx::query_as(
            "SELECT id, item_id, segment_type, start_ticks, end_ticks FROM media_segments WHERE item_id = ? ORDER BY start_ticks",
        )
        .bind(&item_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    };

    let items = segments
        .into_iter()
        .map(|s| MediaSegmentDto {
            id: s.id,
            item_id: s.item_id,
            segment_type: s.segment_type,
            start_ticks: s.start_ticks,
            end_ticks: s.end_ticks,
        })
        .collect();

    Ok(Json(MediaSegmentsResponse { items }))
}

/// POST /MediaSegments/:itemId - Create a new segment
async fn create_segment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Json(body): Json<CreateSegmentRequest>,
) -> Result<Json<MediaSegmentDto>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Validate segment type
    if MediaSegmentType::from_str(&body.segment_type).is_none() {
        return Err((
            StatusCode::BAD_REQUEST,
            format!("Invalid segment type: {}", body.segment_type),
        ));
    }

    // Validate ticks
    if body.start_ticks < 0 || body.end_ticks <= body.start_ticks {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid start/end ticks".to_string(),
        ));
    }

    let segment_id = uuid::Uuid::new_v4().to_string();

    sqlx::query(
        "INSERT INTO media_segments (id, item_id, segment_type, start_ticks, end_ticks) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&segment_id)
    .bind(&item_id)
    .bind(&body.segment_type)
    .bind(body.start_ticks)
    .bind(body.end_ticks)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(MediaSegmentDto {
        id: segment_id,
        item_id,
        segment_type: body.segment_type,
        start_ticks: body.start_ticks,
        end_ticks: body.end_ticks,
    }))
}

/// DELETE /MediaSegments/:itemId/:segmentId - Delete a segment
async fn delete_segment(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((item_id, segment_id)): Path<(String, String)>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    sqlx::query("DELETE FROM media_segments WHERE id = ? AND item_id = ?")
        .bind(&segment_id)
        .bind(&item_id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

// ============================================================================
// Helper functions for importing segments from external sources
// ============================================================================

/// Import intro/outro data from a .edl file (common format for intros)
/// EDL format: start_seconds end_seconds type
/// Type 0 = cut, 1 = mute, 2 = scene marker, 3 = commercial
pub async fn import_edl_file(
    pool: &sqlx::SqlitePool,
    item_id: &str,
    edl_content: &str,
) -> anyhow::Result<i32> {
    let mut imported = 0;

    for line in edl_content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let start_secs: f64 = parts[0].parse().unwrap_or(0.0);
            let end_secs: f64 = parts[1].parse().unwrap_or(0.0);
            let edl_type: i32 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

            // Convert EDL type to our segment type
            let segment_type = match edl_type {
                3 => "Commercial",
                _ => "Intro", // Default to intro for other types
            };

            // Convert seconds to ticks (10,000,000 ticks per second)
            let start_ticks = (start_secs * 10_000_000.0) as i64;
            let end_ticks = (end_secs * 10_000_000.0) as i64;

            if end_ticks > start_ticks {
                let segment_id = uuid::Uuid::new_v4().to_string();
                let result = sqlx::query(
                    "INSERT OR REPLACE INTO media_segments (id, item_id, segment_type, start_ticks, end_ticks) VALUES (?, ?, ?, ?, ?)",
                )
                .bind(&segment_id)
                .bind(item_id)
                .bind(segment_type)
                .bind(start_ticks)
                .bind(end_ticks)
                .execute(pool)
                .await;

                if result.is_ok() {
                    imported += 1;
                }
            }
        }
    }

    Ok(imported)
}

/// Check if an item has intro segment defined
pub async fn has_intro(pool: &sqlx::SqlitePool, item_id: &str) -> bool {
    sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM media_segments WHERE item_id = ? AND segment_type = 'Intro' LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// Get intro segment for an item (for auto-skip functionality)
pub async fn get_intro(pool: &sqlx::SqlitePool, item_id: &str) -> Option<(i64, i64)> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT start_ticks, end_ticks FROM media_segments WHERE item_id = ? AND segment_type = 'Intro' ORDER BY start_ticks LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    row
}

/// Get outro segment for an item (for "next episode" functionality)
pub async fn get_outro(pool: &sqlx::SqlitePool, item_id: &str) -> Option<(i64, i64)> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT start_ticks, end_ticks FROM media_segments WHERE item_id = ? AND segment_type = 'Outro' ORDER BY start_ticks DESC LIMIT 1",
    )
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    row
}
