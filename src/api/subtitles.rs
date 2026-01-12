// Subtitle extraction and serving endpoint

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Router,
};
use serde::Deserialize;
use std::{path::PathBuf, process::Stdio, sync::Arc};
use tokio::io::AsyncReadExt;
use tokio::process::Command;

use crate::{models::MediaItem, services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        // Standard Jellyfin subtitle URL format
        .route(
            "/:item_id/:media_source_id/Subtitles/:index/:start_ticks/Stream.:format",
            get(get_subtitle),
        )
        // Also support without start_ticks in path
        .route(
            "/:item_id/:media_source_id/Subtitles/:index/Stream.:format",
            get(get_subtitle_no_ticks),
        )
}

#[derive(Debug, Deserialize)]
pub struct SubtitlePath {
    item_id: String,
    #[allow(dead_code)]
    media_source_id: String,
    index: i32,
    #[serde(default)]
    start_ticks: Option<i64>,
    format: String,
}

#[derive(Debug, Deserialize)]
pub struct SubtitlePathNoTicks {
    item_id: String,
    #[allow(dead_code)]
    media_source_id: String,
    index: i32,
    format: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubtitleQuery {
    #[serde(rename = "api_key")]
    pub api_key: Option<String>,
}

async fn require_auth(
    state: &AppState,
    headers: &HeaderMap,
    query_api_key: Option<&str>,
) -> Result<crate::models::User, (StatusCode, String)> {
    // First try api_key from query parameter
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

async fn get_subtitle_no_ticks(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<SubtitlePathNoTicks>,
    Query(query): Query<SubtitleQuery>,
) -> Result<Response, (StatusCode, String)> {
    get_subtitle_inner(
        state,
        headers,
        path.item_id,
        path.index,
        0,
        path.format,
        query,
    )
    .await
}

async fn get_subtitle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<SubtitlePath>,
    Query(query): Query<SubtitleQuery>,
) -> Result<Response, (StatusCode, String)> {
    get_subtitle_inner(
        state,
        headers,
        path.item_id,
        path.index,
        path.start_ticks.unwrap_or(0),
        path.format,
        query,
    )
    .await
}

async fn get_subtitle_inner(
    state: Arc<AppState>,
    headers: HeaderMap,
    item_id: String,
    index: i32,
    start_ticks: i64,
    format: String,
    query: SubtitleQuery,
) -> Result<Response, (StatusCode, String)> {
    let _user = require_auth(&state, &headers, query.api_key.as_deref()).await?;

    // Get the media item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&item_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // Get the file path
    let file_path = item
        .path
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item has no file path".to_string()))?;

    // Convert start_ticks to seconds (1 tick = 100 nanoseconds)
    let start_seconds = start_ticks as f64 / 10_000_000.0;

    // Check cache first (include start_ticks in cache key if non-zero)
    let cache_dir = get_subtitle_cache_dir(&item_id);
    let cache_file = if start_ticks > 0 {
        cache_dir.join(format!("{}_{}.{}", index, start_ticks, &format))
    } else {
        cache_dir.join(format!("{}.{}", index, &format))
    };

    if cache_file.exists() {
        tracing::debug!("Serving cached subtitle: {:?}", cache_file);
        return serve_subtitle_file(&cache_file, &format).await;
    }

    // Extract subtitle using ffmpeg
    tracing::info!(
        "Extracting subtitle stream {} from {} to {} (start: {:.2}s)",
        index,
        file_path,
        format,
        start_seconds
    );

    // Create cache directory
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Determine output codec and format for ffmpeg
    let (output_codec, output_format) = match format.to_lowercase().as_str() {
        "vtt" | "webvtt" => ("webvtt", "webvtt"),
        "srt" | "subrip" => ("srt", "srt"),
        "ass" | "ssa" => ("copy", "ass"), // Copy ASS directly without re-encoding
        _ => ("webvtt", "webvtt"),        // Default to WebVTT
    };

    // Build ffmpeg command
    // If start_ticks > 0, we need to offset the subtitle timestamps
    let mut cmd = Command::new(find_ffmpeg());
    cmd.args(["-i", file_path]);

    // Add timestamp offset if seeking
    if start_ticks > 0 {
        // Use -ss after -i for subtitle streams to properly offset
        cmd.args(["-ss", &format!("{:.3}", start_seconds)]);
    }

    cmd.args([
        "-map",
        &format!("0:{}", index),
        "-c:s",
        output_codec,
        "-f",
        output_format,
        "-",
    ]);

    // Run ffmpeg to extract the subtitle
    let output = cmd
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("Failed to run ffmpeg: {}", e),
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("ffmpeg subtitle extraction failed: {}", stderr);
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Subtitle extraction failed: {}", stderr),
        ));
    }

    let subtitle_data = output.stdout;

    // Cache the result
    if let Err(e) = tokio::fs::write(&cache_file, &subtitle_data).await {
        tracing::warn!("Failed to cache subtitle: {}", e);
    }

    // Serve the subtitle
    let content_type = subtitle_content_type(&format);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, subtitle_data.len())
        .header(header::CACHE_CONTROL, "max-age=31536000") // Cache for 1 year
        .body(Body::from(subtitle_data))
        .unwrap())
}

async fn serve_subtitle_file(
    path: &PathBuf,
    format: &str,
) -> Result<Response, (StatusCode, String)> {
    let mut file = tokio::fs::File::open(path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, e.to_string()))?;

    let mut data = Vec::new();
    file.read_to_end(&mut data)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let content_type = subtitle_content_type(format);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, data.len())
        .header(header::CACHE_CONTROL, "max-age=31536000")
        .body(Body::from(data))
        .unwrap())
}

fn subtitle_content_type(format: &str) -> &'static str {
    match format.to_lowercase().as_str() {
        "vtt" | "webvtt" => "text/vtt; charset=utf-8",
        "srt" | "subrip" => "application/x-subrip; charset=utf-8",
        "ass" | "ssa" => "text/x-ssa; charset=utf-8",
        "ttml" => "application/ttml+xml; charset=utf-8",
        _ => "text/plain; charset=utf-8",
    }
}

fn get_subtitle_cache_dir(item_id: &str) -> PathBuf {
    // Use a cache directory relative to the current working directory
    PathBuf::from("cache").join("subtitles").join(item_id)
}

fn find_ffmpeg() -> String {
    // Check environment variable first
    if let Ok(path) = std::env::var("FFMPEG_PATH") {
        return path;
    }

    // Common locations to check
    let paths = [
        "/nix/store/2v155vxx0l5ysxjpsw5hnxwjs2c5p785-ffmpeg-8.0-bin/bin/ffmpeg",
        "/usr/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/opt/homebrew/bin/ffmpeg",
    ];

    for path in paths {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }

    // Fall back to PATH lookup
    "ffmpeg".to_string()
}
