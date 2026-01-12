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

use crate::{models::MediaItem, services::auth, AppState};

use super::users::parse_emby_auth_header;

// =============================================================================
// Image Info (for listing images)
// =============================================================================

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageInfo {
    pub image_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_index: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub image_tag: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blur_hash: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct ItemIdPath {
    #[serde(rename = "itemId")]
    item_id: String,
}

/// GET /Items/:itemId/Images - Get list of images for an item
async fn get_item_images(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<ItemIdPath>,
) -> Result<Json<Vec<ImageInfo>>, (StatusCode, String)> {
    // Images don't require auth in Jellyfin by default
    if let Some((_, _, _, Some(token))) = parse_emby_auth_header(&headers) {
        let _ = auth::validate_session(&state.db, &token).await;
    }

    let mut images = Vec::new();

    // Check if this is a synthetic season ID
    let actual_item_id = if let Some(pos) = path.item_id.rfind("_season_") {
        &path.item_id[..pos]
    } else {
        &path.item_id
    };

    // Query images from database
    #[derive(sqlx::FromRow)]
    struct ImageRow {
        image_type: String,
        path: String,
    }

    let db_images: Vec<ImageRow> =
        sqlx::query_as("SELECT image_type, path FROM images WHERE item_id = ? ORDER BY image_type")
            .bind(actual_item_id)
            .fetch_all(&state.db)
            .await
            .unwrap_or_default();

    for (idx, row) in db_images.iter().enumerate() {
        // Get file metadata for size
        let (size, width, height) = if let Ok(meta) = tokio::fs::metadata(&row.path).await {
            (Some(meta.len() as i64), None, None) // TODO: Get actual dimensions
        } else {
            (None, None, None)
        };

        // Generate a simple tag from the path hash
        let tag = format!("{:x}", md5_hash(&row.path));

        images.push(ImageInfo {
            image_type: row.image_type.clone(),
            image_index: Some(idx as i32),
            image_tag: Some(tag),
            path: Some(row.path.clone()),
            blur_hash: None, // TODO: Generate blur hashes
            height,
            width,
            size,
        });
    }

    // If no database images, try to find local images
    if images.is_empty() {
        for image_type in &["Primary", "Backdrop", "Banner", "Thumb"] {
            if let Some(img_path) = find_image_for_item(&state, &path.item_id, image_type).await {
                let (size, width, height) = if let Ok(meta) = tokio::fs::metadata(&img_path).await {
                    (Some(meta.len() as i64), None, None)
                } else {
                    (None, None, None)
                };

                let tag = format!("{:x}", md5_hash(&img_path));

                images.push(ImageInfo {
                    image_type: image_type.to_string(),
                    image_index: Some(0),
                    image_tag: Some(tag),
                    path: Some(img_path),
                    blur_hash: None,
                    height,
                    width,
                    size,
                });
            }
        }
    }

    Ok(Json(images))
}

/// Simple hash function for generating image tags
fn md5_hash(input: &str) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    input.hash(&mut hasher);
    hasher.finish()
}

// =============================================================================
// Image Query/Path structs
// =============================================================================

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:itemId/Images", get(get_item_images))
        .route("/:itemId/Images/:imageType", get(get_image))
        .route("/:itemId/Images/:imageType/:index", get(get_image_indexed))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageQuery {
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub quality: Option<u32>,
    pub fill_width: Option<u32>,
    pub fill_height: Option<u32>,
    pub tag: Option<String>,
    // We ignore most of these for now - just serve original images
}

#[derive(Debug, Deserialize)]
pub struct ImagePath {
    #[serde(rename = "itemId")]
    item_id: String,
    #[serde(rename = "imageType")]
    image_type: String,
}

#[derive(Debug, Deserialize)]
pub struct ImagePathIndexed {
    #[serde(rename = "itemId")]
    item_id: String,
    #[serde(rename = "imageType")]
    image_type: String,
    index: u32,
}

/// Get the MIME type for an image file based on extension
fn get_image_content_type(path: &str) -> &'static str {
    let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        _ => "image/jpeg", // Default to JPEG
    }
}

/// Common image file patterns to search for
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif"];

