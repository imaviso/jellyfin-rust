// Home screen endpoints - Latest items, Resume, NextUp

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode, Uri},
    routing::get,
    Json, Router,
};
use std::sync::Arc;

use crate::{models::MediaItem, services::auth, AppState};

use super::items::{get_user_item_data, BaseItemDto, ImageTags, ItemsResponse, UserItemDataDto};
use super::users::parse_emby_auth_header;

/// Routes for /Users/:userId/Items/Latest
pub fn user_latest_routes() -> Router<Arc<AppState>> {
    Router::new().route("/", get(get_latest_items))
}

/// Routes for /UserItems/Resume
pub fn resume_routes() -> Router<Arc<AppState>> {
    Router::new().route("/", get(get_resume_items))
}

/// Routes for /Shows/NextUp
pub fn next_up_routes() -> Router<Arc<AppState>> {
    Router::new().route("/", get(get_next_up))
}

/// Parse query string manually to handle repeated params like fields=X&fields=Y
fn parse_query_params(query: &str) -> std::collections::HashMap<String, Vec<String>> {
    let mut params: std::collections::HashMap<String, Vec<String>> =
        std::collections::HashMap::new();
    for part in query.split('&') {
        if let Some((key, value)) = part.split_once('=') {
            let key = urlencoding::decode(key)
                .unwrap_or_else(|_| key.into())
                .to_string();
            let value = urlencoding::decode(value)
                .unwrap_or_else(|_| value.into())
                .to_string();
            params.entry(key).or_default().push(value);
        }
    }
    params
}

/// Helper to get first value from params
fn get_param(params: &std::collections::HashMap<String, Vec<String>>, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.first().cloned())
}

/// Helper to get param as i32
fn get_param_i32(
    params: &std::collections::HashMap<String, Vec<String>>,
    key: &str,
) -> Option<i32> {
    get_param(params, key).and_then(|v| v.parse().ok())
}

#[derive(Debug, Default)]
pub struct LatestQuery {
    pub parent_id: Option<String>,
    pub limit: Option<i32>,
    pub fields: Vec<String>,
    pub image_type_limit: Option<i32>,
    pub enable_image_types: Vec<String>,
}

impl LatestQuery {
    fn from_uri(uri: &Uri) -> Self {
        let params = parse_query_params(uri.query().unwrap_or(""));
        Self {
            parent_id: get_param(&params, "parentId"),
            limit: get_param_i32(&params, "limit"),
            fields: params.get("fields").cloned().unwrap_or_default(),
            image_type_limit: get_param_i32(&params, "imageTypeLimit"),
            enable_image_types: params.get("enableImageTypes").cloned().unwrap_or_default(),
        }
    }
}

#[derive(Debug, Default)]
pub struct ResumeQuery {
    pub user_id: Option<String>,
    pub limit: Option<i32>,
    pub parent_id: Option<String>,
    pub fields: Vec<String>,
    pub media_types: Option<String>,
    pub enable_image_types: Vec<String>,
    pub enable_total_record_count: Option<bool>,
    pub enable_user_data: Option<bool>,
}

impl ResumeQuery {
    fn from_uri(uri: &Uri) -> Self {
        let params = parse_query_params(uri.query().unwrap_or(""));
        Self {
            user_id: get_param(&params, "userId"),
            limit: get_param_i32(&params, "limit"),
            parent_id: get_param(&params, "parentId"),
            fields: params.get("fields").cloned().unwrap_or_default(),
            media_types: get_param(&params, "mediaTypes"),
            enable_image_types: params.get("enableImageTypes").cloned().unwrap_or_default(),
            enable_total_record_count: get_param(&params, "enableTotalRecordCount")
                .map(|v| v == "true"),
            enable_user_data: get_param(&params, "enableUserData").map(|v| v == "true"),
        }
    }
}

