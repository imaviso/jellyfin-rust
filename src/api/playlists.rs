use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{models::MediaItem, services::auth, AppState};

use super::items::{BaseItemDto, ImageTags, UserItemDataDto};
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_playlists))
        .route("/", post(create_playlist))
        .route("/:id", get(get_playlist))
        .route("/:id", delete(delete_playlist))
        .route("/:id/Items", get(get_playlist_items))
        .route("/:id/Items", post(add_items_to_playlist))
        .route("/:id/Items", delete(remove_items_from_playlist))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistsQuery {
    pub user_id: Option<String>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreatePlaylistRequest {
    pub name: String,
    pub ids: Option<String>,
    pub user_id: Option<String>,
    pub media_type: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistItemsQuery {
    pub ids: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistCreatedResponse {
    pub id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistsResponse {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: i32,
    pub start_index: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct PlaylistRow {
    id: String,
    name: String,
    user_id: String,
    media_type: Option<String>,
    sort_name: Option<String>,
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

async fn get_playlists(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PlaylistsQuery>,
) -> Result<Json<PlaylistsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(500);

    let playlists: Vec<PlaylistRow> = sqlx::query_as(
        "SELECT id, name, user_id, media_type, sort_name FROM playlists WHERE user_id = ? ORDER BY COALESCE(sort_name, name) LIMIT ? OFFSET ?",
    )
    .bind(&user.id)
    .bind(limit)
    .bind(start_index)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM playlists WHERE user_id = ?")
        .bind(&user.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    let mut items = Vec::with_capacity(playlists.len());
    for pl in playlists {
        let count: (i32,) =
            sqlx::query_as("SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?")
                .bind(&pl.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or((0,));

        items.push(BaseItemDto {
            id: pl.id.clone(),
            name: pl.name,
            item_type: "Playlist".to_string(),
            server_id: "jellyfin-rust-server".to_string(),
            parent_id: None,
            overview: None,
            year: None,
            production_year: None,
            index_number: None,
            parent_index_number: None,
            runtime_ticks: None,
            community_rating: None,
            path: None,
            premiere_date: None,
            sort_name: pl.sort_name,
            series_id: None,
            series_name: None,
            season_id: None,
            season_name: None,
            is_folder: true,
            child_count: Some(count.0),
            media_type: pl.media_type,
            collection_type: None,
            user_data: UserItemDataDto::default(),
            image_tags: None,
            provider_ids: None,
            media_sources: None,
            can_download: false,
            supports_media_source_display: false,
        });
    }

    Ok(Json(PlaylistsResponse {
        items,
        total_record_count: total.0,
        start_index,
    }))
}

async fn create_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CreatePlaylistRequest>,
) -> Result<Json<PlaylistCreatedResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    let playlist_id = uuid::Uuid::new_v4().to_string();
    let sort_name = query.name.to_lowercase();

    sqlx::query(
        "INSERT INTO playlists (id, name, user_id, media_type, sort_name) VALUES (?, ?, ?, ?, ?)",
    )
    .bind(&playlist_id)
    .bind(&query.name)
    .bind(&user.id)
    .bind(&query.media_type)
    .bind(&sort_name)
    .execute(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    if let Some(ref ids) = query.ids {
        for (i, item_id) in ids.split(',').enumerate() {
            let item_id = item_id.trim();
            if !item_id.is_empty() {
                let _ = sqlx::query(
                    "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id, sort_order) VALUES (?, ?, ?)",
                )
                .bind(&playlist_id)
                .bind(item_id)
                .bind(i as i32)
                .execute(&state.db)
                .await;
            }
        }
    }

    Ok(Json(PlaylistCreatedResponse { id: playlist_id }))
}

async fn get_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<BaseItemDto>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    let playlist: PlaylistRow = sqlx::query_as(
        "SELECT id, name, user_id, media_type, sort_name FROM playlists WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Playlist not found".to_string()))?;

    let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM playlist_items WHERE playlist_id = ?")
        .bind(&id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    Ok(Json(BaseItemDto {
        id: playlist.id,
        name: playlist.name,
        item_type: "Playlist".to_string(),
        server_id: "jellyfin-rust-server".to_string(),
        parent_id: None,
        overview: None,
        year: None,
        production_year: None,
        index_number: None,
        parent_index_number: None,
        runtime_ticks: None,
        community_rating: None,
        path: None,
        premiere_date: None,
        sort_name: playlist.sort_name,
        series_id: None,
        series_name: None,
        season_id: None,
        season_name: None,
        is_folder: true,
        child_count: Some(count.0),
        media_type: playlist.media_type,
        collection_type: None,
        user_data: UserItemDataDto::default(),
        image_tags: None,
        provider_ids: None,
        media_sources: None,
        can_download: false,
        supports_media_source_display: false,
    }))
}

async fn delete_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    sqlx::query("DELETE FROM playlists WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&user.id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn get_playlist_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<PlaylistsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Verify user owns this playlist
    let _playlist: PlaylistRow = sqlx::query_as(
        "SELECT id, name, user_id, media_type, sort_name FROM playlists WHERE id = ? AND user_id = ?",
    )
    .bind(&id)
    .bind(&user.id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Playlist not found".to_string()))?;

    let items: Vec<MediaItem> = sqlx::query_as(
        r#"
        SELECT m.* FROM media_items m
        JOIN playlist_items pi ON m.id = pi.item_id
        WHERE pi.playlist_id = ?
        ORDER BY pi.sort_order, m.sort_name
        "#,
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = items.len() as i32;

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

    Ok(Json(PlaylistsResponse {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    }))
}

async fn add_items_to_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PlaylistItemsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Verify user owns this playlist
    let _: (String,) = sqlx::query_as("SELECT id FROM playlists WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&user.id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Playlist not found".to_string()))?;

    let max_order: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(sort_order), 0) FROM playlist_items WHERE playlist_id = ?",
    )
    .bind(&id)
    .fetch_one(&state.db)
    .await
    .unwrap_or((0,));

    let mut order = max_order.0;
    for item_id in query.ids.split(',') {
        let item_id = item_id.trim();
        if !item_id.is_empty() {
            order += 1;
            let _ = sqlx::query(
                "INSERT OR IGNORE INTO playlist_items (playlist_id, item_id, sort_order) VALUES (?, ?, ?)",
            )
            .bind(&id)
            .bind(item_id)
            .bind(order)
            .execute(&state.db)
            .await;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

async fn remove_items_from_playlist(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<PlaylistItemsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Verify user owns this playlist
    let _: (String,) = sqlx::query_as("SELECT id FROM playlists WHERE id = ? AND user_id = ?")
        .bind(&id)
        .bind(&user.id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Playlist not found".to_string()))?;

    for item_id in query.ids.split(',') {
        let item_id = item_id.trim();
        if !item_id.is_empty() {
            let _ = sqlx::query("DELETE FROM playlist_items WHERE playlist_id = ? AND item_id = ?")
                .bind(&id)
                .bind(item_id)
                .execute(&state.db)
                .await;
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

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
