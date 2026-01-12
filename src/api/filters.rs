// Browse filters API - Genres, Studios endpoints

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{services::auth, AppState};

use super::items::{BaseItemDto, ImageTags, UserItemDataDto};
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_genres))
        .route("/:name", get(get_genre))
}

pub fn studio_routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_studios))
        .route("/:name", get(get_studio))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilterQuery {
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub search_term: Option<String>,
    pub parent_id: Option<String>,
    pub user_id: Option<String>,
    pub is_favorite: Option<bool>,
    pub sort_by: Option<String>,
    pub sort_order: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FilterItemsResponse {
    pub items: Vec<BaseItemDto>,
    pub total_record_count: i32,
    pub start_index: i32,
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

/// GET /Genres
/// Returns list of all genres with item counts
async fn get_genres(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FilterQuery>,
) -> Result<Json<FilterItemsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(500);

    // Get genres with item counts
    let mut sql = String::from(
        "SELECT g.id, g.name, COUNT(ig.item_id) as item_count 
         FROM genres g
         LEFT JOIN item_genres ig ON g.id = ig.genre_id
         LEFT JOIN media_items m ON ig.item_id = m.id",
    );

    // Filter by library if parent_id is provided
    if let Some(ref parent_id) = query.parent_id {
        sql.push_str(&format!(
            " AND m.library_id = '{}'",
            parent_id.replace('\'', "''")
        ));
    }

    sql.push_str(" GROUP BY g.id, g.name");

    // Search term filter
    if let Some(ref term) = query.search_term {
        let escaped = term.replace('\'', "''").to_lowercase();
        sql.push_str(&format!(" HAVING LOWER(g.name) LIKE '%{}%'", escaped));
    }

    // Sorting
    let sort_order = if query.sort_order.as_deref() == Some("Descending") {
        "DESC"
    } else {
        "ASC"
    };
    sql.push_str(&format!(
        " ORDER BY g.name {} LIMIT {} OFFSET {}",
        sort_order, limit, start_index
    ));

    #[derive(sqlx::FromRow)]
    struct GenreRow {
        id: String,
        name: String,
        item_count: i32,
    }

    let genres: Vec<GenreRow> = sqlx::query_as(&sql)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    // Get total count
    let total: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM genres")
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<BaseItemDto> = genres
        .into_iter()
        .map(|g| BaseItemDto {
            id: g.id,
            name: g.name,
            item_type: "Genre".to_string(),
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
            sort_name: None,
            series_id: None,
            series_name: None,
            season_id: None,
            season_name: None,
            is_folder: true,
            child_count: Some(g.item_count),
            media_type: None,
            collection_type: None,
            user_data: UserItemDataDto::default(),
            image_tags: None,
            provider_ids: None,
            media_sources: None,
            can_download: false,
            supports_media_source_display: false,
        })
        .collect();

    Ok(Json(FilterItemsResponse {
        items,
        total_record_count: total.0,
        start_index,
    }))
}

/// GET /Genres/:name
async fn get_genre(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<BaseItemDto>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // URL decode the name
    let decoded_name = urlencoding::decode(&name)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid genre name".to_string()))?;

    #[derive(sqlx::FromRow)]
    struct GenreRow {
        id: String,
        name: String,
    }

    let genre: GenreRow = sqlx::query_as("SELECT id, name FROM genres WHERE name = ?")
        .bind(decoded_name.as_ref())
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Genre not found".to_string()))?;

    // Get item count
    let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM item_genres WHERE genre_id = ?")
        .bind(&genre.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    Ok(Json(BaseItemDto {
        id: genre.id,
        name: genre.name,
        item_type: "Genre".to_string(),
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
        sort_name: None,
        series_id: None,
        series_name: None,
        season_id: None,
        season_name: None,
        is_folder: true,
        child_count: Some(count.0),
        media_type: None,
        collection_type: None,
        user_data: UserItemDataDto::default(),
        image_tags: None,
        provider_ids: None,
        media_sources: None,
        can_download: false,
        supports_media_source_display: false,
    }))
}

/// GET /Studios
async fn get_studios(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<FilterQuery>,
) -> Result<Json<FilterItemsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(500);

    let mut sql = String::from(
        "SELECT s.id, s.name, COUNT(ist.item_id) as item_count 
         FROM studios s
         LEFT JOIN item_studios ist ON s.id = ist.studio_id
         LEFT JOIN media_items m ON ist.item_id = m.id",
    );

    if let Some(ref parent_id) = query.parent_id {
        sql.push_str(&format!(
            " AND m.library_id = '{}'",
            parent_id.replace('\'', "''")
        ));
    }

    sql.push_str(" GROUP BY s.id, s.name");

    if let Some(ref term) = query.search_term {
        let escaped = term.replace('\'', "''").to_lowercase();
        sql.push_str(&format!(" HAVING LOWER(s.name) LIKE '%{}%'", escaped));
    }

    let sort_order = if query.sort_order.as_deref() == Some("Descending") {
        "DESC"
    } else {
        "ASC"
    };
    sql.push_str(&format!(
        " ORDER BY s.name {} LIMIT {} OFFSET {}",
        sort_order, limit, start_index
    ));

    #[derive(sqlx::FromRow)]
    struct StudioRow {
        id: String,
        name: String,
        item_count: i32,
    }

    let studios: Vec<StudioRow> = sqlx::query_as(&sql)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let total: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM studios")
        .fetch_one(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let items: Vec<BaseItemDto> = studios
        .into_iter()
        .map(|s| BaseItemDto {
            id: s.id,
            name: s.name,
            item_type: "Studio".to_string(),
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
            sort_name: None,
            series_id: None,
            series_name: None,
            season_id: None,
            season_name: None,
            is_folder: true,
            child_count: Some(s.item_count),
            media_type: None,
            collection_type: None,
            user_data: UserItemDataDto::default(),
            image_tags: None,
            provider_ids: None,
            media_sources: None,
            can_download: false,
            supports_media_source_display: false,
        })
        .collect();

    Ok(Json(FilterItemsResponse {
        items,
        total_record_count: total.0,
        start_index,
    }))
}

/// GET /Studios/:name
async fn get_studio(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(name): Path<String>,
) -> Result<Json<BaseItemDto>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let decoded_name = urlencoding::decode(&name)
        .map_err(|_| (StatusCode::BAD_REQUEST, "Invalid studio name".to_string()))?;

    #[derive(sqlx::FromRow)]
    struct StudioRow {
        id: String,
        name: String,
    }

    let studio: StudioRow = sqlx::query_as("SELECT id, name FROM studios WHERE name = ?")
        .bind(decoded_name.as_ref())
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Studio not found".to_string()))?;

    let count: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM item_studios WHERE studio_id = ?")
        .bind(&studio.id)
        .fetch_one(&state.db)
        .await
        .unwrap_or((0,));

    Ok(Json(BaseItemDto {
        id: studio.id,
        name: studio.name,
        item_type: "Studio".to_string(),
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
        sort_name: None,
        series_id: None,
        series_name: None,
        season_id: None,
        season_name: None,
        is_folder: true,
        child_count: Some(count.0),
        media_type: None,
        collection_type: None,
        user_data: UserItemDataDto::default(),
        image_tags: None,
        provider_ids: None,
        media_sources: None,
        can_download: false,
        supports_media_source_display: false,
    }))
}

/// Helper to insert or get a genre ID
pub async fn get_or_create_genre(
    pool: &sqlx::SqlitePool,
    name: &str,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();

    // Try to insert, ignore if exists
    sqlx::query("INSERT OR IGNORE INTO genres (id, name) VALUES (?, ?)")
        .bind(&id)
        .bind(name)
        .execute(pool)
        .await?;

    // Get the actual ID (might be different if it already existed)
    let result: (String,) = sqlx::query_as("SELECT id FROM genres WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await?;

    Ok(result.0)
}

/// Helper to insert or get a studio ID
pub async fn get_or_create_studio(
    pool: &sqlx::SqlitePool,
    name: &str,
) -> Result<String, sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();

    sqlx::query("INSERT OR IGNORE INTO studios (id, name) VALUES (?, ?)")
        .bind(&id)
        .bind(name)
        .execute(pool)
        .await?;

    let result: (String,) = sqlx::query_as("SELECT id FROM studios WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await?;

    Ok(result.0)
}

/// Helper to link an item to a genre
pub async fn link_item_genre(
    pool: &sqlx::SqlitePool,
    item_id: &str,
    genre_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO item_genres (item_id, genre_id) VALUES (?, ?)")
        .bind(item_id)
        .bind(genre_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Helper to link an item to a studio  
pub async fn link_item_studio(
    pool: &sqlx::SqlitePool,
    item_id: &str,
    studio_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO item_studios (item_id, studio_id) VALUES (?, ?)")
        .bind(item_id)
        .bind(studio_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Helper to insert or get a person ID
pub async fn get_or_create_person(
    pool: &sqlx::SqlitePool,
    cast_member: &crate::services::anilist::CastMember,
) -> Result<String, sqlx::Error> {
    // Check if person exists by ID first
    let existing: Option<(String,)> =
        sqlx::query_as::<_, (String,)>("SELECT id FROM persons WHERE id = ?")
            .bind(&cast_member.person_id)
            .fetch_optional(pool)
            .await?;

    if let Some(row) = existing {
        return Ok(row.0);
    }

    // Insert new person
    let sort_name = cast_member.person_name.to_lowercase();
    let anilist_id = cast_member
        .person_id
        .strip_prefix("anilist-staff-")
        .map(|s| s.to_string());

    sqlx::query(
        "INSERT OR IGNORE INTO persons (id, name, role, image_url, anilist_id, sort_name) VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(&cast_member.person_id)
    .bind(&cast_member.person_name)
    .bind(&cast_member.role)
    .bind(&cast_member.person_image_url)
    .bind(&anilist_id)
    .bind(&sort_name)
    .execute(pool)
    .await?;

    Ok(cast_member.person_id.clone())
}

/// Helper to link an item to a person (cast/voice actor)
pub async fn link_item_person(
    pool: &sqlx::SqlitePool,
    item_id: &str,
    person_id: &str,
    role: Option<&str>,
    sort_order: i32,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO item_persons (item_id, person_id, role, sort_order) VALUES (?, ?, ?, ?)",
    )
    .bind(item_id)
    .bind(person_id)
    .bind(role.unwrap_or(""))
    .bind(sort_order)
    .execute(pool)
    .await?;
    Ok(())
}