/// Search for image files near a media item
async fn find_image_for_item(state: &AppState, item_id: &str, image_type: &str) -> Option<String> {
    // Check if this is a synthetic season ID (format: {series_id}_season_{num})
    // If so, use the series ID for image lookup
    let actual_item_id = if let Some(pos) = item_id.rfind("_season_") {
        // Extract series_id from synthetic season ID (everything before _season_)
        &item_id[..pos]
    } else {
        item_id
    };

    // First check if we have an image in the database
    let db_image: Option<(String,)> =
        sqlx::query_as("SELECT path FROM images WHERE item_id = ? AND image_type = ?")
            .bind(actual_item_id)
            .bind(image_type)
            .fetch_optional(&state.db)
            .await
            .ok()?;

    if let Some((path,)) = db_image {
        if tokio::fs::metadata(&path).await.is_ok() {
            return Some(path);
        }
    }

    // Otherwise, try to find images in the media item's directory
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(actual_item_id)
        .fetch_optional(&state.db)
        .await
        .ok()??;

    // Get the directory containing the media file or series folder
    let search_dir = if let Some(ref path) = item.path {
        std::path::Path::new(path).parent()?.to_path_buf()
    } else {
        return None;
    };

    // Image filename patterns based on type
    let patterns: Vec<String> = match image_type.to_lowercase().as_str() {
        "primary" | "poster" => {
            let mut p = vec![
                "poster".to_string(),
                "cover".to_string(),
                "folder".to_string(),
            ];
            // Also try item name
            p.push(item.name.to_lowercase());
            p
        }
        "backdrop" | "fanart" => {
            vec![
                "backdrop".to_string(),
                "fanart".to_string(),
                "background".to_string(),
            ]
        }
        "banner" => vec!["banner".to_string()],
        "thumb" | "thumbnail" => vec!["thumb".to_string(), "thumbnail".to_string()],
        _ => vec![image_type.to_lowercase()],
    };

    // Search for matching image files
    for pattern in &patterns {
        for ext in IMAGE_EXTENSIONS {
            let filename = format!("{}.{}", pattern, ext);
            let image_path = search_dir.join(&filename);
            if tokio::fs::metadata(&image_path).await.is_ok() {
                return Some(image_path.to_string_lossy().to_string());
            }
        }
    }

    // For episodes, try to find series artwork in parent directory
    if item.item_type == "Episode" {
        if let Some(ref parent_id) = item.parent_id {
            // Try to find image for parent series
            return Box::pin(find_image_for_item(state, parent_id, image_type)).await;
        }
    }

    None
}

/// GET /Items/:itemId/Images/:imageType
async fn get_image(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<ImagePath>,
    Query(_query): Query<ImageQuery>,
) -> Result<Response, (StatusCode, String)> {
    // Images don't require auth in Jellyfin by default
    // But we'll check if there's a token and validate it if present
    if let Some((_, _, _, Some(token))) = parse_emby_auth_header(&headers) {
        let _ = auth::validate_session(&state.db, &token).await;
    }

    let image_path = find_image_for_item(&state, &path.item_id, &path.image_type)
        .await
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Image not found".to_string()))?;

    serve_image_file(&image_path).await
}

/// GET /Items/:itemId/Images/:imageType/:index
async fn get_image_indexed(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(path): Path<ImagePathIndexed>,
    Query(query): Query<ImageQuery>,
) -> Result<Response, (StatusCode, String)> {
    // For now, ignore index and return the primary image
    get_image(
        State(state),
        headers,
        Path(ImagePath {
            item_id: path.item_id,
            image_type: path.image_type,
        }),
        Query(query),
    )
    .await
}

/// Serve an image file
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
        .header(header::CACHE_CONTROL, "public, max-age=86400") // Cache for 24 hours
        .body(body)
        .unwrap())
}

/// Store image reference in database
pub async fn store_image(
    db: &sqlx::SqlitePool,
    item_id: &str,
    image_type: &str,
    path: &str,
) -> Result<(), sqlx::Error> {
    let id = uuid::Uuid::new_v4().to_string();
    sqlx::query(
        "INSERT OR REPLACE INTO images (id, item_id, image_type, path) VALUES (?, ?, ?, ?)",
    )
    .bind(&id)
    .bind(item_id)
    .bind(image_type)
    .bind(path)
    .execute(db)
    .await?;
    Ok(())
}
