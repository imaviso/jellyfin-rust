use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{models::MediaItem, services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:id/stream", get(stream_video))
        .route("/:id/stream.:container", get(stream_video))
        // Jellyfin clients also use these endpoints
        .route("/:id/original", get(stream_video))
        .route("/:id/original.:container", get(stream_video))
        // Trickplay endpoints (seek preview thumbnails)
        .route(
            "/:id/Trickplay/:width/tiles.m3u8",
            get(get_trickplay_playlist),
        )
        .route("/:id/Trickplay/:width/:index", get(get_trickplay_tile))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StreamQuery {
    pub static_stream: Option<bool>,
    pub media_source_id: Option<String>,
    pub device_id: Option<String>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub container: Option<String>,
    // api_key is passed as lowercase query param by clients
    #[serde(rename = "api_key")]
    pub api_key: Option<String>,
    // We ignore most of these since we only do direct play
}

async fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
    query_api_key: Option<&str>,
) -> Result<crate::models::User, (StatusCode, String)> {
    // First try api_key from query parameter (clients like Fladder use this)
    // Fall back to X-Emby-Authorization header
    let token = if let Some(key) = query_api_key {
        Some(key.to_string())
    } else {
        parse_emby_auth_header(headers).and_then(|(_, _, _, t)| t)
    };

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))
}

/// Get the MIME type for a video file based on extension
fn get_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "ts" => "video/mp2t",
        "m2ts" | "mts" => "video/mp2t",
        "ogv" => "video/ogg",
        "3gp" => "video/3gpp",
        _ => "application/octet-stream",
    }
}

/// Parse HTTP Range header (e.g., "bytes=0-1023" or "bytes=1024-")
fn parse_range_header(range_header: Option<&HeaderValue>, file_size: u64) -> Option<(u64, u64)> {
    let range_str = range_header?.to_str().ok()?;

    if !range_str.starts_with("bytes=") {
        return None;
    }

    let range = range_str.trim_start_matches("bytes=");
    let parts: Vec<&str> = range.split('-').collect();

    if parts.len() != 2 {
        return None;
    }

    let start: u64 = if parts[0].is_empty() {
        // Suffix range: "-500" means last 500 bytes
        let suffix_len: u64 = parts[1].parse().ok()?;
        file_size.saturating_sub(suffix_len)
    } else {
        parts[0].parse().ok()?
    };

    let end: u64 = if parts[1].is_empty() {
        file_size - 1
    } else {
        parts[1].parse().ok()?
    };

    // Validate range
    if start > end || start >= file_size {
        return None;
    }

    // Clamp end to file size
    let end = end.min(file_size - 1);

    Some((start, end))
}

async fn stream_video(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path_params): Path<VideoPath>,
    Query(query): Query<StreamQuery>,
) -> Result<Response, (StatusCode, String)> {
    let _user = require_auth(&state, &headers, query.api_key.as_deref()).await?;

    // Get the media item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&path_params.id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // Get the file path
    let file_path = item
        .path
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item has no file path".to_string()))?;

    // Open the file
    let file = File::open(file_path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Cannot open file: {}", e)))?;

    let metadata = file.metadata().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot read file metadata: {}", e),
        )
    })?;

    let file_size = metadata.len();
    let content_type = get_content_type(file_path);

    // Check for Range header
    let range = parse_range_header(headers.get(header::RANGE), file_size);

    match range {
        Some((start, end)) => {
            // Partial content response (206)
            let length = end - start + 1;

            tracing::debug!(
                "Serving range {}-{}/{} for {}",
                start,
                end,
                file_size,
                file_path
            );

            // Seek to start position
            let mut file = file;
            file.seek(std::io::SeekFrom::Start(start))
                .await
                .map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("Seek failed: {}", e),
                    )
                })?;

            // Create a limited reader for the range
            let limited = file.take(length);
            let stream = ReaderStream::new(limited);
            let body = Body::from_stream(stream);

            Ok(Response::builder()
                .status(StatusCode::PARTIAL_CONTENT)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, length)
                .header(
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, file_size),
                )
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(body)
                .unwrap())
        }
        None => {
            // Full content response (200)
            tracing::debug!("Serving full file {} ({} bytes)", file_path, file_size);

            let stream = ReaderStream::new(file);
            let body = Body::from_stream(stream);

            Ok(Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, content_type)
                .header(header::CONTENT_LENGTH, file_size)
                .header(header::ACCEPT_RANGES, "bytes")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(body)
                .unwrap())
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct VideoPath {
    id: String,
    #[serde(default)]
    container: Option<String>,
}

// =============================================================================
// Trickplay endpoints (seek preview thumbnails)
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct TrickplayPath {
    id: String,
    width: i32,
}

#[derive(Debug, Deserialize)]
pub struct TrickplayTilePath {
    id: String,
    width: i32,
    index: String, // e.g., "0.jpg"
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrickplayQuery {
    pub media_source_id: Option<String>,
}

/// GET /Videos/:id/Trickplay/:width/tiles.m3u8 - Get trickplay tiles playlist
///
/// Currently returns 404 as trickplay generation is not yet implemented.
/// When implemented, this would return an HLS playlist pointing to tile images.
async fn get_trickplay_playlist(
    State(_state): State<Arc<AppState>>,
    Path(path): Path<TrickplayPath>,
    Query(_query): Query<TrickplayQuery>,
) -> Result<Response, (StatusCode, String)> {
    // TODO: Implement trickplay generation
    // For now, return 404 - clients will gracefully handle missing trickplay
    tracing::debug!(
        "Trickplay playlist requested for item {} at width {} - not yet implemented",
        path.id,
        path.width
    );

    Err((
        StatusCode::NOT_FOUND,
        "Trickplay not available for this item".to_string(),
    ))
}

/// GET /Videos/:id/Trickplay/:width/:index.jpg - Get trickplay tile image
///
/// Currently returns 404 as trickplay generation is not yet implemented.
async fn get_trickplay_tile(
    State(_state): State<Arc<AppState>>,
    Path(path): Path<TrickplayTilePath>,
    Query(_query): Query<TrickplayQuery>,
) -> Result<Response, (StatusCode, String)> {
    tracing::debug!(
        "Trickplay tile {} requested for item {} at width {} - not yet implemented",
        path.index,
        path.id,
        path.width
    );

    Err((
        StatusCode::NOT_FOUND,
        "Trickplay not available for this item".to_string(),
    ))
}
