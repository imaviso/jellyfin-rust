use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{services::auth, AppState};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/Info", get(get_system_info))
        .route("/Info/Public", get(get_public_system_info))
        .route("/Info/Storage", get(get_storage_info))
        .route("/Configuration", get(get_configuration))
        .route("/Restart", post(restart_server))
        .route("/Shutdown", post(shutdown_server))
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub server_name: String,
    pub version: String,
    pub id: String,
    pub operating_system: String,
    pub has_pending_restart: bool,
    pub has_update_available: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicSystemInfo {
    pub server_name: String,
    pub version: String,
    pub id: String,
    pub local_address: String,
    pub startup_wizard_completed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct ServerConfiguration {
    pub enable_slow_response_warning: bool,
    pub slow_response_threshold_ms: i64,
    pub enable_dashboard: bool,
    pub enable_https: bool,
    pub enable_normalized_item_by_name_ids: bool,
    pub is_port_authorized: bool,
    pub quick_connect_available: bool,
    pub enable_case_sensitive_item_ids: bool,
    pub disable_live_tv_channel_user_data_name: bool,
    pub metadata_country_code: String,
    pub preferred_metadata_language: String,
    pub sort_remove_characters: Vec<String>,
    pub sort_replace_characters: Vec<String>,
    pub library_scan_fanout_concurrency: i32,
    pub enable_external_content_in_suggestions: bool,
}

async fn get_system_info() -> Json<SystemInfo> {
    Json(SystemInfo {
        server_name: "Jellyfin Rust".to_string(),
        version: "10.11.5".to_string(), // Mimic Jellyfin version for client compat
        id: "jellyfin-rust-server".to_string(),
        operating_system: std::env::consts::OS.to_string(),
        has_pending_restart: false,
        has_update_available: false,
    })
}

async fn get_public_system_info() -> Json<PublicSystemInfo> {
    Json(PublicSystemInfo {
        server_name: "Jellyfin Rust".to_string(),
        version: "10.11.5".to_string(),
        id: "jellyfin-rust-server".to_string(),
        local_address: "http://localhost:8096".to_string(),
        startup_wizard_completed: true,
    })
}

async fn get_configuration() -> Json<ServerConfiguration> {
    Json(ServerConfiguration {
        enable_slow_response_warning: true,
        slow_response_threshold_ms: 500,
        enable_dashboard: true,
        enable_https: false,
        enable_normalized_item_by_name_ids: true,
        is_port_authorized: true,
        quick_connect_available: false,
        enable_case_sensitive_item_ids: true,
        disable_live_tv_channel_user_data_name: true,
        metadata_country_code: "US".to_string(),
        preferred_metadata_language: "en".to_string(),
        sort_remove_characters: vec!["\"".to_string(), "'".to_string()],
        sort_replace_characters: vec![".".to_string(), "+".to_string(), "%".to_string()],
        library_scan_fanout_concurrency: 0,
        enable_external_content_in_suggestions: true,
    })
}

// =============================================================================
// Storage Info
// =============================================================================

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct FolderStorageDto {
    pub path: String,
    pub free_space: i64,
    pub used_space: i64,
    pub storage_type: String,
    pub device_id: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryStorageDto {
    pub name: String,
    pub path: String,
    pub free_space: i64,
    pub used_space: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SystemStorageDto {
    pub program_data_folder: Option<FolderStorageDto>,
    pub cache_folder: Option<FolderStorageDto>,
    pub log_folder: Option<FolderStorageDto>,
    pub libraries: Vec<LibraryStorageDto>,
}

/// Get disk usage info for a path
fn get_folder_storage(path: &std::path::Path) -> Option<FolderStorageDto> {
    use std::process::Command;

    // Use df command to get disk usage
    let output = Command::new("df")
        .arg("-B1") // bytes
        .arg(path)
        .output()
        .ok()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() < 2 {
        return None;
    }

    // Parse df output: Filesystem 1B-blocks Used Available Use% Mounted
    let parts: Vec<&str> = lines[1].split_whitespace().collect();
    if parts.len() < 4 {
        return None;
    }

    let total: i64 = parts[1].parse().unwrap_or(0);
    let used: i64 = parts[2].parse().unwrap_or(0);
    let available: i64 = parts[3].parse().unwrap_or(0);

    Some(FolderStorageDto {
        path: path.to_string_lossy().to_string(),
        free_space: available,
        used_space: used,
        storage_type: "Local".to_string(),
        device_id: Some(parts[0].to_string()),
    })
}

/// GET /System/Info/Storage - Get storage information
async fn get_storage_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<SystemStorageDto>, (StatusCode, String)> {
    require_admin(&state, &headers).await?;

    // Get storage info for data directory
    let data_folder = get_folder_storage(&state.config.paths.data_dir);
    let cache_folder = get_folder_storage(&state.config.paths.cache_dir);

    // Get library storage info
    let libraries: Vec<crate::models::Library> = sqlx::query_as("SELECT * FROM libraries")
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let library_storage: Vec<LibraryStorageDto> = libraries
        .iter()
        .filter_map(|lib| {
            let path = std::path::Path::new(&lib.path);
            get_folder_storage(path).map(|storage| LibraryStorageDto {
                name: lib.name.clone(),
                path: lib.path.clone(),
                free_space: storage.free_space,
                used_space: storage.used_space,
            })
        })
        .collect();

    Ok(Json(SystemStorageDto {
        program_data_folder: data_folder,
        cache_folder,
        log_folder: None,
        libraries: library_storage,
    }))
}

/// Helper to require admin authentication
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

/// POST /System/Restart - Restart the server
///
/// This sends a 204 response and then triggers a process restart.
/// Since we can't truly restart ourselves, we exit with code 0 and rely on
/// a process manager (systemd, docker, etc.) to restart us.
async fn restart_server(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers).await?;

    tracing::info!("Server restart requested by admin");

    // Spawn a task to exit after a brief delay (allows response to be sent)
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        tracing::info!("Restarting server...");
        // Exit with code 0 - process manager should restart us
        std::process::exit(0);
    });

    Ok(StatusCode::NO_CONTENT)
}

/// POST /System/Shutdown - Shutdown the server
///
/// This sends a 204 response and then triggers a graceful shutdown.
async fn shutdown_server(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<StatusCode, (StatusCode, String)> {
    require_admin(&state, &headers).await?;

    tracing::info!("Server shutdown requested by admin");

    // Spawn a task to exit after a brief delay (allows response to be sent)
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
        tracing::info!("Shutting down server...");
        // Exit with code 0 for clean shutdown
        std::process::exit(0);
    });

    Ok(StatusCode::NO_CONTENT)
}
