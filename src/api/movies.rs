// Movies API - Movie-specific endpoints like recommendations

use axum::{
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{models::MediaItem, services::auth, AppState};

use super::items::{BaseItemDto, ImageTags, UserItemDataDto};
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/Recommendations", get(get_recommendations))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecommendationsQuery {
    pub user_id: Option<String>,
    pub parent_id: Option<String>,
    pub fields: Option<String>,
    pub category_limit: Option<i32>,
    pub item_limit: Option<i32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct RecommendationDto {
    pub items: Vec<BaseItemDto>,
    pub recommendation_type: String,
    pub baseline_item_name: Option<String>,
    pub category_id: String,
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

/// GET /Movies/Recommendations - Get movie recommendations
/// Returns recommendations based on:
/// 1. Similar to favorites
/// 2. Similar to recently watched
/// 3. By genre
async fn get_recommendations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<RecommendationsQuery>,
) -> Result<Json<Vec<RecommendationDto>>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    let category_limit = query.category_limit.unwrap_or(5).min(10);
    let item_limit = query.item_limit.unwrap_or(8).min(20);

    let mut recommendations = Vec::new();

    // Category 1: Based on favorites
    let favorite_movies: Vec<MediaItem> = sqlx::query_as(
        "SELECT m.* FROM media_items m
         INNER JOIN user_favorites f ON m.id = f.item_id
         WHERE f.user_id = ? AND m.item_type = 'Movie'
         ORDER BY RANDOM()
         LIMIT 3",
    )
    .bind(&user.id)
    .fetch_all(&state.db)
    .await
    .unwrap_or_default();

    for fav in favorite_movies.iter().take(category_limit as usize) {
        // Get genres of this favorite
        let genres: Vec<(String,)> = sqlx::query_as(
            "SELECT g.name FROM genres g
             INNER JOIN item_genres ig ON g.id = ig.genre_id
             WHERE ig.item_id = ?",
        )
        .bind(&fav.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        if genres.is_empty() {
            continue;
        }

        let genre_names: Vec<String> = genres.into_iter().map(|(g,)| g).collect();

        // Find similar movies by genre
        let placeholders: Vec<String> = genre_names.iter().map(|_| "?".to_string()).collect();
        let sql = format!(
            "SELECT DISTINCT m.* FROM media_items m
             INNER JOIN item_genres ig ON m.id = ig.item_id
             INNER JOIN genres g ON ig.genre_id = g.id
             WHERE m.item_type = 'Movie' AND m.id != ? AND g.name IN ({})
             ORDER BY m.community_rating DESC NULLS LAST
             LIMIT ?",
            placeholders.join(",")
        );

        let mut query_builder = sqlx::query_as::<_, MediaItem>(&sql).bind(&fav.id);
        for genre in &genre_names {
            query_builder = query_builder.bind(genre);
        }
        query_builder = query_builder.bind(item_limit);

        let similar: Vec<MediaItem> = query_builder
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

        if !similar.is_empty() {
            let items = convert_to_dtos(&state, &similar, &user.id).await;
            recommendations.push(RecommendationDto {
                items,
                recommendation_type: "SimilarToLikedItem".to_string(),
                baseline_item_name: Some(fav.name.clone()),
                category_id: format!("similar-{}", fav.id),
            });
        }

        if recommendations.len() >= category_limit as usize {
            break;
        }
    }

    // Category 2: Based on recently watched
    if recommendations.len() < category_limit as usize {
        let recent_movies: Vec<MediaItem> = sqlx::query_as(
            "SELECT m.* FROM media_items m
             INNER JOIN playback_progress p ON m.id = p.item_id
             WHERE p.user_id = ? AND m.item_type = 'Movie' AND p.played = 1
             ORDER BY p.last_played DESC
             LIMIT 3",
        )
        .bind(&user.id)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for recent in recent_movies.iter() {
            if recommendations.len() >= category_limit as usize {
                break;
            }

            // Get genres of this recent movie
            let genres: Vec<(String,)> = sqlx::query_as(
                "SELECT g.name FROM genres g
                 INNER JOIN item_genres ig ON g.id = ig.genre_id
                 WHERE ig.item_id = ?",
            )
            .bind(&recent.id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            if genres.is_empty() {
                continue;
            }

            let genre_names: Vec<String> = genres.into_iter().map(|(g,)| g).collect();
            let placeholders: Vec<String> = genre_names.iter().map(|_| "?".to_string()).collect();
            let sql = format!(
                "SELECT DISTINCT m.* FROM media_items m
                 INNER JOIN item_genres ig ON m.id = ig.item_id
                 INNER JOIN genres g ON ig.genre_id = g.id
                 WHERE m.item_type = 'Movie' AND m.id != ? AND g.name IN ({})
                 AND m.id NOT IN (SELECT item_id FROM playback_progress WHERE user_id = ? AND played = 1)
                 ORDER BY m.community_rating DESC NULLS LAST
                 LIMIT ?",
                placeholders.join(",")
            );

            let mut query_builder = sqlx::query_as::<_, MediaItem>(&sql).bind(&recent.id);
            for genre in &genre_names {
                query_builder = query_builder.bind(genre);
            }
            query_builder = query_builder.bind(&user.id).bind(item_limit);

            let similar: Vec<MediaItem> = query_builder
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();

            if !similar.is_empty() {
                let items = convert_to_dtos(&state, &similar, &user.id).await;
                recommendations.push(RecommendationDto {
                    items,
                    recommendation_type: "SimilarToRecentlyPlayed".to_string(),
                    baseline_item_name: Some(recent.name.clone()),
                    category_id: format!("recent-{}", recent.id),
                });
            }
        }
    }

    // Category 3: By top genres (if we still need more categories)
    if recommendations.len() < category_limit as usize {
        let top_genres: Vec<(String, String)> = sqlx::query_as(
            "SELECT g.name, g.id FROM genres g
             INNER JOIN item_genres ig ON g.id = ig.genre_id
             INNER JOIN media_items m ON ig.item_id = m.id
             WHERE m.item_type = 'Movie'
             GROUP BY g.id
             ORDER BY COUNT(*) DESC
             LIMIT 5",
        )
        .fetch_all(&state.db)
        .await
        .unwrap_or_default();

        for (genre_name, genre_id) in top_genres {
            if recommendations.len() >= category_limit as usize {
                break;
            }

            let movies: Vec<MediaItem> = sqlx::query_as(
                "SELECT m.* FROM media_items m
                 INNER JOIN item_genres ig ON m.id = ig.item_id
                 WHERE ig.genre_id = ? AND m.item_type = 'Movie'
                 AND m.id NOT IN (SELECT item_id FROM playback_progress WHERE user_id = ? AND played = 1)
                 ORDER BY m.community_rating DESC NULLS LAST
                 LIMIT ?",
            )
            .bind(&genre_id)
            .bind(&user.id)
            .bind(item_limit)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

            if !movies.is_empty() {
                let items = convert_to_dtos(&state, &movies, &user.id).await;
                recommendations.push(RecommendationDto {
                    items,
                    recommendation_type: "HasDirectorFrom".to_string(), // Using this as "By Genre"
                    baseline_item_name: Some(genre_name.clone()),
                    category_id: format!("genre-{}", genre_id),
                });
            }
        }
    }

    Ok(Json(recommendations))
}

/// Helper to convert MediaItems to BaseItemDto
async fn convert_to_dtos(
    state: &AppState,
    items: &[MediaItem],
    user_id: &str,
) -> Vec<BaseItemDto> {
    let mut dtos = Vec::with_capacity(items.len());

    for item in items {
        let is_folder = matches!(
            item.item_type.as_str(),
            "Series" | "Season" | "Folder" | "CollectionFolder"
        );
        let media_type = match item.item_type.as_str() {
            "Episode" | "Movie" => Some("Video".to_string()),
            "Audio" => Some("Audio".to_string()),
            _ => None,
        };

        // Get image tags
        let image_tags = get_image_tags_for_item(&state.db, &item.id).await;

        // Get user data
        let user_data = get_user_item_data(&state.db, user_id, &item.id).await;

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

    dtos
}

/// Helper to fetch image tags for an item
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

/// Get user-specific data for an item
async fn get_user_item_data(
    pool: &sqlx::SqlitePool,
    user_id: &str,
    item_id: &str,
) -> UserItemDataDto {
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
