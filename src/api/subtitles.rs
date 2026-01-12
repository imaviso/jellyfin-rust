// Subtitle extraction, serving, search, and download endpoints

use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
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

/// Subtitle search routes - mounted under /Items
pub fn search_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/:item_id/RemoteSearch/Subtitles/:language",
            get(search_subtitles),
        )
        .route(
            "/:item_id/RemoteSearch/Subtitles/:subtitle_id",
            post(download_subtitle),
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

// =============================================================================
// Subtitle Search & Download
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteSubtitleInfo {
    pub id: String,
    pub provider_name: String,
    pub name: String,
    pub format: Option<String>,
    pub author: Option<String>,
    pub comment: Option<String>,
    pub date_created: Option<String>,
    pub community_rating: Option<f64>,
    pub download_count: Option<i32>,
    pub is_hash_match: Option<bool>,
    pub is_forced: Option<bool>,
    pub is_hearing_impaired: Option<bool>,
    pub three_letter_iso_language_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SearchSubtitlesPath {
    item_id: String,
    language: String,
}

#[derive(Debug, Deserialize)]
pub struct DownloadSubtitlePath {
    item_id: String,
    subtitle_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchSubtitlesQuery {
    pub is_perfect_match: Option<bool>,
    pub is_forced: Option<bool>,
}

/// GET /Items/{itemId}/RemoteSearch/Subtitles/{language}
/// Search for subtitles from external providers (OpenSubtitles, etc.)
async fn search_subtitles(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<SearchSubtitlesPath>,
    Query(_query): Query<SearchSubtitlesQuery>,
) -> Result<Json<Vec<RemoteSubtitleInfo>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers, None).await?;

    // Get the item to find its details for searching
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&path.item_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    let mut results = Vec::new();

    // Try OpenSubtitles if API key is configured
    if let Ok(api_key) = std::env::var("OPENSUBTITLES_API_KEY") {
        let search_results =
            search_opensubtitles(&api_key, &item, &path.language).await;
        results.extend(search_results);
    }

    // If no external providers configured, return empty result
    // Clients will handle this gracefully
    if results.is_empty() {
        tracing::debug!(
            "No subtitle providers configured or no results for item {} language {}",
            path.item_id,
            path.language
        );
    }

    Ok(Json(results))
}

/// POST /Items/{itemId}/RemoteSearch/Subtitles/{subtitleId}
/// Download a specific subtitle from a provider
async fn download_subtitle(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<DownloadSubtitlePath>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers, None).await?;

    // Get the item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&path.item_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // Parse the subtitle_id to get provider and file info
    // Format: provider:file_id:format (e.g., "opensubtitles:12345:srt")
    let parts: Vec<&str> = path.subtitle_id.split(':').collect();
    if parts.len() < 2 {
        return Err((
            StatusCode::BAD_REQUEST,
            "Invalid subtitle ID format".to_string(),
        ));
    }

    let provider = parts[0];
    let file_id = parts[1];
    let format = parts.get(2).unwrap_or(&"srt");

    match provider {
        "opensubtitles" => {
            if let Ok(api_key) = std::env::var("OPENSUBTITLES_API_KEY") {
                download_opensubtitles_subtitle(&state, &api_key, &item, file_id, format).await?;
            } else {
                return Err((
                    StatusCode::SERVICE_UNAVAILABLE,
                    "OpenSubtitles API key not configured".to_string(),
                ));
            }
        }
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                format!("Unknown subtitle provider: {}", provider),
            ));
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

