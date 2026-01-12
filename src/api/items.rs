use axum::{
    body::Body,
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::Response,
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::fs::File;
use tokio_util::io::ReaderStream;

use crate::{models::MediaItem, services::auth, services::mediainfo, AppState};

use super::playbackinfo::{MediaSourceInfo, MediaStreamInfo};

/// Build MediaSourceInfo for a media item (used for single item requests)
/// This provides video/audio/subtitle stream info to clients like Fladder
async fn build_media_source_for_item(item: &MediaItem) -> Option<MediaSourceInfo> {
    let file_path = item.path.as_ref()?;

    // Get file size
    let file_size = tokio::fs::metadata(file_path)
        .await
        .ok()
        .map(|m| m.len() as i64);

    // Extract detailed media info using ffprobe
    let media_info = mediainfo::extract_media_info_async(std::path::Path::new(file_path))
        .await
        .ok()?;

    // Build media streams from ffprobe output
    let mut media_streams = Vec::new();

    // Add video stream
    if media_info.video_codec.is_some() {
        media_streams.push(MediaStreamInfo {
            stream_type: "Video".to_string(),
            codec: media_info.video_codec.clone(),
            index: 0,
            is_default: true,
            is_forced: false,
            is_external: false,
            width: media_info.width,
            height: media_info.height,
            bit_rate: media_info.bitrate.map(|b| b as i64),
            aspect_ratio: media_info
                .width
                .zip(media_info.height)
                .map(|(w, h)| format!("{}:{}", w, h)),
            average_frame_rate: None,
            real_frame_rate: None,
            video_range: Some("SDR".to_string()),
            video_range_type: Some("SDR".to_string()),
            pixel_format: None,
            level: None,
            profile: None,
            channels: None,
            sample_rate: None,
            channel_layout: None,
            language: None,
            title: None,
            display_title: media_info.video_codec.as_ref().map(|c| {
                if let (Some(w), Some(h)) = (media_info.width, media_info.height) {
                    format!("{} - {}x{}", c.to_uppercase(), w, h)
                } else {
                    c.to_uppercase()
                }
            }),
            delivery_method: None,
            delivery_url: None,
            is_text_subtitle_stream: None,
            supports_external_stream: None,
        });
    }

    // Add audio streams
    for audio in &media_info.audio_streams {
        let channel_layout = audio.channels.map(|ch| match ch {
            1 => "mono".to_string(),
            2 => "stereo".to_string(),
            6 => "5.1".to_string(),
            8 => "7.1".to_string(),
            _ => format!("{} channels", ch),
        });

        media_streams.push(MediaStreamInfo {
            stream_type: "Audio".to_string(),
            codec: Some(audio.codec.clone()),
            index: audio.index,
            is_default: audio.is_default,
            is_forced: false,
            is_external: false,
            width: None,
            height: None,
            bit_rate: None,
            aspect_ratio: None,
            average_frame_rate: None,
            real_frame_rate: None,
            video_range: None,
            video_range_type: None,
            pixel_format: None,
            level: None,
            profile: None,
            channels: audio.channels,
            sample_rate: audio.sample_rate,
            channel_layout,
            language: audio.language.clone(),
            title: audio.title.clone(),
            display_title: Some(audio.display_title()),
            delivery_method: None,
            delivery_url: None,
            is_text_subtitle_stream: None,
            supports_external_stream: None,
        });
    }

    // Add subtitle streams
    for sub in &media_info.subtitle_streams {
        let is_text = sub.is_text_based();
        let format_ext = match sub.codec.as_str() {
            "ass" | "ssa" => "ass",
            "subrip" | "srt" => "srt",
            "webvtt" | "vtt" => "vtt",
            _ => "srt",
        };
        media_streams.push(MediaStreamInfo {
            stream_type: "Subtitle".to_string(),
            codec: Some(sub.codec.clone()),
            index: sub.index,
            is_default: sub.is_default,
            is_forced: sub.is_forced,
            is_external: false,
            width: None,
            height: None,
            bit_rate: None,
            aspect_ratio: None,
            average_frame_rate: None,
            real_frame_rate: None,
            video_range: None,
            video_range_type: None,
            pixel_format: None,
            level: None,
            profile: None,
            channels: None,
            sample_rate: None,
            channel_layout: None,
            language: sub.language.clone(),
            title: sub.title.clone(),
            display_title: Some(sub.display_title()),
            delivery_method: if is_text {
                Some("External".to_string())
            } else {
                Some("Embed".to_string())
            },
            delivery_url: if is_text {
                Some(format!(
                    "/Videos/{}/{}/Subtitles/{}/0/Stream.{}",
                    item.id, item.id, sub.index, format_ext
                ))
            } else {
                None
            },
            is_text_subtitle_stream: Some(is_text),
            supports_external_stream: Some(is_text),
        });
    }

    // Determine container from path
    let container = file_path.rsplit('.').next().map(|s| s.to_lowercase());

    Some(MediaSourceInfo {
        id: item.id.clone(),
        name: item.name.clone(),
        path: item.path.clone(),
        protocol: "File".to_string(),
        container,
        size: file_size,
        bitrate: media_info.bitrate.map(|b| b as i64),
        runtime_ticks: item.runtime_ticks.or(media_info.duration_ticks),
        source_type: "Default".to_string(),
        is_remote: false,
        read_at_native_framerate: false,
        supports_transcoding: false,
        supports_direct_stream: true,
        supports_direct_play: true,
        is_infinite_stream: false,
        requires_opening: false,
        requires_closing: false,
        requires_looping: false,
        supports_probing: true,
        media_streams,
        direct_stream_url: Some(format!("/Videos/{}/stream", item.id)),
        transcoding_url: None,
        transcoding_sub_protocol: None,
        transcoding_container: None,
    })
}
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_items))
        .route("/Counts", get(get_item_counts))
        .route("/Filters", get(get_item_filters))
        .route("/Filters2", get(get_item_filters2))
        .route("/:id", get(get_item))
        .route("/:id", axum::routing::delete(delete_item))
        .route("/:id/Similar", get(get_similar_items))
        .route("/:id/Refresh", axum::routing::post(refresh_item))
        .route("/:id/Download", get(download_item))
        .route("/:id/RemoteImages", get(get_remote_images))
        .route(
            "/:id/RemoteImages/Download",
            axum::routing::post(download_remote_image),
        )
        .route("/:id/ExternalIdInfos", get(get_external_id_infos))
        .route("/:id/MetadataEditor", get(get_metadata_editor))
        .route(
            "/RemoteSearch/Series",
            axum::routing::post(remote_search_series),
        )
        .route(
            "/RemoteSearch/Movie",
            axum::routing::post(remote_search_movie),
        )
        .route(
            "/RemoteSearch/Apply/:id",
            axum::routing::post(apply_remote_search),
        )
}

pub fn search_routes() -> Router<Arc<AppState>> {
    Router::new().route("/Hints", get(search_hints))
}

// =============================================================================
// Item Counts
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemCounts {
    pub movie_count: i32,
    pub series_count: i32,
    pub episode_count: i32,
    pub artist_count: i32,
    pub program_count: i32,
    pub trailer_count: i32,
    pub song_count: i32,
    pub album_count: i32,
    pub music_video_count: i32,
    pub box_set_count: i32,
    pub book_count: i32,
    pub item_count: i32,
}

