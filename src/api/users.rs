use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{services::auth, AppState};

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/AuthenticateByName", post(authenticate_by_name))
        .route("/", get(get_users))
        .route("/Public", get(get_public_users))
        .route("/Me", get(get_current_user))
}

/// User image routes - mounted at /Users/:userId/Images
pub fn user_image_routes() -> Router<Arc<AppState>> {
    Router::new().route("/:image_type", get(get_user_image))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateRequest {
    pub username: String,
    pub pw: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    pub user: UserDto,
    pub session_info: SessionInfo,
    pub access_token: String,
    pub server_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub id: String,
    pub name: String,
    pub server_id: String,
    pub has_password: bool,
    pub has_configured_password: bool,
    pub enable_auto_login: bool,
    pub policy: UserPolicy,
    pub configuration: UserConfiguration,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserPolicy {
    pub is_administrator: bool,
    pub is_hidden: bool,
    pub is_disabled: bool,
    pub enable_all_folders: bool,
    pub enable_audio_playback_transcoding: bool,
    pub enable_video_playback_transcoding: bool,
    pub enable_playback_remuxing: bool,
    pub enable_media_conversion: bool,
    pub authentication_provider_id: String,
    pub password_reset_provider_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserConfiguration {
    pub play_default_audio_track: bool,
    pub subtitle_language_preference: String,
    pub display_missing_episodes: bool,
    pub subtitle_mode: String,
    pub enable_local_password: bool,
    pub hide_played_in_latest: bool,
    pub remember_audio_selections: bool,
    pub remember_subtitle_selections: bool,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfo {
    pub id: String,
    pub user_id: String,
    pub user_name: String,
    pub client: String,
    pub device_name: String,
    pub device_id: String,
}

impl Default for UserPolicy {
    fn default() -> Self {
        Self {
            is_administrator: false,
            is_hidden: false,
            is_disabled: false,
            enable_all_folders: true,
            enable_audio_playback_transcoding: false,
            enable_video_playback_transcoding: false,
            enable_playback_remuxing: true,
            enable_media_conversion: false,
            authentication_provider_id:
                "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider".to_string(),
            password_reset_provider_id:
                "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider".to_string(),
        }
    }
}

impl Default for UserConfiguration {
    fn default() -> Self {
        Self {
            play_default_audio_track: true,
            subtitle_language_preference: String::new(),
            display_missing_episodes: false,
            subtitle_mode: "Default".to_string(),
            enable_local_password: false,
            hide_played_in_latest: true,
            remember_audio_selections: true,
            remember_subtitle_selections: true,
        }
    }
}

/// Parse the X-Emby-Authorization header
/// Format: MediaBrowser Client="...", Device="...", DeviceId="...", Version="...", Token="..."
pub fn parse_emby_auth_header(
    headers: &HeaderMap,
) -> Option<(String, String, String, Option<String>)> {
    let auth_header = headers
        .get("X-Emby-Authorization")
        .or_else(|| headers.get("Authorization"))?
        .to_str()
        .ok()?;

    let mut client = String::new();
    let mut device = String::new();
    let mut device_id = String::new();
    let mut token = None;

    // Remove "MediaBrowser " or "Emby " prefix
    let params = auth_header
        .strip_prefix("MediaBrowser ")
        .or_else(|| auth_header.strip_prefix("Emby "))
        .unwrap_or(auth_header);

    for part in params.split(',') {
        let part = part.trim();
        if let Some((key, value)) = part.split_once('=') {
            let value = value.trim_matches('"');
            match key.trim() {
                "Client" => client = value.to_string(),
                "Device" => device = value.to_string(),
                "DeviceId" => device_id = value.to_string(),
                "Token" => token = Some(value.to_string()),
                _ => {}
            }
        }
    }

    Some((client, device, device_id, token))
}

async fn authenticate_by_name(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(req): Json<AuthenticateRequest>,
) -> Result<Json<AuthenticationResult>, (StatusCode, String)> {
    let (client, device_name, device_id, _) =
        parse_emby_auth_header(&headers).unwrap_or_else(|| {
            (
                "Unknown".to_string(),
                "Unknown".to_string(),
                "unknown".to_string(),
                None,
            )
        });

    let (user, session) = auth::authenticate(
        &state.db,
        &req.username,
        &req.pw,
        &device_id,
        &device_name,
        &client,
    )
    .await
    .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    let user_dto = UserDto {
        id: user.id.clone(),
        name: user.name.clone(),
        server_id: "jellyfin-rust-server".to_string(),
        has_password: true,
        has_configured_password: true,
        enable_auto_login: false,
        policy: UserPolicy {
            is_administrator: user.is_admin,
            ..Default::default()
        },
        configuration: UserConfiguration::default(),
    };

    let session_info = SessionInfo {
        id: session.token.clone(),
        user_id: user.id,
        user_name: user.name,
        client: session.client,
        device_name: session.device_name,
        device_id: session.device_id,
    };

    Ok(Json(AuthenticationResult {
        user: user_dto,
        session_info,
        access_token: session.token,
        server_id: "jellyfin-rust-server".to_string(),
    }))
}

async fn get_users(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<Vec<UserDto>>, (StatusCode, String)> {
    // Verify authentication
    let (_, _, _, token) = parse_emby_auth_header(&headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    let users: Vec<crate::models::User> = sqlx::query_as("SELECT * FROM users")
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let user_dtos: Vec<UserDto> = users
        .into_iter()
        .map(|u| UserDto {
            id: u.id,
            name: u.name,
            server_id: "jellyfin-rust-server".to_string(),
            has_password: true,
            has_configured_password: true,
            enable_auto_login: false,
            policy: UserPolicy {
                is_administrator: u.is_admin,
                ..Default::default()
            },
            configuration: UserConfiguration::default(),
        })
        .collect();

    Ok(Json(user_dtos))
}

async fn get_public_users(
    State(state): State<Arc<AppState>>,
) -> Result<Json<Vec<PublicUserDto>>, (StatusCode, String)> {
    let users: Vec<crate::models::User> = sqlx::query_as("SELECT * FROM users")
        .fetch_all(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let public_users: Vec<PublicUserDto> = users
        .into_iter()
        .map(|u| PublicUserDto {
            id: u.id,
            name: u.name,
            has_password: true,
            has_configured_password: true,
        })
        .collect();

    Ok(Json(public_users))
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PublicUserDto {
    pub id: String,
    pub name: String,
    pub has_password: bool,
    pub has_configured_password: bool,
}

async fn get_current_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<UserDto>, (StatusCode, String)> {
    let (_, _, _, token) = parse_emby_auth_header(&headers)
        .ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing auth header".to_string()))?;

    let token = token.ok_or_else(|| (StatusCode::UNAUTHORIZED, "Missing token".to_string()))?;

    let user = auth::validate_session(&state.db, &token)
        .await
        .map_err(|e| (StatusCode::UNAUTHORIZED, e.to_string()))?;

    Ok(Json(UserDto {
        id: user.id,
        name: user.name,
        server_id: "jellyfin-rust-server".to_string(),
        has_password: true,
        has_configured_password: true,
        enable_auto_login: false,
        policy: UserPolicy {
            is_administrator: user.is_admin,
            ..Default::default()
        },
        configuration: UserConfiguration::default(),
    }))
}

/// GET /Users/:userId/Images/:imageType - Get user image
/// Since we don't support user images, always return 404
async fn get_user_image(Path((_user_id, _image_type)): Path<(String, String)>) -> StatusCode {
    // User images not supported - return 404
    // Client handles this gracefully and shows default avatar
    StatusCode::NOT_FOUND
}