/// Search OpenSubtitles API for subtitles
async fn search_opensubtitles(
    api_key: &str,
    item: &MediaItem,
    language: &str,
) -> Vec<RemoteSubtitleInfo> {
    let client = reqwest::Client::new();

    // Build search query
    let mut query_params = vec![("languages", language.to_string())];

    // Add IMDB ID if available (best match)
    if let Some(ref imdb_id) = item.imdb_id {
        query_params.push(("imdb_id", imdb_id.clone()));
    } else if let Some(ref tmdb_id) = item.tmdb_id {
        // Use TMDB ID
        if item.item_type == "Movie" {
            query_params.push(("tmdb_id", tmdb_id.clone()));
        }
    } else {
        // Fall back to query by name
        query_params.push(("query", item.name.clone()));
        if let Some(year) = item.year {
            query_params.push(("year", year.to_string()));
        }
    }

    // For episodes, add season and episode numbers
    if item.item_type == "Episode" {
        if let Some(season) = item.parent_index_number {
            query_params.push(("season_number", season.to_string()));
        }
        if let Some(episode) = item.index_number {
            query_params.push(("episode_number", episode.to_string()));
        }
    }

    let response = client
        .get("https://api.opensubtitles.com/api/v1/subtitles")
        .header("Api-Key", api_key)
        .header("Content-Type", "application/json")
        .query(&query_params)
        .send()
        .await;

    let response = match response {
        Ok(r) => r,
        Err(e) => {
            tracing::error!("OpenSubtitles search failed: {}", e);
            return vec![];
        }
    };

    if !response.status().is_success() {
        tracing::error!(
            "OpenSubtitles returned status: {}",
            response.status()
        );
        return vec![];
    }

    let json: serde_json::Value = match response.json().await {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("Failed to parse OpenSubtitles response: {}", e);
            return vec![];
        }
    };

    let mut results = Vec::new();

    if let Some(data) = json.get("data").and_then(|d| d.as_array()) {
        for sub in data.iter().take(20) {
            let attributes = match sub.get("attributes") {
                Some(a) => a,
                None => continue,
            };

            let file_id = sub
                .get("id")
                .and_then(|i| i.as_str())
                .unwrap_or("")
                .to_string();

            let files = attributes
                .get("files")
                .and_then(|f| f.as_array())
                .map(|f| f.first())
                .flatten();

            let format = files
                .and_then(|f| f.get("file_name"))
                .and_then(|n| n.as_str())
                .and_then(|n| n.rsplit('.').next())
                .unwrap_or("srt");

            let name = attributes
                .get("release")
                .and_then(|r| r.as_str())
                .unwrap_or(&item.name)
                .to_string();

            let download_count = attributes
                .get("download_count")
                .and_then(|d| d.as_i64())
                .map(|d| d as i32);

            let hearing_impaired = attributes
                .get("hearing_impaired")
                .and_then(|h| h.as_bool())
                .unwrap_or(false);

            let language_code = attributes
                .get("language")
                .and_then(|l| l.as_str())
                .unwrap_or(language);

            results.push(RemoteSubtitleInfo {
                id: format!("opensubtitles:{}:{}", file_id, format),
                provider_name: "OpenSubtitles".to_string(),
                name,
                format: Some(format.to_string()),
                author: attributes
                    .get("uploader")
                    .and_then(|u| u.get("name"))
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string()),
                comment: attributes
                    .get("comments")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string()),
                date_created: attributes
                    .get("upload_date")
                    .and_then(|d| d.as_str())
                    .map(|s| s.to_string()),
                community_rating: attributes
                    .get("ratings")
                    .and_then(|r| r.as_f64()),
                download_count,
                is_hash_match: Some(false),
                is_forced: Some(false),
                is_hearing_impaired: Some(hearing_impaired),
                three_letter_iso_language_name: Some(language_code.to_string()),
            });
        }
    }

    results
}

/// Download and save an OpenSubtitles subtitle file
async fn download_opensubtitles_subtitle(
    state: &AppState,
    api_key: &str,
    item: &MediaItem,
    file_id: &str,
    format: &str,
) -> Result<(), (StatusCode, String)> {
    let client = reqwest::Client::new();

    // First, get the download link from OpenSubtitles
    let download_response = client
        .post("https://api.opensubtitles.com/api/v1/download")
        .header("Api-Key", api_key)
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "file_id": file_id.parse::<i64>().unwrap_or(0)
        }))
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Download request failed: {}", e)))?;

    if !download_response.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("OpenSubtitles download failed: {}", download_response.status()),
        ));
    }

    let download_json: serde_json::Value = download_response
        .json()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Failed to parse download response: {}", e)))?;

    let download_link = download_json
        .get("link")
        .and_then(|l| l.as_str())
        .ok_or_else(|| (StatusCode::BAD_GATEWAY, "No download link in response".to_string()))?;

    // Download the actual subtitle file
    let subtitle_response = client
        .get(download_link)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Subtitle download failed: {}", e)))?;

    let subtitle_bytes = subtitle_response
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Failed to read subtitle data: {}", e)))?;

    // Save the subtitle file
    let cache_dir = get_subtitle_cache_dir(&item.id);
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Find the next available subtitle index
    let mut index = 100; // Start external subtitles at index 100
    loop {
        let path = cache_dir.join(format!("{}.{}", index, format));
        if !path.exists() {
            break;
        }
        index += 1;
    }

    let subtitle_path = cache_dir.join(format!("{}.{}", index, format));
    tokio::fs::write(&subtitle_path, &subtitle_bytes)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save subtitle: {}", e)))?;

    tracing::info!(
        "Downloaded subtitle for item {} to {:?}",
        item.id,
        subtitle_path
    );

    Ok(())
}