/// GET /Items/Counts - Get item counts by type
async fn get_item_counts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<ItemCounts>, (StatusCode, String)> {
    // Validate auth
    let (_, _, _, token) = parse_emby_auth_header(&headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    crate::services::auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    // Count items by type
    let counts: Vec<(String, i32)> =
        sqlx::query_as("SELECT item_type, COUNT(*) as count FROM media_items GROUP BY item_type")
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let mut movie_count = 0;
    let mut series_count = 0;
    let mut episode_count = 0;
    let mut total_count = 0;

    for (item_type, count) in counts {
        total_count += count;
        match item_type.as_str() {
            "Movie" => movie_count = count,
            "Series" => series_count = count,
            "Episode" => episode_count = count,
            _ => {}
        }
    }

    // Count collections as BoxSets
    let box_set_count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM collections")
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    Ok(Json(ItemCounts {
        movie_count,
        series_count,
        episode_count,
        artist_count: 0,
        program_count: 0,
        trailer_count: 0,
        song_count: 0,
        album_count: 0,
        music_video_count: 0,
        box_set_count: box_set_count.0,
        book_count: 0,
        item_count: total_count,
    }))
}

// =============================================================================
// Item Filters
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryFiltersLegacy {
    pub genres: Vec<String>,
    pub tags: Vec<String>,
    pub official_ratings: Vec<String>,
    pub years: Vec<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryFilters {
    pub genres: Vec<NameGuidPair>,
    pub tags: Vec<String>,
    pub official_ratings: Vec<String>,
    pub years: Vec<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NameGuidPair {
    pub name: String,
    pub id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FiltersQuery {
    pub user_id: Option<String>,
    pub parent_id: Option<String>,
    pub include_item_types: Option<String>,
    pub is_airing: Option<bool>,
    pub is_movie: Option<bool>,
    pub is_sports: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_news: Option<bool>,
    pub is_series: Option<bool>,
    pub recursive: Option<bool>,
}

/// GET /Items/Filters - Get filter values (legacy format)
async fn get_item_filters(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<QueryFiltersLegacy>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get distinct genres
    let genres: Vec<(String,)> = if let Some(ref parent_id) = query.parent_id {
        sqlx::query_as(
            "SELECT DISTINCT g.name FROM genres g 
             INNER JOIN item_genres ig ON g.id = ig.genre_id 
             INNER JOIN media_items m ON ig.item_id = m.id 
             WHERE m.library_id = ? 
             ORDER BY g.name",
        )
        .bind(parent_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as("SELECT DISTINCT name FROM genres ORDER BY name")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
    };

    // Get distinct years
    let years: Vec<(i32,)> = if let Some(ref parent_id) = query.parent_id {
        sqlx::query_as(
            "SELECT DISTINCT year FROM media_items 
             WHERE library_id = ? AND year IS NOT NULL 
             ORDER BY year DESC",
        )
        .bind(parent_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as(
            "SELECT DISTINCT year FROM media_items WHERE year IS NOT NULL ORDER BY year DESC",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    };

    Ok(Json(QueryFiltersLegacy {
        genres: genres.into_iter().map(|(g,)| g).collect(),
        tags: vec![], // We don't have tags yet
        official_ratings: vec![], // We don't have ratings yet
        years: years.into_iter().map(|(y,)| y).collect(),
    }))
}

/// GET /Items/Filters2 - Get filter values (new format with IDs)
async fn get_item_filters2(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FiltersQuery>,
) -> Result<Json<QueryFilters>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get genres with IDs
    let genres: Vec<(String, String)> = if let Some(ref parent_id) = query.parent_id {
        sqlx::query_as(
            "SELECT DISTINCT g.name, g.id FROM genres g 
             INNER JOIN item_genres ig ON g.id = ig.genre_id 
             INNER JOIN media_items m ON ig.item_id = m.id 
             WHERE m.library_id = ? 
             ORDER BY g.name",
        )
        .bind(parent_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as("SELECT name, id FROM genres ORDER BY name")
            .fetch_all(&state.db)
            .await
            .unwrap_or_default()
    };

    // Get distinct years
    let years: Vec<(i32,)> = if let Some(ref parent_id) = query.parent_id {
        sqlx::query_as(
            "SELECT DISTINCT year FROM media_items 
             WHERE library_id = ? AND year IS NOT NULL 
             ORDER BY year DESC",
        )
        .bind(parent_id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    } else {
        sqlx::query_as(
            "SELECT DISTINCT year FROM media_items WHERE year IS NOT NULL ORDER BY year DESC",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    };

    Ok(Json(QueryFilters {
        genres: genres
            .into_iter()
            .map(|(name, id)| NameGuidPair { name, id })
            .collect(),
        tags: vec![],
        official_ratings: vec![],
        years: years.into_iter().map(|(y,)| y).collect(),
    }))
}

// =============================================================================
// Delete Item
// =============================================================================

/// DELETE /Items/{id} - Delete an item and its associated data
async fn delete_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    // Require admin for deletion
    let (_, _, _, token) = parse_emby_auth_header(&headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    let user = auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    if !user.is_admin {
        return Err((StatusCode::FORBIDDEN, "Admin required".to_string()));
    }

    // Check if item exists
    let item: Option<MediaItem> = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if item.is_none() {
        return Err((StatusCode::NOT_FOUND, "Item not found".to_string()));
    }

    // Delete associated data
    // Delete images
    sqlx::query("DELETE FROM images WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete playback progress
    sqlx::query("DELETE FROM playback_progress WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete favorites
    sqlx::query("DELETE FROM user_favorites WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete genre links
    sqlx::query("DELETE FROM item_genres WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete studio links
    sqlx::query("DELETE FROM item_studios WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete person links
    sqlx::query("DELETE FROM item_persons WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete media segments
    sqlx::query("DELETE FROM media_segments WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete collection items
    sqlx::query("DELETE FROM collection_items WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Delete playlist items
    sqlx::query("DELETE FROM playlist_items WHERE item_id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Finally, delete the item itself
    sqlx::query("DELETE FROM media_items WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!("Item {} deleted by admin {}", id, user.id);

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// Item Queries
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GetItemsQuery {
    pub parent_id: Option<String>,
    pub include_item_types: Option<String>,
    pub exclude_item_types: Option<String>,
    pub recursive: Option<bool>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub fields: Option<String>,
    pub user_id: Option<String>,
    pub search_term: Option<String>,
    pub is_favorite: Option<bool>,
    pub filters: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsResponse {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: i32,
    pub start_index: i32,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub server_id: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_ticks: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub premiere_date: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub season_name: Option<String>,

    pub is_folder: bool,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub child_count: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub collection_type: Option<String>,

    pub user_data: UserItemDataDto,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tags: Option<ImageTags>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<ProviderIds>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_sources: Option<Vec<MediaSourceInfo>>,

    pub can_download: bool,
    pub supports_media_source_display: bool,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub played: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_played_date: Option<String>,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ImageTags {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop: Option<String>,
}

/// Provider IDs map (e.g., Tmdb, Imdb, AniList, Mal)
pub type ProviderIds = std::collections::HashMap<String, String>;

/// Helper to fetch image tags for an item from the database
async fn get_image_tags_for_item(pool: &sqlx::SqlitePool, item_id: &str) -> Option<ImageTags> {
    let images: Vec<(String,)> = sqlx::query_as("SELECT image_type FROM images WHERE item_id = ?")
        .bind(item_id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();

    if images.is_empty() {
        return None;
    }

    let mut tags = ImageTags::default();
    for (image_type,) in images {
        match image_type.as_str() {
            "Primary" => tags.primary = Some(item_id.to_string()),
            "Backdrop" => tags.backdrop = Some(item_id.to_string()),
            _ => {}
        }
    }

    if tags.primary.is_some() || tags.backdrop.is_some() {
        Some(tags)
    } else {
        None
    }
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

/// Get user-specific data for an item (playback progress, favorites, etc.)
pub async fn get_user_item_data(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    item_id: &str,
) -> UserItemDataDto {
    // Get playback progress
    let progress: Option<(i64, bool, i32, Option<String>)> = sqlx::query_as(
        "SELECT position_ticks, played, play_count, last_played FROM playback_progress WHERE user_id = ? AND item_id = ?",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    let (position_ticks, played, play_count, last_played) = progress.unwrap_or((0, false, 0, None));

    // Check if favorited
    let is_favorite = sqlx::query_scalar::<_, i32>(
        "SELECT 1 FROM user_favorites WHERE user_id = ? AND item_id = ?",
    )
    .bind(user_id)
    .bind(item_id)
    .fetch_optional(pool)
    .await
    .ok()
    .flatten()
    .is_some();

    UserItemDataDto {
        playback_position_ticks: position_ticks,
        play_count,
        is_favorite,
        played,
        last_played_date: last_played,
    }
}

// =============================================================================
// Batch query helpers to avoid N+1 query patterns
// =============================================================================

use std::collections::{HashMap, HashSet};

/// Batch fetch child counts for multiple parent IDs
async fn batch_get_child_counts(
    pool: &sqlx::SqlitePool,
    parent_ids: &[&str],
) -> HashMap<String, i32> {
    if parent_ids.is_empty() {
        return HashMap::new();
    }

    // Build query with placeholders
    let placeholders: Vec<&str> = parent_ids.iter().map(|_| "?").collect();
    let query = format!(
        "SELECT parent_id, COUNT(*) as cnt FROM media_items WHERE parent_id IN ({}) GROUP BY parent_id",
        placeholders.join(",")
    );

    let mut query_builder = sqlx::query_as::<_, (String, i32)>(&query);
    for id in parent_ids {
        query_builder = query_builder.bind(*id);
    }

    query_builder
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Batch fetch parent items (for getting series names for episodes)
async fn batch_get_parent_names(
    pool: &sqlx::SqlitePool,
    parent_ids: &[&str],
) -> HashMap<String, String> {
    if parent_ids.is_empty() {
        return HashMap::new();
    }

    let placeholders: Vec<&str> = parent_ids.iter().map(|_| "?").collect();
    let query = format!(
        "SELECT id, name FROM media_items WHERE id IN ({})",
        placeholders.join(",")
    );

    let mut query_builder = sqlx::query_as::<_, (String, String)>(&query);
    for id in parent_ids {
        query_builder = query_builder.bind(*id);
    }

    query_builder
        .fetch_all(pool)
        .await
        .unwrap_or_default()
        .into_iter()
        .collect()
}

/// Batch fetch image tags for multiple items
async fn batch_get_image_tags(
    pool: &sqlx::SqlitePool,
    item_ids: &[&str],
) -> HashMap<String, ImageTags> {
    if item_ids.is_empty() {
        return HashMap::new();
    }

    let placeholders: Vec<&str> = item_ids.iter().map(|_| "?").collect();
    let query = format!(
        "SELECT item_id, image_type FROM images WHERE item_id IN ({})",
        placeholders.join(",")
    );

    let mut query_builder = sqlx::query_as::<_, (String, String)>(&query);
    for id in item_ids {
        query_builder = query_builder.bind(*id);
    }

    let rows = query_builder.fetch_all(pool).await.unwrap_or_default();

    let mut result: HashMap<String, ImageTags> = HashMap::new();
    for (item_id, image_type) in rows {
        let tags = result.entry(item_id.clone()).or_default();
        match image_type.as_str() {
            "Primary" => tags.primary = Some(item_id),
            "Backdrop" => tags.backdrop = Some(item_id.clone()),
            _ => {}
        }
    }
    result
}

/// Batch fetch user data (playback progress + favorites) for multiple items
async fn batch_get_user_data(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    item_ids: &[&str],
) -> HashMap<String, UserItemDataDto> {
    if item_ids.is_empty() {
        return HashMap::new();
    }

    let placeholders: Vec<&str> = item_ids.iter().map(|_| "?").collect();

    // Fetch playback progress
    let progress_query = format!(
        "SELECT item_id, position_ticks, played, play_count, last_played FROM playback_progress WHERE user_id = ? AND item_id IN ({})",
        placeholders.join(",")
    );

    let mut progress_builder =
        sqlx::query_as::<_, (String, i64, bool, i32, Option<String>)>(&progress_query);
    progress_builder = progress_builder.bind(user_id);
    for id in item_ids {
        progress_builder = progress_builder.bind(*id);
    }

    let progress_rows = progress_builder.fetch_all(pool).await.unwrap_or_default();
    let progress_map: HashMap<String, (i64, bool, i32, Option<String>)> = progress_rows
        .into_iter()
        .map(|(id, pos, played, count, last)| (id, (pos, played, count, last)))
        .collect();

    // Fetch favorites
    let favorites_query = format!(
        "SELECT item_id FROM user_favorites WHERE user_id = ? AND item_id IN ({})",
        placeholders.join(",")
    );

    let mut favorites_builder = sqlx::query_as::<_, (String,)>(&favorites_query);
    favorites_builder = favorites_builder.bind(user_id);
    for id in item_ids {
        favorites_builder = favorites_builder.bind(*id);
    }

    let favorite_rows = favorites_builder.fetch_all(pool).await.unwrap_or_default();
    let favorites: HashSet<String> = favorite_rows.into_iter().map(|(id,)| id).collect();

    // Build result map
    let mut result = HashMap::new();
    for id in item_ids {
        let (position_ticks, played, play_count, last_played) = progress_map
            .get(*id)
            .cloned()
            .unwrap_or((0, false, 0, None));
        let is_favorite = favorites.contains(*id);

        result.insert(
            id.to_string(),
            UserItemDataDto {
                playback_position_ticks: position_ticks,
                play_count,
                is_favorite,
                played,
                last_played_date: last_played,
            },
        );
    }
    result
}

fn media_item_to_dto(
    item: &MediaItem,
    child_count: Option<i32>,
    series_name: Option<String>,
    image_tags: Option<ImageTags>,
    user_data: Option<UserItemDataDto>,
) -> BaseItemDto {
    let is_folder = matches!(
        item.item_type.as_str(),
        "Series" | "Season" | "Folder" | "CollectionFolder"
    );
    let media_type = match item.item_type.as_str() {
        "Episode" | "Movie" => Some("Video".to_string()),
        "Audio" => Some("Audio".to_string()),
        _ => None,
    };

    // Build provider IDs map
    let provider_ids = {
        let mut ids = ProviderIds::new();
        if let Some(ref id) = item.tmdb_id {
            ids.insert("Tmdb".to_string(), id.clone());
        }
        if let Some(ref id) = item.imdb_id {
            ids.insert("Imdb".to_string(), id.clone());
        }
        if let Some(ref id) = item.anilist_id {
            ids.insert("AniList".to_string(), id.clone());
        }
        if let Some(ref id) = item.mal_id {
            ids.insert("Mal".to_string(), id.clone());
        }
        if let Some(ref id) = item.anidb_id {
            ids.insert("AniDb".to_string(), id.clone());
        }
        if let Some(ref id) = item.kitsu_id {
            ids.insert("Kitsu".to_string(), id.clone());
        }
        if ids.is_empty() {
            None
        } else {
            Some(ids)
        }
    };

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
        // For episodes: parent_id is the series_id (episodes are direct children of series)
        series_id: if item.item_type == "Episode" {
            item.parent_id.clone()
        } else {
            None
        },
        series_name,
        // Generate synthetic season_id for episodes: {series_id}_season_{season_number}
        season_id: if item.item_type == "Episode" {
            if let (Some(ref series_id), Some(season_num)) =
                (&item.parent_id, item.parent_index_number)
            {
                Some(format!("{}_season_{}", series_id, season_num))
            } else {
                None
            }
        } else {
            None
        },
        season_name: item.parent_index_number.map(|s| {
            if s == 0 {
                "Specials".to_string()
            } else {
                format!("Season {}", s)
            }
        }),
        is_folder,
        child_count,
        media_type,
        collection_type: None,
        user_data: user_data.unwrap_or_default(),
        image_tags,
        provider_ids,
        media_sources: None, // Populated separately for single item requests
        can_download: item.path.is_some(),
        supports_media_source_display: item.item_type == "Episode" || item.item_type == "Movie",
    }
}

async fn get_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<GetItemsQuery>,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;
    let user_id = query.user_id.as_deref().unwrap_or(&user.id);

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(1000);

    // Parse item types once for reuse
    let include_types: Option<Vec<&str>> = query
        .include_item_types
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim()).collect());

    // Determine sort column (whitelist to prevent injection)
    let sort_by = query.sort_by.as_deref().unwrap_or("SortName");
    let order_col = match sort_by {
        "DateCreated" => "created_at",
        "PremiereDate" => "premiere_date",
        "IndexNumber" => "index_number",
        "CommunityRating" => "community_rating",
        "Name" => "name",
        _ => "sort_name",
    };
    let sort_order = if query.sort_order.as_deref() == Some("Descending") {
        "DESC"
    } else {
        "ASC"
    };

    // Build main query using QueryBuilder for safe parameter binding
    let mut qb: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new("SELECT * FROM media_items WHERE 1=1");

    // Filter by parent
    if let Some(ref parent_id) = query.parent_id {
        qb.push(" AND parent_id = ").push_bind(parent_id.clone());
    } else if !query.recursive.unwrap_or(false) {
        qb.push(" AND parent_id IS NULL");
    }

    // Filter by item types using tuple binding
    if let Some(ref types) = include_types {
        qb.push(" AND item_type IN (");
        let mut separated = qb.separated(", ");
        for t in types {
            separated.push_bind(t.to_string());
        }
        separated.push_unseparated(")");
    }

    // Search term - case insensitive search
    if let Some(ref term) = query.search_term {
        let search_pattern = format!("%{}%", term.to_lowercase());
        qb.push(" AND (LOWER(name) LIKE ")
            .push_bind(search_pattern.clone())
            .push(" OR LOWER(COALESCE(overview, '')) LIKE ")
            .push_bind(search_pattern)
            .push(")");
    }

    // Filter by favorites using subquery with bound parameter
    if query.is_favorite == Some(true) {
        qb.push(" AND id IN (SELECT item_id FROM user_favorites WHERE user_id = ")
            .push_bind(user_id.to_string())
            .push(")");
    }

    // Sort and pagination (column names are whitelisted, not user input)
    qb.push(" ORDER BY ")
        .push(order_col)
        .push(" ")
        .push(sort_order)
        .push(" LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(start_index);

    // Execute main query
    let items: Vec<MediaItem> = qb
        .build_query_as()
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Build count query with same filters
    let mut count_qb: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new("SELECT COUNT(*) FROM media_items WHERE 1=1");

    if let Some(ref parent_id) = query.parent_id {
        count_qb
            .push(" AND parent_id = ")
            .push_bind(parent_id.clone());
    } else if !query.recursive.unwrap_or(false) {
        count_qb.push(" AND parent_id IS NULL");
    }

    if let Some(ref types) = include_types {
        count_qb.push(" AND item_type IN (");
        let mut separated = count_qb.separated(", ");
        for t in types {
            separated.push_bind(t.to_string());
        }
        separated.push_unseparated(")");
    }

    if let Some(ref term) = query.search_term {
        let search_pattern = format!("%{}%", term.to_lowercase());
        count_qb
            .push(" AND (LOWER(name) LIKE ")
            .push_bind(search_pattern.clone())
            .push(" OR LOWER(COALESCE(overview, '')) LIKE ")
            .push_bind(search_pattern)
            .push(")");
    }

    if query.is_favorite == Some(true) {
        count_qb
            .push(" AND id IN (SELECT item_id FROM user_favorites WHERE user_id = ")
            .push_bind(user_id.to_string())
            .push(")");
    }

    let total: (i32,) = count_qb
        .build_query_as()
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Batch fetch all related data to avoid N+1 queries
    // Collect IDs for batch queries
    let item_ids: Vec<&str> = items.iter().map(|i| i.id.as_str()).collect();

    // Items that need child counts (Series/Season)
    let folder_ids: Vec<&str> = items
        .iter()
        .filter(|i| matches!(i.item_type.as_str(), "Series" | "Season"))
        .map(|i| i.id.as_str())
        .collect();

    // Parent IDs for episodes (to get series names)
    let episode_parent_ids: Vec<&str> = items
        .iter()
        .filter(|i| i.item_type == "Episode")
        .filter_map(|i| i.parent_id.as_deref())
        .collect();

    // Execute batch queries in parallel
    let (child_counts, parent_names, image_tags_map, user_data_map) = tokio::join!(
        batch_get_child_counts(&state.db, &folder_ids),
        batch_get_parent_names(&state.db, &episode_parent_ids),
        batch_get_image_tags(&state.db, &item_ids),
        batch_get_user_data(&state.db, user_id, &item_ids),
    );

    // Convert to DTOs using batched data
    let mut dtos = Vec::with_capacity(items.len());
    for item in &items {
        let child_count = if matches!(item.item_type.as_str(), "Series" | "Season") {
            child_counts.get(&item.id).copied()
        } else {
            None
        };

        let series_name = if item.item_type == "Episode" {
            item.parent_id
                .as_ref()
                .and_then(|pid| parent_names.get(pid).cloned())
        } else {
            None
        };

        let image_tags = image_tags_map.get(&item.id).cloned();
        let user_data = user_data_map.get(&item.id).cloned().unwrap_or_default();

        dtos.push(media_item_to_dto(
            item,
            child_count,
            series_name,
            image_tags,
            Some(user_data),
        ));
    }

    Ok(Json(ItemsResponse {
        items: dtos,
        total_record_count: total.0,
        start_index,
    }))
}

async fn get_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<BaseItemDto>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Check if this is a synthetic season ID (format: {series_id}_season_{num})
    if let Some(pos) = id.rfind("_season_") {
        let series_id = &id[..pos];
        let season_num: i32 = id[pos + 8..].parse().unwrap_or(1);

        // Get the series
        let series: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
            .bind(series_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "Series not found".to_string()))?;

        // Count episodes in this season
        let episode_count: (i32,) = sqlx::query_as(
            "SELECT COUNT(*) FROM media_items 
             WHERE parent_id = ? AND item_type = 'Episode' AND COALESCE(parent_index_number, 1) = ?",
        )
        .bind(series_id)
        .bind(season_num)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

        // Get image tags from series
        let image_tags = get_image_tags_for_item(&state.db, series_id).await;

        // Season name
        let season_name = if season_num == 0 {
            "Specials".to_string()
        } else {
            format!("Season {}", season_num)
        };

        let sort_name = if season_num == 0 {
            "Season 999".to_string()
        } else {
            format!("Season {:03}", season_num)
        };

        // Build synthetic season DTO
        let dto = BaseItemDto {
            id: id.clone(),
            name: season_name,
            item_type: "Season".to_string(),
            server_id: "jellyfin-rust-server".to_string(),
            parent_id: Some(series_id.to_string()),
            overview: None,
            year: series.year,
            production_year: series.year,
            index_number: Some(season_num),
            parent_index_number: None,
            runtime_ticks: None,
            community_rating: None,
            path: None,
            premiere_date: None,
            sort_name: Some(sort_name),
            series_id: Some(series_id.to_string()),
            series_name: Some(series.name.clone()),
            season_id: None,
            season_name: None,
            is_folder: true,
            child_count: Some(episode_count.0),
            media_type: None,
            collection_type: None,
            user_data: UserItemDataDto::default(),
            image_tags,
            provider_ids: None,
            media_sources: None,
            can_download: false,
            supports_media_source_display: false,
        };

        return Ok(Json(dto));
    }

    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // Get child count for folders
    let child_count = if matches!(item.item_type.as_str(), "Series" | "Season") {
        let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM media_items WHERE parent_id = ?")
            .bind(&item.id)
            .fetch_one(&state.db)
            .await
            .unwrap_or((0,));
        Some(count.0)
    } else {
        None
    };

    // Get series name for episodes
    let series_name = if item.item_type == "Episode" {
        if let Some(ref parent_id) = item.parent_id {
            let parent: Option<MediaItem> =
                sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
                    .bind(parent_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
            parent.map(|p| p.name)
        } else {
            None
        }
    } else {
        None
    };

    // Get image tags
    let image_tags = get_image_tags_for_item(&state.db, &item.id).await;

    // Get user-specific data
    let user_data = get_user_item_data(&state.db, &user.id, &item.id).await;

    let mut dto = media_item_to_dto(&item, child_count, series_name, image_tags, Some(user_data));

    // For video items, populate media_sources with stream info (fixes "null null" badge in Fladder)
    if matches!(item.item_type.as_str(), "Episode" | "Movie") {
        if let Some(media_source) = build_media_source_for_item(&item).await {
            dto.media_sources = Some(vec![media_source]);
        }
    }

    Ok(Json(dto))
}

async fn get_similar_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Get the source item to find its type and genres
    let source_item: Option<MediaItem> = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let source = match source_item {
        Some(item) => item,
        None => {
            return Ok(Json(ItemsResponse {
                items: vec![],
                total_record_count: 0,
                start_index: 0,
            }))
        }
    };

    // Get genres of the source item
    let source_genres: Vec<(String,)> =
        sqlx::query_as("SELECT genre_id FROM item_genres WHERE item_id = ?")
            .bind(&id)
            .fetch_all(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if source_genres.is_empty() {
        // No genres to match on - return empty
        return Ok(Json(ItemsResponse {
            items: vec![],
            total_record_count: 0,
            start_index: 0,
        }));
    }

    let genre_ids: Vec<String> = source_genres.into_iter().map(|(g,)| g).collect();

    // Find items that share genres with the source item
    // Ordered by number of shared genres (most similar first)
    let similar_items: Vec<MediaItem> = sqlx::query_as(
        r#"
        SELECT m.*, COUNT(ig.genre_id) as shared_genres
        FROM media_items m
        JOIN item_genres ig ON m.id = ig.item_id
        WHERE ig.genre_id IN (SELECT value FROM json_each(?))
          AND m.id != ?
          AND m.item_type = ?
        GROUP BY m.id
        ORDER BY shared_genres DESC, m.community_rating DESC NULLS LAST
        LIMIT 12
        "#,
    )
    .bind(serde_json::to_string(&genre_ids).unwrap_or_default())
    .bind(&id)
    .bind(&source.item_type)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = similar_items.len() as i32;

    // Convert to DTOs
    let mut dtos = Vec::with_capacity(similar_items.len());
    for item in similar_items {
        let is_folder = matches!(
            item.item_type.as_str(),
            "Series" | "Season" | "Folder" | "CollectionFolder"
        );
        let media_type = match item.item_type.as_str() {
            "Episode" | "Movie" => Some("Video".to_string()),
            "Audio" => Some("Audio".to_string()),
            _ => None,
        };

        let image_tags = get_image_tags_for_item(&state.db, &item.id).await;
        let user_data = get_user_item_data(&state.db, &user.id, &item.id).await;

        dtos.push(BaseItemDto {
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
            user_data,
            image_tags,
            provider_ids: None,
            media_sources: None,
            can_download: item.path.is_some(),
            supports_media_source_display: item.item_type == "Episode" || item.item_type == "Movie",
        });
    }

    Ok(Json(ItemsResponse {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    }))
}

// User-specific item endpoints (called as /Users/{userId}/Items)
pub async fn get_user_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(_user_id): Path<String>,
    Query(query): Query<GetItemsQuery>,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    // Delegate to regular get_items - user-specific data (playback progress)
    // would be merged in a full implementation
    get_items(State(state), headers, Query(query)).await
}

pub async fn get_user_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path((_user_id, item_id)): Path<(String, String)>,
) -> Result<Json<BaseItemDto>, (StatusCode, String)> {
    get_item(State(state), headers, Path(item_id)).await
}

// Search hints query
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchHintsQuery {
    pub search_term: Option<String>,
    pub limit: Option<i32>,
    pub include_item_types: Option<String>,
    pub exclude_item_types: Option<String>,
    pub media_types: Option<String>,
    pub is_movie: Option<bool>,
    pub is_series: Option<bool>,
    pub is_news: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_sports: Option<bool>,
    #[serde(rename = "userId")]
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHintsResponse {
    pub search_hints: Vec<SearchHint>,
    pub total_record_count: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHint {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub year: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary_image_tag: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_image_tag: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb_image_item_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_image_tag: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub backdrop_image_item_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_name: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub series_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub runtime_ticks: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub media_type: Option<String>,

    pub is_folder: bool,
    pub run_time_ticks: Option<i64>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_name: Option<String>,
}

/// GET /Search/Hints - Search for items with type-ahead hints
async fn search_hints(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<SearchHintsQuery>,
) -> Result<Json<SearchHintsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let search_term = match query.search_term {
        Some(ref term) if !term.is_empty() => term.clone(),
        _ => {
            return Ok(Json(SearchHintsResponse {
                search_hints: vec![],
                total_record_count: 0,
            }))
        }
    };

    let limit = query.limit.unwrap_or(20).min(100);

    // Try FTS search first, fall back to LIKE if FTS fails
    let items: Vec<MediaItem> = match search_with_fts(&state.db, &search_term, &query, limit).await
    {
        Ok(items) => items,
        Err(_) => {
            // Fallback to LIKE search
            search_with_like(&state.db, &search_term, &query, limit).await?
        }
    };

    // Convert to search hints
    let mut hints = Vec::with_capacity(items.len());
    for item in &items {
        // Get series name for episodes
        let (series_name, series_id) = if item.item_type == "Episode" {
            if let Some(ref parent_id) = item.parent_id {
                let parent: Option<MediaItem> =
                    sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
                        .bind(parent_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                (
                    parent.as_ref().map(|p| p.name.clone()),
                    Some(parent_id.clone()),
                )
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };

        let is_folder = matches!(
            item.item_type.as_str(),
            "Series" | "Season" | "Folder" | "CollectionFolder"
        );
        let media_type = match item.item_type.as_str() {
            "Episode" | "Movie" => Some("Video".to_string()),
            "Audio" => Some("Audio".to_string()),
            _ => None,
        };

        hints.push(SearchHint {
            id: item.id.clone(),
            name: item.name.clone(),
            item_type: item.item_type.clone(),
            year: item.year,
            production_year: item.year,
            index_number: item.index_number,
            parent_index_number: item.parent_index_number,
            primary_image_tag: Some("default".to_string()), // Placeholder
            thumb_image_tag: None,
            thumb_image_item_id: None,
            backdrop_image_tag: None,
            backdrop_image_item_id: None,
            series_name,
            series_id,
            runtime_ticks: item.runtime_ticks,
            media_type,
            is_folder,
            run_time_ticks: item.runtime_ticks,
            channel_id: None,
            channel_name: None,
        });
    }

    Ok(Json(SearchHintsResponse {
        total_record_count: hints.len() as i32,
        search_hints: hints,
    }))
}

// ============================================================================
// Search helper functions
// ============================================================================

/// Search using FTS5 (faster and better ranking)
async fn search_with_fts(
    pool: &sqlx::SqlitePool,
    search_term: &str,
    query: &SearchHintsQuery,
    limit: i32,
) -> Result<Vec<MediaItem>, sqlx::Error> {
    // Prepare FTS query
    let fts_query = prepare_fts_query(search_term);

    if fts_query.is_empty() {
        return Ok(vec![]);
    }

    // Parse item type filters
    let include_types: Option<Vec<&str>> = query
        .include_item_types
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim()).collect());
    let exclude_types: Option<Vec<&str>> = query
        .exclude_item_types
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim()).collect());

    // Build query with QueryBuilder for safe parameter binding
    let mut qb: sqlx::QueryBuilder<sqlx::Sqlite> = sqlx::QueryBuilder::new(
        r#"SELECT m.*
        FROM media_items m
        JOIN media_items_fts f ON m.rowid = f.rowid
        WHERE media_items_fts MATCH "#,
    );

    qb.push_bind(fts_query);

    // Include type filter
    if let Some(ref types) = include_types {
        qb.push(" AND m.item_type IN (");
        let mut separated = qb.separated(", ");
        for t in types {
            separated.push_bind(t.to_string());
        }
        separated.push_unseparated(")");
    }

    // Exclude type filter
    if let Some(ref types) = exclude_types {
        qb.push(" AND m.item_type NOT IN (");
        let mut separated = qb.separated(", ");
        for t in types {
            separated.push_bind(t.to_string());
        }
        separated.push_unseparated(")");
    }

    qb.push(" ORDER BY bm25(media_items_fts) LIMIT ")
        .push_bind(limit);

    qb.build_query_as().fetch_all(pool).await
}

/// Fallback search using LIKE (slower but always works)
async fn search_with_like(
    pool: &sqlx::SqlitePool,
    search_term: &str,
    query: &SearchHintsQuery,
    limit: i32,
) -> Result<Vec<MediaItem>, (StatusCode, String)> {
    let search_lower = search_term.to_lowercase();
    let search_pattern = format!("%{}%", search_lower);
    let prefix_pattern = format!("{}%", search_lower);

    // Parse item type filters
    let include_types: Option<Vec<&str>> = query
        .include_item_types
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim()).collect());
    let exclude_types: Option<Vec<&str>> = query
        .exclude_item_types
        .as_ref()
        .map(|t| t.split(',').map(|s| s.trim()).collect());

    // Build query with QueryBuilder
    let mut qb: sqlx::QueryBuilder<sqlx::Sqlite> =
        sqlx::QueryBuilder::new("SELECT * FROM media_items WHERE (LOWER(name) LIKE ");

    qb.push_bind(search_pattern.clone())
        .push(" OR LOWER(COALESCE(overview, '')) LIKE ")
        .push_bind(search_pattern)
        .push(")");

    // Include type filter
    if let Some(ref types) = include_types {
        qb.push(" AND item_type IN (");
        let mut separated = qb.separated(", ");
        for t in types {
            separated.push_bind(t.to_string());
        }
        separated.push_unseparated(")");
    }

    // Exclude type filter
    if let Some(ref types) = exclude_types {
        qb.push(" AND item_type NOT IN (");
        let mut separated = qb.separated(", ");
        for t in types {
            separated.push_bind(t.to_string());
        }
        separated.push_unseparated(")");
    }

    // Order by relevance: exact matches first, then prefix matches, then contains
    qb.push(" ORDER BY CASE WHEN LOWER(name) = ")
        .push_bind(search_lower.clone())
        .push(" THEN 0 WHEN LOWER(name) LIKE ")
        .push_bind(prefix_pattern)
        .push(" THEN 1 ELSE 2 END, name LIMIT ")
        .push_bind(limit);

    qb.build_query_as()
        .fetch_all(pool)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))
}

/// Prepare a user query for FTS5
fn prepare_fts_query(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|s| !s.is_empty() && s.len() >= 2)
        .map(|s| {
            // Escape special FTS5 characters and add prefix matching
            let escaped = s.replace(['"', '\'', '*'], "").replace('-', " ");
            format!("\"{}\"*", escaped)
        })
        .collect();

    terms.join(" OR ")
}

// =============================================================================
// Item Refresh - Re-fetch metadata for an item
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshQuery {
    /// Jellyfin standard: metadataRefreshMode (Default, FullRefresh, ValidationOnly)
    pub metadata_refresh_mode: Option<String>,
    /// Jellyfin standard: imageRefreshMode (Default, FullRefresh, ValidationOnly)
    pub image_refresh_mode: Option<String>,
    /// Replace all metadata (default: false - only fill missing fields)
    pub replace_all_metadata: Option<bool>,
    /// Replace all images (default: false)
    pub replace_all_images: Option<bool>,
    /// Regenerate trickplay (ignored for now)
    pub regenerate_trickplay: Option<bool>,
}

/// POST /Items/:id/Refresh - Trigger metadata refresh for an item or library
async fn refresh_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<RefreshQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Parse the refresh mode
    // Default = scan for new files only (quick scan)
    // ValidationOnly = search for missing metadata (fill gaps)
    // FullRefresh = replace all metadata
    let metadata_mode = query.metadata_refresh_mode.as_deref().unwrap_or("Default");

    let is_default_mode = metadata_mode == "Default";
    let is_validation_mode = metadata_mode == "ValidationOnly";
    let is_full_refresh = metadata_mode == "FullRefresh";

    // Determine if we should replace existing data
    let replace_all = query.replace_all_metadata.unwrap_or(false) || is_full_refresh;
    let replace_images = query.replace_all_images.unwrap_or(false)
        || query
            .image_refresh_mode
            .as_ref()
            .map(|m| m == "FullRefresh")
            .unwrap_or(false);

    // First, check if this is a library ID
    let library: Option<crate::models::Library> =
        sqlx::query_as("SELECT * FROM libraries WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(lib) = library {
        let db = state.db.clone();
        let config = state.config.clone();

        if is_default_mode {
            // Default mode: Quick scan - only find new/updated files
            tracing::info!(
                "Quick scan requested for library '{}' ({})",
                lib.name,
                lib.id
            );
            tokio::spawn(async move {
                match crate::scanner::quick_scan_library(
                    &db,
                    &lib.id,
                    &lib.path,
                    &lib.library_type,
                    config.paths.cache_dir,
                )
                .await
                {
                    Ok(result) => {
                        if result.files_added > 0 || result.files_removed > 0 {
                            tracing::info!(
                                "Quick scan for '{}': {} added, {} removed",
                                lib.name,
                                result.files_added,
                                result.files_removed
                            );
                        } else {
                            tracing::info!("Quick scan for '{}': no changes", lib.name);
                        }
                    }
                    Err(e) => tracing::error!("Quick scan failed for {}: {}", lib.id, e),
                }
            });
        } else {
            // ValidationOnly or FullRefresh: Full library scan with metadata
            tracing::info!(
                "Full refresh requested for library '{}' ({}) [mode={}]",
                lib.name,
                lib.id,
                metadata_mode
            );
            tokio::spawn(async move {
                if let Err(e) = crate::scanner::scan_library_with_cache_dir(
                    &db,
                    &lib.id,
                    &lib.path,
                    &lib.library_type,
                    config.paths.cache_dir,
                    Some(config.anime_db_enabled),
                )
                .await
                {
                    tracing::error!("Failed to refresh library {}: {}", lib.id, e);
                } else {
                    tracing::info!("Library refresh completed for '{}'", lib.name);
                }
            });
        }

        return Ok(StatusCode::NO_CONTENT);
    }

    // Otherwise, check if it's a media item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // For Default mode on items, there's nothing to scan - just return success
    if is_default_mode {
        tracing::debug!(
            "Default refresh mode for item '{}' - no action needed",
            item.name
        );
        return Ok(StatusCode::NO_CONTENT);
    }

    // ValidationOnly or FullRefresh: fetch metadata
    // ValidationOnly = only fill missing fields (replace_all = false)
    // FullRefresh = replace everything (replace_all = true)
    let should_replace = if is_validation_mode {
        false
    } else {
        replace_all
    };

    // Spawn a background task to refresh metadata
    let db = state.db.clone();
    let config = state.config.clone();
    tokio::spawn(async move {
        if let Err(e) =
            refresh_item_metadata(&db, &config, &item, should_replace, replace_images).await
        {
            tracing::error!("Failed to refresh metadata for item {}: {}", id, e);
        }
    });

    // Return 204 No Content immediately (refresh happens in background)
    Ok(StatusCode::NO_CONTENT)
}

/// Internal function to refresh metadata for an item
async fn refresh_item_metadata(
    db: &sqlx::SqlitePool,
    config: &crate::config::AppConfig,
    item: &MediaItem,
    replace_all: bool,
    replace_images: bool,
) -> anyhow::Result<()> {
    use super::filters::{
        get_or_create_genre, get_or_create_person, get_or_create_studio, link_item_genre,
        link_item_person, link_item_studio,
    };
    use crate::services::metadata::MetadataService;

    let cache_dir = config.paths.cache_dir.join("images");
    let metadata_service = MetadataService::from_env(cache_dir, None);

    tracing::info!(
        "Refreshing metadata for {} '{}' (replace_all={})",
        item.item_type,
        item.name,
        replace_all
    );

    match item.item_type.as_str() {
        "Series" => {
            // Try to fetch metadata using the series name
            let is_anime = MetadataService::is_likely_anime(&item.name);
            let metadata = if is_anime {
                metadata_service
                    .get_anime_metadata(&item.name, item.year)
                    .await?
            } else {
                metadata_service
                    .get_series_metadata(&item.name, item.year)
                    .await?
            };

            if let Some(meta) = metadata {
                tracing::info!(
                    "Found metadata via {} for series: {} -> {}",
                    meta.provider,
                    item.name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );

                // Update the series with new metadata
                if replace_all {
                    sqlx::query(
                        r#"UPDATE media_items SET 
                           name = COALESCE(?, name),
                           overview = ?,
                           year = COALESCE(?, year),
                           premiere_date = ?,
                           community_rating = ?,
                           anilist_id = COALESCE(?, anilist_id),
                           mal_id = COALESCE(?, mal_id),
                           anidb_id = COALESCE(?, anidb_id),
                           kitsu_id = COALESCE(?, kitsu_id),
                           tmdb_id = COALESCE(?, tmdb_id),
                           imdb_id = COALESCE(?, imdb_id)
                           WHERE id = ?"#,
                    )
                    .bind(meta.name.as_deref())
                    .bind(meta.overview.as_deref())
                    .bind(meta.year)
                    .bind(meta.premiere_date.as_deref())
                    .bind(meta.community_rating)
                    .bind(meta.anilist_id.as_deref())
                    .bind(meta.mal_id.as_deref())
                    .bind(meta.anidb_id.as_deref())
                    .bind(meta.kitsu_id.as_deref())
                    .bind(meta.tmdb_id.as_deref())
                    .bind(meta.imdb_id.as_deref())
                    .bind(&item.id)
                    .execute(db)
                    .await?;
                } else {
                    // Only fill missing fields
                    sqlx::query(
                        r#"UPDATE media_items SET 
                           overview = COALESCE(overview, ?),
                           year = COALESCE(year, ?),
                           premiere_date = COALESCE(premiere_date, ?),
                           community_rating = COALESCE(community_rating, ?),
                           anilist_id = COALESCE(anilist_id, ?),
                           mal_id = COALESCE(mal_id, ?),
                           anidb_id = COALESCE(anidb_id, ?),
                           kitsu_id = COALESCE(kitsu_id, ?),
                           tmdb_id = COALESCE(tmdb_id, ?),
                           imdb_id = COALESCE(imdb_id, ?)
                           WHERE id = ?"#,
                    )
                    .bind(meta.overview.as_deref())
                    .bind(meta.year)
                    .bind(meta.premiere_date.as_deref())
                    .bind(meta.community_rating)
                    .bind(meta.anilist_id.as_deref())
                    .bind(meta.mal_id.as_deref())
                    .bind(meta.anidb_id.as_deref())
                    .bind(meta.kitsu_id.as_deref())
                    .bind(meta.tmdb_id.as_deref())
                    .bind(meta.imdb_id.as_deref())
                    .bind(&item.id)
                    .execute(db)
                    .await?;
                }

                // Queue images
                if replace_images {
                    // Delete existing images first
                    sqlx::query("DELETE FROM images WHERE item_id = ?")
                        .bind(&item.id)
                        .execute(db)
                        .await?;
                }

                if let Some(ref url) = meta.poster_url {
                    crate::db::queue_image(db, &item.id, "Primary", url).await?;
                }
                if let Some(ref url) = meta.backdrop_url {
                    crate::db::queue_image(db, &item.id, "Backdrop", url).await?;
                }

                // Update genres
                if let Some(ref genres) = meta.genres {
                    // Clear existing genres if replacing
                    if replace_all {
                        sqlx::query("DELETE FROM item_genres WHERE item_id = ?")
                            .bind(&item.id)
                            .execute(db)
                            .await?;
                    }
                    for genre_name in genres {
                        if let Ok(genre_id) = get_or_create_genre(db, genre_name).await {
                            let _ = link_item_genre(db, &item.id, &genre_id).await;
                        }
                    }
                }

                // Update studio
                if let Some(ref studio_name) = meta.studio {
                    if replace_all {
                        sqlx::query("DELETE FROM item_studios WHERE item_id = ?")
                            .bind(&item.id)
                            .execute(db)
                            .await?;
                    }
                    if let Ok(studio_id) = get_or_create_studio(db, studio_name).await {
                        let _ = link_item_studio(db, &item.id, &studio_id).await;
                    }
                }

                // Update cast
                if !meta.cast.is_empty() {
                    if replace_all {
                        sqlx::query("DELETE FROM item_persons WHERE item_id = ?")
                            .bind(&item.id)
                            .execute(db)
                            .await?;
                    }
                    for (i, cast_member) in meta.cast.iter().enumerate() {
                        if let Ok(person_id) = get_or_create_person(db, cast_member).await {
                            let _ = link_item_person(
                                db,
                                &item.id,
                                &person_id,
                                cast_member.character_name.as_deref(),
                                i as i32,
                            )
                            .await;
                        }
                    }
                }

                tracing::info!("Successfully refreshed metadata for series '{}'", item.name);
            } else {
                tracing::warn!("No metadata found for series '{}'", item.name);
            }
        }
        "Movie" => {
            let metadata = metadata_service
                .get_movie_metadata(&item.name, item.year)
                .await?;

            if let Some(meta) = metadata {
                tracing::info!(
                    "Found metadata via {} for movie: {} -> {}",
                    meta.provider,
                    item.name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );

                // Update the movie
                if replace_all {
                    sqlx::query(
                        r#"UPDATE media_items SET 
                           name = COALESCE(?, name),
                           overview = ?,
                           year = COALESCE(?, year),
                           premiere_date = ?,
                           community_rating = ?,
                           tmdb_id = COALESCE(?, tmdb_id),
                           imdb_id = COALESCE(?, imdb_id)
                           WHERE id = ?"#,
                    )
                    .bind(meta.name.as_deref())
                    .bind(meta.overview.as_deref())
                    .bind(meta.year)
                    .bind(meta.premiere_date.as_deref())
                    .bind(meta.community_rating)
                    .bind(meta.tmdb_id.as_deref())
                    .bind(meta.imdb_id.as_deref())
                    .bind(&item.id)
                    .execute(db)
                    .await?;
                } else {
                    sqlx::query(
                        r#"UPDATE media_items SET 
                           overview = COALESCE(overview, ?),
                           year = COALESCE(year, ?),
                           premiere_date = COALESCE(premiere_date, ?),
                           community_rating = COALESCE(community_rating, ?),
                           tmdb_id = COALESCE(tmdb_id, ?),
                           imdb_id = COALESCE(imdb_id, ?)
                           WHERE id = ?"#,
                    )
                    .bind(meta.overview.as_deref())
                    .bind(meta.year)
                    .bind(meta.premiere_date.as_deref())
                    .bind(meta.community_rating)
                    .bind(meta.tmdb_id.as_deref())
                    .bind(meta.imdb_id.as_deref())
                    .bind(&item.id)
                    .execute(db)
                    .await?;
                }

                // Queue images
                if replace_images {
                    sqlx::query("DELETE FROM images WHERE item_id = ?")
                        .bind(&item.id)
                        .execute(db)
                        .await?;
                }

                if let Some(ref url) = meta.poster_url {
                    crate::db::queue_image(db, &item.id, "Primary", url).await?;
                }
                if let Some(ref url) = meta.backdrop_url {
                    crate::db::queue_image(db, &item.id, "Backdrop", url).await?;
                }

                // Update genres
                if let Some(ref genres) = meta.genres {
                    if replace_all {
                        sqlx::query("DELETE FROM item_genres WHERE item_id = ?")
                            .bind(&item.id)
                            .execute(db)
                            .await?;
                    }
                    for genre_name in genres {
                        if let Ok(genre_id) = get_or_create_genre(db, genre_name).await {
                            let _ = link_item_genre(db, &item.id, &genre_id).await;
                        }
                    }
                }

                tracing::info!("Successfully refreshed metadata for movie '{}'", item.name);
            } else {
                tracing::warn!("No metadata found for movie '{}'", item.name);
            }
        }
        _ => {
            tracing::debug!("Refresh not supported for item type: {}", item.item_type);
        }
    }

    Ok(())
}

// =============================================================================
// Item Download
// =============================================================================

/// GET /Items/:id/Download - Download the media file for an item
async fn download_item(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Response, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the media item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
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

    // Get content type based on extension
    let content_type = get_content_type_for_download(file_path);

    // Get filename for Content-Disposition header
    let filename = std::path::Path::new(file_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download");

    // Stream the file as a download
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, file_size)
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", filename),
        )
        .header(header::CACHE_CONTROL, "no-cache")
        .body(body)
        .unwrap())
}

/// Get MIME type for download based on file extension
fn get_content_type_for_download(path: &str) -> &'static str {
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
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

// =============================================================================
// Remote Images - Search for alternative artwork from providers
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteImageInfo {
    pub provider_name: String,
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumbnail_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub community_rating: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vote_count: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(rename = "Type")]
    pub image_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rating_type: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteImageResult {
    pub images: Vec<RemoteImageInfo>,
    pub total_record_count: i32,
    pub providers: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteImagesQuery {
    #[serde(rename = "type")]
    pub image_type: Option<String>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub include_all_languages: Option<bool>,
}

/// GET /Items/:id/RemoteImages - Get available remote images for an item
async fn get_remote_images(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<RemoteImagesQuery>,
) -> Result<Json<RemoteImageResult>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the item to find its provider IDs
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    let mut images = Vec::new();
    let mut providers = Vec::new();

    // Get images from TMDB if we have a TMDB ID and API key
    if let Some(ref tmdb_id) = item.tmdb_id {
        if let (Ok(tmdb_id_num), Ok(api_key)) =
            (tmdb_id.parse::<i64>(), std::env::var("TMDB_API_KEY"))
        {
            providers.push("TheMovieDb".to_string());

            // Fetch images from TMDB directly
            let endpoint = if item.item_type == "Movie" {
                format!(
                    "https://api.themoviedb.org/3/movie/{}/images?api_key={}",
                    tmdb_id_num, api_key
                )
            } else {
                format!(
                    "https://api.themoviedb.org/3/tv/{}/images?api_key={}",
                    tmdb_id_num, api_key
                )
            };

            let client = reqwest::Client::new();
            if let Ok(resp) = client.get(&endpoint).send().await {
                if let Ok(response) = resp.json::<serde_json::Value>().await {
                    // Parse posters
                    if let Some(posters) = response.get("posters").and_then(|p| p.as_array()) {
                        for poster in posters.iter().take(10) {
                            if let Some(file_path) =
                                poster.get("file_path").and_then(|f| f.as_str())
                            {
                                let should_include = query.image_type.is_none()
                                    || query.image_type.as_deref() == Some("Primary");

                                if should_include {
                                    images.push(RemoteImageInfo {
                                        provider_name: "TheMovieDb".to_string(),
                                        url: format!(
                                            "https://image.tmdb.org/t/p/original{}",
                                            file_path
                                        ),
                                        thumbnail_url: Some(format!(
                                            "https://image.tmdb.org/t/p/w300{}",
                                            file_path
                                        )),
                                        height: poster
                                            .get("height")
                                            .and_then(|h| h.as_i64())
                                            .map(|h| h as i32),
                                        width: poster
                                            .get("width")
                                            .and_then(|w| w.as_i64())
                                            .map(|w| w as i32),
                                        community_rating: poster
                                            .get("vote_average")
                                            .and_then(|v| v.as_f64()),
                                        vote_count: poster
                                            .get("vote_count")
                                            .and_then(|v| v.as_i64())
                                            .map(|v| v as i32),
                                        language: poster
                                            .get("iso_639_1")
                                            .and_then(|l| l.as_str())
                                            .map(|s| s.to_string()),
                                        image_type: "Primary".to_string(),
                                        rating_type: Some("Score".to_string()),
                                    });
                                }
                            }
                        }
                    }

                    // Parse backdrops
                    if let Some(backdrops) = response.get("backdrops").and_then(|b| b.as_array()) {
                        for backdrop in backdrops.iter().take(10) {
                            if let Some(file_path) =
                                backdrop.get("file_path").and_then(|f| f.as_str())
                            {
                                let should_include = query.image_type.is_none()
                                    || query.image_type.as_deref() == Some("Backdrop");

                                if should_include {
                                    images.push(RemoteImageInfo {
                                        provider_name: "TheMovieDb".to_string(),
                                        url: format!(
                                            "https://image.tmdb.org/t/p/original{}",
                                            file_path
                                        ),
                                        thumbnail_url: Some(format!(
                                            "https://image.tmdb.org/t/p/w780{}",
                                            file_path
                                        )),
                                        height: backdrop
                                            .get("height")
                                            .and_then(|h| h.as_i64())
                                            .map(|h| h as i32),
                                        width: backdrop
                                            .get("width")
                                            .and_then(|w| w.as_i64())
                                            .map(|w| w as i32),
                                        community_rating: backdrop
                                            .get("vote_average")
                                            .and_then(|v| v.as_f64()),
                                        vote_count: backdrop
                                            .get("vote_count")
                                            .and_then(|v| v.as_i64())
                                            .map(|v| v as i32),
                                        language: backdrop
                                            .get("iso_639_1")
                                            .and_then(|l| l.as_str())
                                            .map(|s| s.to_string()),
                                        image_type: "Backdrop".to_string(),
                                        rating_type: Some("Score".to_string()),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Get images from AniList if we have an AniList ID
    if let Some(ref anilist_id) = item.anilist_id {
        if let Ok(anilist_id_num) = anilist_id.parse::<i64>() {
            providers.push("AniList".to_string());

            let cache_dir = state.config.paths.cache_dir.join("images");
            let anilist = crate::services::anilist::AniListClient::new(cache_dir);
            if let Ok(Some(anime)) = anilist.get_anime_by_id(anilist_id_num).await {
                // Cover image (Primary)
                if let Some(ref cover) = anime.poster_url {
                    let should_include = query.image_type.is_none()
                        || query.image_type.as_deref() == Some("Primary");

                    if should_include {
                        images.push(RemoteImageInfo {
                            provider_name: "AniList".to_string(),
                            url: cover.clone(),
                            thumbnail_url: Some(cover.clone()),
                            height: None,
                            width: None,
                            community_rating: anime.community_rating,
                            vote_count: None,
                            language: Some("ja".to_string()),
                            image_type: "Primary".to_string(),
                            rating_type: Some("Score".to_string()),
                        });
                    }
                }

                // Banner image (Backdrop)
                if let Some(ref banner) = anime.backdrop_url {
                    let should_include = query.image_type.is_none()
                        || query.image_type.as_deref() == Some("Backdrop");

                    if should_include {
                        images.push(RemoteImageInfo {
                            provider_name: "AniList".to_string(),
                            url: banner.clone(),
                            thumbnail_url: Some(banner.clone()),
                            height: None,
                            width: None,
                            community_rating: anime.community_rating,
                            vote_count: None,
                            language: Some("ja".to_string()),
                            image_type: "Backdrop".to_string(),
                            rating_type: Some("Score".to_string()),
                        });
                    }
                }
            }
        }
    }

    let total = images.len() as i32;

    Ok(Json(RemoteImageResult {
        images,
        total_record_count: total,
        providers,
    }))
}

// =============================================================================
// Download Remote Image
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadRemoteImageQuery {
    #[serde(rename = "type")]
    pub image_type: String,
    pub image_url: Option<String>,
}

/// POST /Items/:id/RemoteImages/Download - Download and save a remote image
async fn download_remote_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<DownloadRemoteImageQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the item to verify it exists
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    let image_url = query.image_url.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            "imageUrl query parameter is required".to_string(),
        )
    })?;

    // Determine the image type (Primary, Backdrop, etc.)
    let image_type = &query.image_type;

    // Create cache directory for images
    let cache_dir = state.config.paths.cache_dir.join("images").join(&id);
    tokio::fs::create_dir_all(&cache_dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Determine file extension from URL or default to jpg
    let extension = image_url
        .rsplit('.')
        .next()
        .filter(|ext| ["jpg", "jpeg", "png", "webp", "gif"].contains(&ext.to_lowercase().as_str()))
        .unwrap_or("jpg");

    let filename = format!("{}.{}", image_type.to_lowercase(), extension);
    let file_path = cache_dir.join(&filename);

    // Download the image
    tracing::info!("Downloading {} image for item {} from {}", image_type, id, image_url);

    let client = reqwest::Client::new();
    let response = client
        .get(&image_url)
        .send()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Failed to download image: {}", e)))?;

    if !response.status().is_success() {
        return Err((
            StatusCode::BAD_GATEWAY,
            format!("Image download failed with status: {}", response.status()),
        ));
    }

    let bytes = response
        .bytes()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Failed to read image data: {}", e)))?;

    // Save the image file
    tokio::fs::write(&file_path, &bytes)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("Failed to save image: {}", e)))?;

    // Store image reference in database
    let image_id = uuid::Uuid::new_v4().to_string();
    let file_path_str = file_path.to_string_lossy().to_string();

    sqlx::query(
        "INSERT OR REPLACE INTO images (id, item_id, image_type, path) VALUES (?, ?, ?, ?)",
    )
    .bind(&image_id)
    .bind(&id)
    .bind(image_type)
    .bind(&file_path_str)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!(
        "Downloaded and saved {} image for item {} to {}",
        image_type,
        id,
        file_path_str
    );

    Ok(StatusCode::NO_CONTENT)
}

// =============================================================================
// External ID Infos - Show available external IDs for an item
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExternalIdInfo {
    pub name: String,
    pub key: String,
    #[serde(rename = "Type")]
    pub id_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url_format_string: Option<String>,
}

/// GET /Items/:id/ExternalIdInfos - Get external ID info for an item type
async fn get_external_id_infos(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<Vec<ExternalIdInfo>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the item to determine its type
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    let mut infos = Vec::new();

    // Common IDs for all types
    infos.push(ExternalIdInfo {
        name: "IMDb".to_string(),
        key: "Imdb".to_string(),
        id_type: "Series".to_string(),
        url_format_string: Some("https://www.imdb.com/title/{0}".to_string()),
    });

    infos.push(ExternalIdInfo {
        name: "TheMovieDb".to_string(),
        key: "Tmdb".to_string(),
        id_type: "Series".to_string(),
        url_format_string: if item.item_type == "Movie" {
            Some("https://www.themoviedb.org/movie/{0}".to_string())
        } else {
            Some("https://www.themoviedb.org/tv/{0}".to_string())
        },
    });

    // Anime-specific IDs
    if item.item_type == "Series" || item.item_type == "Movie" {
        infos.push(ExternalIdInfo {
            name: "AniList".to_string(),
            key: "AniList".to_string(),
            id_type: "Series".to_string(),
            url_format_string: Some("https://anilist.co/anime/{0}".to_string()),
        });

        infos.push(ExternalIdInfo {
            name: "MyAnimeList".to_string(),
            key: "MyAnimeList".to_string(),
            id_type: "Series".to_string(),
            url_format_string: Some("https://myanimelist.net/anime/{0}".to_string()),
        });

        infos.push(ExternalIdInfo {
            name: "AniDB".to_string(),
            key: "AniDb".to_string(),
            id_type: "Series".to_string(),
            url_format_string: Some("https://anidb.net/anime/{0}".to_string()),
        });
    }

    Ok(Json(infos))
}

// =============================================================================
// Metadata Editor - Get metadata editor info
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ParentalRating {
    pub name: String,
    pub value: i32,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CountryInfo {
    pub name: String,
    pub display_name: String,
    pub two_letter_iso_region_name: String,
    pub three_letter_iso_region_name: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CultureDto {
    pub name: String,
    pub display_name: String,
    pub two_letter_iso_language_name: String,
    pub three_letter_iso_language_name: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct NameValuePair {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataEditorInfo {
    pub parental_rating_options: Vec<ParentalRating>,
    pub countries: Vec<CountryInfo>,
    pub cultures: Vec<CultureDto>,
    pub external_id_infos: Vec<ExternalIdInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    pub content_type_options: Vec<NameValuePair>,
}

/// GET /Items/:id/MetadataEditor - Get metadata editor configuration
async fn get_metadata_editor(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<MetadataEditorInfo>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get external ID infos for this item
    let external_ids_result =
        get_external_id_infos(State(state.clone()), headers.clone(), Path(id.clone())).await?;

    let info = MetadataEditorInfo {
        parental_rating_options: vec![
            ParentalRating {
                name: "G".to_string(),
                value: 1,
            },
            ParentalRating {
                name: "PG".to_string(),
                value: 5,
            },
            ParentalRating {
                name: "PG-13".to_string(),
                value: 7,
            },
            ParentalRating {
                name: "R".to_string(),
                value: 9,
            },
            ParentalRating {
                name: "NC-17".to_string(),
                value: 10,
            },
            ParentalRating {
                name: "TV-Y".to_string(),
                value: 1,
            },
            ParentalRating {
                name: "TV-Y7".to_string(),
                value: 3,
            },
            ParentalRating {
                name: "TV-G".to_string(),
                value: 1,
            },
            ParentalRating {
                name: "TV-PG".to_string(),
                value: 5,
            },
            ParentalRating {
                name: "TV-14".to_string(),
                value: 7,
            },
            ParentalRating {
                name: "TV-MA".to_string(),
                value: 9,
            },
        ],
        countries: vec![
            CountryInfo {
                name: "United States".to_string(),
                display_name: "United States".to_string(),
                two_letter_iso_region_name: "US".to_string(),
                three_letter_iso_region_name: "USA".to_string(),
            },
            CountryInfo {
                name: "Japan".to_string(),
                display_name: "Japan".to_string(),
                two_letter_iso_region_name: "JP".to_string(),
                three_letter_iso_region_name: "JPN".to_string(),
            },
            CountryInfo {
                name: "United Kingdom".to_string(),
                display_name: "United Kingdom".to_string(),
                two_letter_iso_region_name: "GB".to_string(),
                three_letter_iso_region_name: "GBR".to_string(),
            },
        ],
        cultures: vec![
            CultureDto {
                name: "en-US".to_string(),
                display_name: "English (United States)".to_string(),
                two_letter_iso_language_name: "en".to_string(),
                three_letter_iso_language_name: Some("eng".to_string()),
            },
            CultureDto {
                name: "ja-JP".to_string(),
                display_name: "Japanese (Japan)".to_string(),
                two_letter_iso_language_name: "ja".to_string(),
                three_letter_iso_language_name: Some("jpn".to_string()),
            },
        ],
        external_id_infos: external_ids_result.0,
        content_type: None,
        content_type_options: vec![
            NameValuePair {
                name: "Movies".to_string(),
                value: "movies".to_string(),
            },
            NameValuePair {
                name: "TV Shows".to_string(),
                value: "tvshows".to_string(),
            },
            NameValuePair {
                name: "Mixed Content".to_string(),
                value: "mixed".to_string(),
            },
        ],
    };

    Ok(Json(info))
}

// =============================================================================
// Remote Search - Search for series/movies to identify
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesInfoRemoteSearchQuery {
    #[serde(default)]
    pub search_info: Option<SeriesSearchInfo>,
    pub item_id: Option<String>,
    #[serde(default)]
    pub include_disabled_providers: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeriesSearchInfo {
    pub name: Option<String>,
    pub year: Option<i32>,
    pub provider_ids: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MovieInfoRemoteSearchQuery {
    #[serde(default)]
    pub search_info: Option<MovieSearchInfo>,
    pub item_id: Option<String>,
    #[serde(default)]
    pub include_disabled_providers: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MovieSearchInfo {
    pub name: Option<String>,
    pub year: Option<i32>,
    pub provider_ids: Option<std::collections::HashMap<String, String>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteSearchResult {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<std::collections::HashMap<String, String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub production_year: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub index_number_end: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_index_number: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub premiere_date: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_url: Option<String>,
    pub search_provider_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub overview: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_artist: Option<AlbumArtist>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artists: Option<Vec<AlbumArtist>>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AlbumArtist {
    pub name: String,
    pub id: Option<String>,
}

/// POST /Items/RemoteSearch/Series - Search for series metadata
async fn remote_search_series(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(query): Json<SeriesInfoRemoteSearchQuery>,
) -> Result<Json<Vec<RemoteSearchResult>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let mut results = Vec::new();

    // Get search parameters
    let (search_name, search_year) = if let Some(ref info) = query.search_info {
        (info.name.clone(), info.year)
    } else if let Some(ref item_id) = query.item_id {
        // Get name from existing item
        let item: Option<MediaItem> = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if let Some(item) = item {
            (Some(item.name), item.year)
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let search_name =
        search_name.ok_or_else(|| (StatusCode::BAD_REQUEST, "Search name required".to_string()))?;

    // Search AniList
    let cache_dir = state.config.paths.cache_dir.join("images");
    let anilist = crate::services::anilist::AniListClient::new(cache_dir);
    if let Ok(anime_results) = anilist.search_anime(&search_name, search_year).await {
        for anime in anime_results.into_iter().take(10) {
            let mut provider_ids = std::collections::HashMap::new();
            provider_ids.insert("AniList".to_string(), anime.id.to_string());
            if let Some(mal_id) = anime.id_mal {
                provider_ids.insert("MyAnimeList".to_string(), mal_id.to_string());
            }

            let title = anime
                .title
                .as_ref()
                .and_then(|t| t.romaji.clone().or_else(|| t.english.clone()))
                .unwrap_or_default();

            let premiere_date = anime.start_date.as_ref().map(|d| {
                format!(
                    "{:04}-{:02}-{:02}",
                    d.year.unwrap_or(0),
                    d.month.unwrap_or(1),
                    d.day.unwrap_or(1)
                )
            });

            let cover_url = anime
                .cover_image
                .as_ref()
                .and_then(|c| c.large.clone().or_else(|| c.medium.clone()));

            results.push(RemoteSearchResult {
                name: title,
                provider_ids: Some(provider_ids),
                production_year: anime.season_year,
                index_number: None,
                index_number_end: None,
                parent_index_number: None,
                premiere_date,
                image_url: cover_url,
                search_provider_name: "AniList".to_string(),
                overview: anime.description,
                album_artist: None,
                artists: None,
            });
        }
    }

    // Search TMDB
    let tmdb_cache_dir = state.config.paths.cache_dir.join("images");
    if let Some(tmdb) = crate::services::tmdb::TmdbClient::from_env(tmdb_cache_dir) {
        if let Ok(tv_results) = tmdb.search_tv(&search_name, search_year).await {
            for tv in tv_results.into_iter().take(10) {
                let mut provider_ids = std::collections::HashMap::new();
                provider_ids.insert("Tmdb".to_string(), tv.id.to_string());

                let year = tv
                    .first_air_date
                    .as_ref()
                    .and_then(|d| d.split('-').next())
                    .and_then(|y| y.parse().ok());

                results.push(RemoteSearchResult {
                    name: tv.name.clone(),
                    provider_ids: Some(provider_ids),
                    production_year: year,
                    index_number: None,
                    index_number_end: None,
                    parent_index_number: None,
                    premiere_date: tv.first_air_date.clone(),
                    image_url: tv
                        .poster_path
                        .map(|p| format!("https://image.tmdb.org/t/p/w300{}", p)),
                    search_provider_name: "TheMovieDb".to_string(),
                    overview: tv.overview,
                    album_artist: None,
                    artists: None,
                });
            }
        }
    }

    Ok(Json(results))
}

/// POST /Items/RemoteSearch/Movie - Search for movie metadata
async fn remote_search_movie(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(query): Json<MovieInfoRemoteSearchQuery>,
) -> Result<Json<Vec<RemoteSearchResult>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let mut results = Vec::new();

    // Get search parameters
    let (search_name, search_year) = if let Some(ref info) = query.search_info {
        (info.name.clone(), info.year)
    } else if let Some(ref item_id) = query.item_id {
        let item: Option<MediaItem> = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        if let Some(item) = item {
            (Some(item.name), item.year)
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    let search_name =
        search_name.ok_or_else(|| (StatusCode::BAD_REQUEST, "Search name required".to_string()))?;

    // Search TMDB for movies
    let tmdb_cache_dir = state.config.paths.cache_dir.join("images");
    if let Some(tmdb) = crate::services::tmdb::TmdbClient::from_env(tmdb_cache_dir) {
        if let Ok(movie_results) = tmdb.search_movie(&search_name, search_year).await {
            for movie in movie_results.into_iter().take(15) {
                let mut provider_ids = std::collections::HashMap::new();
                provider_ids.insert("Tmdb".to_string(), movie.id.to_string());

                let year = movie
                    .release_date
                    .as_ref()
                    .and_then(|d| d.split('-').next())
                    .and_then(|y| y.parse().ok());

                results.push(RemoteSearchResult {
                    name: movie.title.clone(),
                    provider_ids: Some(provider_ids),
                    production_year: year,
                    index_number: None,
                    index_number_end: None,
                    parent_index_number: None,
                    premiere_date: movie.release_date.clone(),
                    image_url: movie
                        .poster_path
                        .map(|p| format!("https://image.tmdb.org/t/p/w300{}", p)),
                    search_provider_name: "TheMovieDb".to_string(),
                    overview: movie.overview,
                    album_artist: None,
                    artists: None,
                });
            }
        }
    }

    Ok(Json(results))
}

// =============================================================================
// Apply Remote Search - Apply metadata from search result
// =============================================================================

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ApplyRemoteSearchBody {
    pub name: Option<String>,
    pub provider_ids: Option<std::collections::HashMap<String, String>>,
    pub production_year: Option<i32>,
    pub premiere_date: Option<String>,
    pub image_url: Option<String>,
    pub search_provider_name: Option<String>,
    pub overview: Option<String>,
}

/// POST /Items/RemoteSearch/Apply/:id - Apply metadata from a search result
async fn apply_remote_search(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<ApplyRemoteSearchBody>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // Extract provider IDs
    let mut anilist_id: Option<String> = None;
    let mut mal_id: Option<String> = None;
    let mut anidb_id: Option<String> = None;
    let mut tmdb_id: Option<String> = None;
    let mut imdb_id: Option<String> = None;

    if let Some(ref ids) = body.provider_ids {
        anilist_id = ids.get("AniList").cloned();
        mal_id = ids.get("MyAnimeList").cloned();
        anidb_id = ids.get("AniDb").cloned();
        tmdb_id = ids.get("Tmdb").cloned();
        imdb_id = ids.get("Imdb").cloned();
    }

    // Update the item with new metadata
    sqlx::query(
        r#"UPDATE media_items SET 
            name = COALESCE(?, name),
            overview = COALESCE(?, overview),
            year = COALESCE(?, year),
            premiere_date = COALESCE(?, premiere_date),
            anilist_id = COALESCE(?, anilist_id),
            mal_id = COALESCE(?, mal_id),
            anidb_id = COALESCE(?, anidb_id),
            tmdb_id = COALESCE(?, tmdb_id),
            imdb_id = COALESCE(?, imdb_id)
        WHERE id = ?"#,
    )
    .bind(body.name.as_deref())
    .bind(body.overview.as_deref())
    .bind(body.production_year)
    .bind(body.premiere_date.as_deref())
    .bind(anilist_id.as_deref())
    .bind(mal_id.as_deref())
    .bind(anidb_id.as_deref())
    .bind(tmdb_id.as_deref())
    .bind(imdb_id.as_deref())
    .bind(&id)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Queue image download if provided
    if let Some(ref image_url) = body.image_url {
        let _ = crate::db::queue_image(&state.db, &id, "Primary", image_url).await;
    }

    tracing::info!(
        "Applied remote search metadata to '{}' (id={}) from {}",
        item.name,
        id,
        body.search_provider_name.as_deref().unwrap_or("unknown")
    );

    Ok(StatusCode::NO_CONTENT)
}
