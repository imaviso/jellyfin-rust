use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

use crate::{models::Library, scanner, services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/", get(get_virtual_folders))
        .route("/", post(add_virtual_folder))
        .route("/", delete(remove_virtual_folder))
        .route("/LibraryOptions", post(update_library_options))
        .route("/Refresh", post(refresh_library))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualFolderInfo {
    pub name: String,
    pub locations: Vec<String>,
    pub collection_type: Option<String>,
    pub library_options: LibraryOptions,
    pub item_id: String,
    pub primary_image_item_id: Option<String>,
    pub refresh_status: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryOptions {
    pub enable_photos: bool,
    pub enable_realtime_monitor: bool,
    pub enable_chapter_image_extraction: bool,
    pub extract_chapter_images_during_library_scan: bool,
    pub save_local_metadata: bool,
    pub enable_internet_providers: bool,
    pub enable_automatic_series_grouping: bool,
    pub enable_embedded_titles: bool,
    pub enable_embedded_episode_infos: bool,
    pub automatic_refresh_interval_days: i32,
    pub metadata_savers: Vec<String>,
    pub type_options: Vec<TypeOptions>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct TypeOptions {
    #[serde(rename = "Type")]
    pub type_name: String,
    pub metadata_fetchers: Vec<String>,
    pub image_fetchers: Vec<String>,
}

impl Default for LibraryOptions {
    fn default() -> Self {
        Self {
            enable_photos: true,
            enable_realtime_monitor: true,
            enable_chapter_image_extraction: false,
            extract_chapter_images_during_library_scan: false,
            save_local_metadata: false,
            enable_internet_providers: true,
            enable_automatic_series_grouping: false,
            enable_embedded_titles: false,
            enable_embedded_episode_infos: false,
            automatic_refresh_interval_days: 0,
            metadata_savers: vec![],
            type_options: vec![],
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddVirtualFolderQuery {
    pub name: String,
    pub collection_type: Option<String>,
    pub paths: Option<String>,
    pub refresh_library: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AddVirtualFolderBody {
    pub library_options: Option<LibraryOptions>,
}

async fn require_admin(state: &AppState, headers: &HeaderMap) -> Result<(), (StatusCode, String)> {
    let (_, _, _, token) = parse_emby_auth_header(headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    let user = auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    if !user.is_admin {
        return Err((StatusCode::FORBIDDEN, "Admin required".to_string()));
    }

    Ok(())
}

async fn get_virtual_folders(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<VirtualFolderInfo>>, (StatusCode, String)> {
    // Verify authentication (any user can list libraries)
    let (_, _, _, token) = parse_emby_auth_header(&headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    let libraries: Vec<Library> = sqlx::query_as("SELECT * FROM libraries")
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let folders: Vec<VirtualFolderInfo> = libraries
        .into_iter()
        .map(|lib| VirtualFolderInfo {
            name: lib.name,
            locations: vec![lib.path],
            collection_type: Some(lib.library_type),
            library_options: LibraryOptions::default(),
            item_id: lib.id,
            primary_image_item_id: None,
            refresh_status: "Idle".to_string(),
        })
        .collect();

    Ok(Json(folders))
}

async fn add_virtual_folder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<AddVirtualFolderQuery>,
    body: Option<Json<AddVirtualFolderBody>>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers).await?;

    let id = Uuid::new_v4().to_string();
    let collection_type = query
        .collection_type
        .unwrap_or_else(|| "movies".to_string());

    // Get path from query params or use a default
    let path = query.paths.unwrap_or_default();

    sqlx::query("INSERT INTO libraries (id, name, path, library_type) VALUES (?, ?, ?, ?)")
        .bind(&id)
        .bind(&query.name)
        .bind(&path)
        .bind(&collection_type)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::info!("Created library '{}' at path '{}'", query.name, path);

    // Trigger a library scan for the newly added library
    let should_refresh = query.refresh_library.unwrap_or(true);
    if should_refresh && !path.is_empty() {
        let pool = state.db.clone();
        let library_id = id.clone();
        let library_path = path.clone();
        let library_type = collection_type.clone();
        let cache_dir = state.config.paths.cache_dir.clone();
        let anime_db_enabled = state.config.anime_db_enabled;

        let fetch_episode_metadata = state.config.fetch_episode_metadata;
        tokio::spawn(async move {
            tracing::info!(
                "Starting automatic scan for new library '{}' at '{}'",
                library_id,
                library_path
            );
            if let Err(e) = scanner::scan_library_with_cache_dir(
                &pool,
                &library_id,
                &library_path,
                &library_type,
                cache_dir,
                Some(anime_db_enabled),
                Some(fetch_episode_metadata),
            )
            .await
            {
                tracing::error!("Library scan failed for '{}': {}", library_id, e);
            } else {
                tracing::info!("Library scan completed for '{}'", library_id);
            }
        });
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteVirtualFolderQuery {
    pub name: String,
}

async fn remove_virtual_folder(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    axum::extract::Query(query): axum::extract::Query<DeleteVirtualFolderQuery>,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers).await?;

    tracing::debug!("Deleting library with name: '{}'", query.name);

    let result = sqlx::query("DELETE FROM libraries WHERE name = ?")
        .bind(&query.name)
        .execute(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    tracing::debug!("Rows affected: {}", result.rows_affected());

    if result.rows_affected() == 0 {
        return Err((StatusCode::NOT_FOUND, "Library not found".to_string()));
    }

    tracing::info!("Deleted library '{}'", query.name);

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateLibraryOptionsRequest {
    pub id: String,
    pub library_options: LibraryOptions,
}

async fn update_library_options(
    State(_state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(_req): Json<UpdateLibraryOptionsRequest>,
) -> Result<StatusCode, (StatusCode, String)> {
    // For now, just validate auth and return success
    // Library options aren't stored in DB yet
    let (_, _, _, token) = parse_emby_auth_header(&headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let _token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    Ok(StatusCode::NO_CONTENT)
}

async fn refresh_library(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers).await?;

    tracing::info!("Starting library refresh...");

    // Spawn the scan in a background task so we don't block the response
    let pool = state.db.clone();
    let cache_dir = state.config.paths.cache_dir.clone();
    let anime_db_enabled = state.config.anime_db_enabled;
    let fetch_episode_metadata = state.config.fetch_episode_metadata;

    tokio::spawn(async move {
        if let Err(e) = scanner::refresh_all_libraries_with_settings(
            &pool,
            cache_dir,
            Some(anime_db_enabled),
            Some(fetch_episode_metadata),
        )
        .await
        {
            tracing::error!("Library refresh failed: {}", e);
        }
        // Also update any items missing media info
        if let Err(e) = scanner::update_missing_media_info(&pool).await {
            tracing::error!("Media info update failed: {}", e);
        }
    });

    Ok(StatusCode::NO_CONTENT)
}