#[derive(Debug, Default)]
pub struct NextUpQuery {
    pub user_id: Option<String>,
    pub parent_id: Option<String>,
    pub fields: Vec<String>,
    pub limit: Option<i32>,
    pub image_type_limit: Option<i32>,
    pub next_up_date_cutoff: Option<String>,
    pub disable_first_episode: Option<bool>,
    pub enable_resumable: Option<bool>,
    pub enable_rewatching: Option<bool>,
}

impl NextUpQuery {
    fn from_uri(uri: &Uri) -> Self {
        let params = parse_query_params(uri.query().unwrap_or(""));
        Self {
            user_id: get_param(&params, "userId"),
            parent_id: get_param(&params, "parentId"),
            fields: params.get("fields").cloned().unwrap_or_default(),
            limit: get_param_i32(&params, "limit"),
            image_type_limit: get_param_i32(&params, "imageTypeLimit"),
            next_up_date_cutoff: get_param(&params, "nextUpDateCutoff"),
            disable_first_episode: get_param(&params, "disableFirstEpisode").map(|v| v == "true"),
            enable_resumable: get_param(&params, "enableResumable").map(|v| v == "true"),
            enable_rewatching: get_param(&params, "enableRewatching").map(|v| v == "true"),
        }
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

fn media_item_to_dto(
    item: &MediaItem,
    series_name: Option<String>,
    image_tags: Option<ImageTags>,
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
        let mut ids = std::collections::HashMap::new();
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
        series_id: if item.item_type == "Episode" {
            item.parent_id.clone()
        } else {
            None
        },
        series_name,
        season_id: None,
        season_name: item.parent_index_number.map(|s| format!("Season {}", s)),
        is_folder,
        child_count: None,
        media_type,
        collection_type: None,
        user_data: UserItemDataDto::default(),
        image_tags,
        provider_ids,
        media_sources: None,
        can_download: item.path.is_some(),
        supports_media_source_display: item.item_type == "Episode" || item.item_type == "Movie",
    }
}

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

/// GET /Users/:userId/Items/Latest
/// Returns the latest added items, optionally filtered by library
async fn get_latest_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(_user_id): Path<String>,
    uri: Uri,
) -> Result<Json<Vec<BaseItemDto>>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;
    let query = LatestQuery::from_uri(&uri);

    let limit = query.limit.unwrap_or(16).min(100);

    // Build query - get latest episodes (or movies if we had them)
    let mut sql = String::from("SELECT * FROM media_items WHERE item_type IN ('Episode', 'Movie')");

    // Filter by library if parent_id specified
    if let Some(ref parent_id) = query.parent_id {
        // parent_id is the library ID - filter by library_id
        sql.push_str(&format!(
            " AND library_id = '{}'",
            parent_id.replace('\'', "''")
        ));
    }

    // Order by creation time (newest first)
    sql.push_str(" ORDER BY created_at DESC, id DESC");
    sql.push_str(&format!(" LIMIT {}", limit));

    let items: Vec<MediaItem> = sqlx::query_as(&sql)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get series names for episodes
    let mut result = Vec::new();
    for item in items {
        let series_name = if item.item_type == "Episode" {
            if let Some(ref parent_id) = item.parent_id {
                let series: Option<MediaItem> =
                    sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
                        .bind(parent_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                series.map(|s| s.name)
            } else {
                None
            }
        } else {
            None
        };
        let image_tags = get_image_tags_for_item(&state.db, &item.id).await;
        result.push(media_item_to_dto(&item, series_name, image_tags));
    }

    // Note: Latest endpoint returns an array directly, not wrapped in ItemsResponse
    Ok(Json(result))
}

