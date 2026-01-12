// Shows endpoints - Seasons and Episodes listing for TV shows

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{models::MediaItem, services::auth, AppState};

use super::items::{BaseItemDto, ImageTags, ItemsResponse, UserItemDataDto};
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:seriesId/Seasons", get(get_seasons))
        .route("/:seriesId/Episodes", get(get_episodes))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SeasonsQuery {
    pub user_id: Option<String>,
    pub fields: Option<String>,
    pub is_special_season: Option<bool>,
    pub is_missing: Option<bool>,
    pub adjacent_to: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EpisodesQuery {
    pub user_id: Option<String>,
    pub fields: Option<String>,
    pub season: Option<i32>,
    pub season_id: Option<String>,
    pub is_missing: Option<bool>,
    pub adjacent_to: Option<String>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub start_item_id: Option<String>,
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
        season_id: if item.item_type == "Episode" {
            // Generate synthetic season_id: {series_id}_season_{season_number}
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

/// GET /Shows/:seriesId/Seasons
/// Returns seasons for a series. Since we don't have explicit Season items,
/// we'll synthesize them from episodes' parent_index_number (season number).
async fn get_seasons(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(_query): Query<SeasonsQuery>,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the series
    let series: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&series_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Series not found".to_string()))?;

    // Get series image tags to use for seasons (fallback)
    let series_image_tags = get_image_tags_for_item(&state.db, &series_id).await;

    // Get distinct season numbers from episodes
    // Use COALESCE to handle NULL as season 1 in the query itself
    let season_numbers: Vec<(i32,)> = sqlx::query_as(
        "SELECT DISTINCT COALESCE(parent_index_number, 1) as season_num FROM media_items 
         WHERE parent_id = ? AND item_type = 'Episode' 
         ORDER BY season_num",
    )
    .bind(&series_id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Create synthetic Season items
    let mut items = Vec::new();
    for (season_num,) in season_numbers {
        // Count episodes in this season
        let episode_count: (i32,) = sqlx::query_as(
            "SELECT COUNT(*) FROM media_items 
             WHERE parent_id = ? AND item_type = 'Episode' AND COALESCE(parent_index_number, 1) = ?",
        )
        .bind(&series_id)
        .bind(season_num)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

        // Season name: Season 0 = "Specials", otherwise "Season X"
        let season_name = if season_num == 0 {
            "Specials".to_string()
        } else {
            format!("Season {}", season_num)
        };

        // Sort name: Specials should sort last (use 999), otherwise by number
        let sort_name = if season_num == 0 {
            "Season 999".to_string() // Specials sort last
        } else {
            format!("Season {:03}", season_num)
        };

        items.push(BaseItemDto {
            id: format!("{}_season_{}", series_id, season_num),
            name: season_name,
            item_type: "Season".to_string(),
            server_id: "jellyfin-rust-server".to_string(),
            parent_id: Some(series_id.clone()),
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
            series_id: Some(series_id.clone()),
            series_name: Some(series.name.clone()),
            season_id: None,
            season_name: None,
            is_folder: true,
            child_count: Some(episode_count.0),
            media_type: None,
            collection_type: None,
            user_data: UserItemDataDto::default(),
            // Use series images as fallback for season images
            image_tags: series_image_tags.clone(),
            provider_ids: None,
            media_sources: None,
            can_download: false,
            supports_media_source_display: false,
        });
    }

    let total = items.len() as i32;

    Ok(Json(ItemsResponse {
        items,
        total_record_count: total,
        start_index: 0,
    }))
}

/// GET /Shows/:seriesId/Episodes
/// Returns episodes for a series, optionally filtered by season
async fn get_episodes(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(series_id): Path<String>,
    Query(query): Query<EpisodesQuery>,
) -> Result<Json<ItemsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the series for its name
    let series: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&series_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Series not found".to_string()))?;

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(1000).min(1000);

    // Build query for episodes
    let mut sql =
        String::from("SELECT * FROM media_items WHERE parent_id = ? AND item_type = 'Episode'");

    // Filter by season number if specified
    if let Some(season_num) = query.season {
        sql.push_str(&format!(" AND parent_index_number = {}", season_num));
    }

    // Or filter by synthetic season_id
    if let Some(ref season_id) = query.season_id {
        // Parse season number from synthetic ID like "seriesid_season_1"
        if let Some(num_str) = season_id.rsplit('_').next() {
            if let Ok(season_num) = num_str.parse::<i32>() {
                sql.push_str(&format!(" AND parent_index_number = {}", season_num));
            }
        }
    }

    sql.push_str(" ORDER BY parent_index_number, index_number");
    sql.push_str(&format!(" LIMIT {} OFFSET {}", limit, start_index));

    let episodes: Vec<MediaItem> = sqlx::query_as(&sql)
        .bind(&series_id)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Count total
    let mut count_sql = String::from(
        "SELECT COUNT(*) FROM media_items WHERE parent_id = ? AND item_type = 'Episode'",
    );
    if let Some(season_num) = query.season {
        count_sql.push_str(&format!(" AND parent_index_number = {}", season_num));
    }
    if let Some(ref season_id) = query.season_id {
        if let Some(num_str) = season_id.rsplit('_').next() {
            if let Ok(season_num) = num_str.parse::<i32>() {
                count_sql.push_str(&format!(" AND parent_index_number = {}", season_num));
            }
        }
    }

    let total: (i32,) = sqlx::query_as(&count_sql)
        .bind(&series_id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    // Build items with image tags
    let mut items = Vec::with_capacity(episodes.len());
    for ep in &episodes {
        let image_tags = get_image_tags_for_item(&state.db, &ep.id).await;
        items.push(media_item_to_dto(ep, Some(series.name.clone()), image_tags));
    }

    Ok(Json(ItemsResponse {
        items,
        total_record_count: total.0,
        start_index,
    }))
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
