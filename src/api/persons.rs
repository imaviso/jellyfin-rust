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

use crate::{services::auth, AppState};

use super::items::UserItemDataDto;
use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_persons))
        .route("/:id", get(get_person))
        .route("/:id/Images/:imageType", get(get_person_image))
        .route(
            "/:id/Images/:imageType/:index",
            get(get_person_image_indexed),
        )
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonsQuery {
    pub search_term: Option<String>,
    pub person_types: Option<String>,
    pub start_index: Option<i32>,
    pub limit: Option<i32>,
    pub user_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PersonsResponse {
    pub items: Vec<PersonDto>,
    pub total_record_count: i32,
    pub start_index: i32,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PersonDto {
    pub id: String,
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    pub server_id: String,
    pub role: Option<String>,
    pub primary_image_tag: Option<String>,
    pub image_tags: Option<PersonImageTags>,
    pub user_data: UserItemDataDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_ids: Option<PersonProviderIds>,
}

#[derive(Debug, Serialize, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PersonImageTags {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PersonProviderIds {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub anilist: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmdb: Option<String>,
}

#[derive(Debug, sqlx::FromRow)]
struct PersonRow {
    id: String,
    name: String,
    role: Option<String>,
    image_url: Option<String>,
    anilist_id: Option<String>,
    tmdb_id: Option<String>,
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

async fn get_persons(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<PersonsQuery>,
) -> Result<Json<PersonsResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let start_index = query.start_index.unwrap_or(0);
    let limit = query.limit.unwrap_or(100).min(500);

    let (persons, total) = if let Some(ref search) = query.search_term {
        let search_pattern = format!("%{}%", search);
        let persons: Vec<PersonRow> = sqlx::query_as(
            "SELECT id, name, role, image_url, anilist_id, tmdb_id FROM persons WHERE name LIKE ? ORDER BY name LIMIT ? OFFSET ?",
        )
        .bind(&search_pattern)
        .bind(limit)
        .bind(start_index)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let total: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM persons WHERE name LIKE ?")
            .bind(&search_pattern)
            .fetch_one(&state.db)
            .await
            .unwrap_or((0,));

        (persons, total.0)
    } else {
        let persons: Vec<PersonRow> = sqlx::query_as(
            "SELECT id, name, role, image_url, anilist_id, tmdb_id FROM persons ORDER BY name LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(start_index)
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

        let total: (i32,) = sqlx::query_as("SELECT COUNT(*) FROM persons")
            .fetch_one(&state.db)
            .await
            .unwrap_or((0,));

        (persons, total.0)
    };

    let items: Vec<PersonDto> = persons.into_iter().map(|p| person_row_to_dto(p)).collect();

    Ok(Json(PersonsResponse {
        items,
        total_record_count: total,
        start_index,
    }))
}

async fn get_person(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<PersonDto>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    let person: PersonRow = sqlx::query_as(
        "SELECT id, name, role, image_url, anilist_id, tmdb_id FROM persons WHERE id = ?",
    )
    .bind(&id)
    .fetch_optional(&state.db)
    .await
    .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .ok_or_else(|| (StatusCode::NOT_FOUND, "Person not found".to_string()))?;

    Ok(Json(person_row_to_dto(person)))
}

fn person_row_to_dto(row: PersonRow) -> PersonDto {
    let has_image = row.image_url.is_some();
    let provider_ids = if row.anilist_id.is_some() || row.tmdb_id.is_some() {
        Some(PersonProviderIds {
            anilist: row.anilist_id,
            tmdb: row.tmdb_id,
        })
    } else {
        None
    };

    PersonDto {
        id: row.id.clone(),
        name: row.name,
        item_type: "Person".to_string(),
        server_id: "jellyfin-rust-server".to_string(),
        role: row.role,
        primary_image_tag: if has_image {
            Some(row.id.clone())
        } else {
            None
        },
        image_tags: if has_image {
            Some(PersonImageTags {
                primary: Some(row.id),
            })
        } else {
            None
        },
        user_data: UserItemDataDto::default(),
        provider_ids,
    }
}

// =============================================================================
// Person Images
// =============================================================================

#[derive(Debug, Deserialize)]
pub struct PersonImagePath {
    id: String,
    #[serde(rename = "imageType")]
    image_type: String,
}

#[derive(Debug, Deserialize)]
pub struct PersonImagePathIndexed {
    id: String,
    #[serde(rename = "imageType")]
    image_type: String,
    #[allow(dead_code)]
    index: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageQuery {
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub tag: Option<String>,
}

/// GET /Persons/:id/Images/:imageType
async fn get_person_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<PersonImagePath>,
    Query(_query): Query<ImageQuery>,
) -> Result<Response, (StatusCode, String)> {
    // Images don't require auth in Jellyfin by default
    if let Some((_, _, _, Some(token))) = parse_emby_auth_header(&headers) {
        let _ = auth::validate_session(&state.db, &token).await;
    }

    // Get the person's image_url from database
    let person: Option<(Option<String>,)> =
        sqlx::query_as("SELECT image_url FROM persons WHERE id = ?")
            .bind(&path.id)
            .fetch_optional(&state.db)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let image_url = person.and_then(|(url,)| url).ok_or_else(|| {
        (
            StatusCode::NOT_FOUND,
            "Person or image not found".to_string(),
        )
    })?;

    // Check if we have the image cached locally
    let cache_dir = state.config.paths.cache_dir.join("persons");

    let cached_path = cache_dir.join(format!("{}.jpg", path.id));

    // Check if cached (use async to avoid blocking)
    if tokio::fs::try_exists(&cached_path).await.unwrap_or(false) {
        // Serve from cache
        return serve_image_file(cached_path.to_str().unwrap()).await;
    }

    // Download and cache the image
    match download_and_cache_person_image(&image_url, &cached_path).await {
        Ok(_) => serve_image_file(cached_path.to_str().unwrap()).await,
        Err(e) => {
            tracing::warn!("Failed to download person image: {}", e);
            // Try to redirect to the original URL as fallback
            Err((StatusCode::NOT_FOUND, format!("Image not available: {}", e)))
        }
    }
}

/// GET /Persons/:id/Images/:imageType/:index
async fn get_person_image_indexed(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<PersonImagePathIndexed>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, (StatusCode, String)> {
    get_person_image(
        State(state),
        headers,
        Path(PersonImagePath {
            id: path.id,
            image_type: path.image_type,
        }),
        Query(query),
    )
    .await
}

/// Download an image from URL and cache it locally
async fn download_and_cache_person_image(
    url: &str,
    cache_path: &std::path::Path,
) -> anyhow::Result<()> {
    // Create cache directory if needed
    if let Some(parent) = cache_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    // Download the image
    let client = reqwest::Client::new();
    let response = client
        .get(url)
        .header("User-Agent", "jellyfin-rust/1.0")
        .send()
        .await?;

    if !response.status().is_success() {
        anyhow::bail!("Failed to download image: HTTP {}", response.status());
    }

    let bytes = response.bytes().await?;
    tokio::fs::write(cache_path, &bytes).await?;

    tracing::debug!("Cached person image to {:?}", cache_path);
    Ok(())
}

/// Get MIME type from file path
fn get_image_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "image/jpeg",
    }
}

/// Serve an image file as HTTP response
async fn serve_image_file(path: &str) -> Result<Response, (StatusCode, String)> {
    let file = File::open(path)
        .await
        .map_err(|e| (StatusCode::NOT_FOUND, format!("Cannot open image: {}", e)))?;

    let metadata = file.metadata().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Cannot read metadata: {}", e),
        )
    })?;

    let content_type = get_image_content_type(path);
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, metadata.len())
        .header(header::CACHE_CONTROL, "public, max-age=604800") // Cache for 7 days
        .body(body)
        .unwrap())
}