/// GET /UserItems/Resume
/// Returns items that are in progress (have playback position)
async fn get_resume_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;
    let query = ResumeQuery::from_uri(&uri);

    let limit = query.limit.unwrap_or(16).min(100);

    // Get items with playback progress for this user
    let items: Vec<MediaItem> = sqlx::query_as(
        "SELECT m.* FROM media_items m
         INNER JOIN playback_progress p ON m.id = p.item_id
         WHERE p.user_id = ? AND p.position_ticks > 0 AND p.played = 0
         AND m.item_type IN ('Episode', 'Movie')
         ORDER BY p.last_played DESC
         LIMIT ?",
    )
    .bind(&user.id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get series names and playback progress for each item
    let mut result = Vec::new();
    for item in items {
        let series_name = if item.item_type == "Episode" {
            if let Some(ref parent_id) = item.parent_id {
                let series: Option<MediaItem> =
                    sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
                        .bind(parent_id)
                        .fetch_optional(&state.db)
                        .await
                        .ok()
                        .flatten();
                series.map(|s| s.name)
            } else {
                None
            }
        } else {
            None
        };

        let image_tags = get_image_tags_for_item(&state.db, &item.id).await;
        let mut dto = media_item_to_dto(&item, series_name, image_tags);

        // Get playback progress for this item
        let progress: Option<(i64, bool)> = sqlx::query_as(
            "SELECT position_ticks, played FROM playback_progress WHERE user_id = ? AND item_id = ?",
        )
        .bind(&user.id)
        .bind(&item.id)
        .fetch_optional(&state.db)
        .await
        .ok()
        .flatten();

        if let Some((position, played)) = progress {
            // Check if favorited
            let is_favorite = sqlx::query_scalar::<_, i32>(
                "SELECT 1 FROM user_favorites WHERE user_id = ? AND item_id = ?",
            )
            .bind(&user.id)
            .bind(&item.id)
            .fetch_optional(&state.db)
            .await
            .ok()
            .flatten()
            .is_some();

            dto.user_data = UserItemDataDto {
                playback_position_ticks: position,
                play_count: if played { 1 } else { 0 },
                is_favorite,
                played,
                last_played_date: None,
            };
        }

        result.push(dto);
    }

    Ok(Json(ItemsResponse {
        items: result,
        total_record_count: 0, // Not including total count per client request
        start_index: 0,
    }))
}

/// GET /Shows/NextUp
/// Returns the next unwatched episode for each series the user is watching
async fn get_next_up(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    uri: Uri,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;
    let query = NextUpQuery::from_uri(&uri);

    let limit = query.limit.unwrap_or(16).min(100);

    // Find series where the user has watched at least one episode
    // Then get the next unwatched episode
    let items: Vec<MediaItem> = sqlx::query_as(
        "SELECT m.* FROM media_items m
         WHERE m.item_type = 'Episode'
         AND m.parent_id IN (
             -- Series where user has progress
             SELECT DISTINCT m2.parent_id FROM media_items m2
             INNER JOIN playback_progress p ON m2.id = p.item_id
             WHERE p.user_id = ? AND m2.item_type = 'Episode'
         )
         AND m.id NOT IN (
             -- Episodes already fully watched
             SELECT item_id FROM playback_progress WHERE user_id = ? AND played = 1
         )
         AND m.id NOT IN (
             -- Episodes in progress (those go to Resume)
             SELECT item_id FROM playback_progress WHERE user_id = ? AND position_ticks > 0 AND played = 0
         )
         ORDER BY m.parent_id, m.parent_index_number, m.index_number
         LIMIT ?",
    )
    .bind(&user.id)
    .bind(&user.id)
    .bind(&user.id)
    .bind(limit)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Deduplicate - only one episode per series (the next one to watch)
    let mut seen_series: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result = Vec::new();

    for item in items {
        if let Some(ref parent_id) = item.parent_id {
            if seen_series.contains(parent_id) {
                continue;
            }
            seen_series.insert(parent_id.clone());
        }

        let series_name = if let Some(ref parent_id) = item.parent_id {
            let series: Option<MediaItem> =
                sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
                    .bind(parent_id)
                    .fetch_optional(&state.db)
                    .await
                    .ok()
                    .flatten();
            series.map(|s| s.name)
        } else {
            None
        };

        let image_tags = get_image_tags_for_item(&state.db, &item.id).await;
        result.push(media_item_to_dto(&item, series_name, image_tags));

        if result.len() >= limit as usize {
            break;
        }
    }

    Ok(Json(ItemsResponse {
        items: result,
        total_record_count: 0,
        start_index: 0,
    }))
}
