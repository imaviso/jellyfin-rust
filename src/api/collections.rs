// Collections API - User-created groupings of items

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
        .route("/", get(get_collections))
        .route("/", post(create_collection))
        .route("/:id", get(get_collection))
        .route("/:id", delete(delete_collection))
        .route("/:id/Items", get(get_collection_items))
        .route("/:id/Items", post(add_items_to_collection))
        .route("/:id/Items", delete(remove_items_from_collection))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionsQuery {
    pub user_id: Option<String>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateCollectionRequest {
    pub name: String,
    pub ids: Option<String>, // Comma-separated item IDs to add
    pub parent_id: Option<String>,
    pub is_locked: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionItemsQuery {
    pub ids: String, // Comma-separated item IDs
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionCreatedResponse {
    pub id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct CollectionsResponse {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: i32,
    pub start_index: i32,
}

#[derive(Debug, sqlx::FromRow)]
struct CollectionRow {
    id: String,
    name: String,
    overview: Option<String>,
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

/// GET /Collections - List all collections
async fn get_collections(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CollectionsQuery>,
) -> Result<Json<CollectionsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(500);

    let collections: Vec<CollectionRow> = sqlx::query_as(
        "SELECT id, name, overview, sort_name FROM collections ORDER BY COALESCE(sort_name, name) LIMIT ? OFFSET ?",
    )
    .bind(limit)
    .bind(start_index)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM collections")
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    // Convert to DTOs
    let mut items = Vec::with_capacity(collections.len());
    for col in collections {
        // Get item count in collection
        let count: (i32,) =
            sqlx::query_as("SELECT COUNT(*) FROM collection_items WHERE collection_id = ?")
                .bind(&col.id)
                .fetch_one(&state.db)
                .await
                .unwrap_or((0,));

        items.push(BaseItemDto {
            id: col.id.clone(),
            name: col.name,
            item_type: "BoxSet".to_string(),
            server_id: "jellyfin-rust-server".to_string(),
            parent_id: None,
            overview: col.overview,
            year: None,
            production_year: None,
            index_number: None,
            parent_index_number: None,
            runtime_ticks: None,
            community_rating: None,
            path: None,
            premiere_date: None,
            sort_name: col.sort_name,
            series_id: None,
            series_name: None,
            season_id: None,
            season_name: None,
            is_folder: true,
            child_count: Some(count.0),
            media_type: None,
            collection_type: Some("boxsets".to_string()),
            user_data: UserItemDataDto::default(),
            image_tags: None,
            provider_ids: None,
            media_sources: None,
            can_download: false,
            supports_media_source_display: false,
        });
    }

    Ok(Json(CollectionsResponse {
        items,
        total_record_count: total.0,
        start_index,
    }))
}

/// POST /Collections - Create a new collection
async fn create_collection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<CreateCollectionRequest>,
) -> Result<Json<CollectionCreatedResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let collection_id = uuid::Uuid::new_v4().to_string();
    let sort_name = query.name.to_lowercase();

    sqlx::query("INSERT INTO collections (id, name, sort_name) VALUES (?, ?, ?)")
        .bind(&collection_id)
        .bind(&query.name)
        .bind(&sort_name)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Add initial items if provided
    if let Some(ref ids) = query.ids {
        for (i, item_id) in ids.split(',').enumerate() {
            let item_id = item_id.trim();
            if !item_id.is_empty() {
                let _ = sqlx::query(
                    "INSERT OR IGNORE INTO collection_items (collection_id, item_id, sort_order) VALUES (?, ?, ?)",
                )
                .bind(&collection_id)
                .bind(item_id)
                .bind(i as i32)
                .execute(&state.db)
                .await;
            }
        }
    }

    Ok(Json(CollectionCreatedResponse { id: collection_id }))
}

/// GET /Collections/:id - Get a specific collection
async fn get_collection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<BaseItemDto>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let collection: CollectionRow =
        sqlx::query_as("SELECT id, name, overview, sort_name FROM collections WHERE id = ?")
            .bind(&id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
            .ok_or_else(|| (StatusCode::NOT_FOUND, "Collection not found".to_string()))?;

    let count: (i32,) =
        sqlx::query_as("SELECT COUNT(*) FROM collection_items WHERE collection_id = ?")
            .bind(&id)
            .fetch_one(&state.db)
            .await
            .unwrap_or((0,));

    Ok(Json(BaseItemDto {
        id: collection.id,
        name: collection.name,
        item_type: "BoxSet".to_string(),
        server_id: "jellyfin-rust-server".to_string(),
        parent_id: None,
        overview: collection.overview,
        year: None,
        production_year: None,
        index_number: None,
        parent_index_number: None,
        runtime_ticks: None,
        community_rating: None,
        path: None,
        premiere_date: None,
        sort_name: collection.sort_name,
        series_id: None,
        series_name: None,
        season_id: None,
        season_name: None,
        is_folder: true,
        child_count: Some(count.0),
        media_type: None,
        collection_type: Some("boxsets".to_string()),
        user_data: UserItemDataDto::default(),
        image_tags: None,
        provider_ids: None,
        media_sources: None,
        can_download: false,
        supports_media_source_display: false,
    }))
}

/// DELETE /Collections/:id - Delete a collection
async fn delete_collection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    sqlx::query("DELETE FROM collections WHERE id = ?")
        .bind(&id)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

/// GET /Collections/:id/Items - Get items in a collection
async fn get_collection_items(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<CollectionsResponse>, (StatusCode, String)> {
    let user = require_auth(&state, &headers).await?;

    // Get items in the collection
    let items: Vec<MediaItem> = sqlx::query_as(
        r#"
        SELECT m.* FROM media_items m
        JOIN collection_items ci ON m.id = ci.item_id
        WHERE ci.collection_id = ?
        ORDER BY ci.sort_order, m.sort_name
        "#,
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total = items.len() as i32;

    // Convert to DTOs
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

    Ok(Json(CollectionsResponse {
        items: dtos,
        total_record_count: total,
        start_index: 0,
    }))
}

/// POST /Collections/:id/Items - Add items to a collection
async fn add_items_to_collection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<CollectionItemsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get current max sort order
    let max_order: (i32,) = sqlx::query_as(
        "SELECT COALESCE(MAX(sort_order), 0) FROM collection_items WHERE collection_id = ?",
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
                "INSERT OR IGNORE INTO collection_items (collection_id, item_id, sort_order) VALUES (?, ?, ?)",
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

/// DELETE /Collections/:id/Items - Remove items from a collection
async fn remove_items_from_collection(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(query): Query<CollectionItemsQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    for item_id in query.ids.split(',') {
        let item_id = item_id.trim();
        if !item_id.is_empty() {
            let _ =
                sqlx::query("DELETE FROM collection_items WHERE collection_id = ? AND item_id = ?")
                    .bind(&id)
                    .bind(item_id)
                    .execute(&state.db)
                    .await;
        }
    }

    Ok(StatusCode::NO_CONTENT)
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
