// PlaybackInfo endpoint - provides media source information for clients

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

use crate::{
    models::MediaItem,
    services::{auth, mediainfo},
    AppState,
};

use super::users::parse_emby_auth_header;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/:id/PlaybackInfo", get(get_playback_info))
        .route("/:id/PlaybackInfo", post(get_playback_info))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoQuery {
    pub user_id: Option<String>,
    pub max_streaming_bitrate: Option<i64>,
    pub start_time_ticks: Option<i64>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub max_audio_channels: Option<i32>,
    pub media_source_id: Option<String>,
    pub live_stream_id: Option<String>,
    pub auto_open_live_stream: Option<bool>,
    pub enable_direct_play: Option<bool>,
    pub enable_direct_stream: Option<bool>,
    pub enable_transcoding: Option<bool>,
    pub allow_video_stream_copy: Option<bool>,
    pub allow_audio_stream_copy: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    pub media_sources: Vec<MediaSourceInfo>,
    pub play_session_id: String,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceInfo {
    pub id: String,
    pub name: String,
    pub path: Option<String>,
    pub protocol: String, // "File", "Http", etc.
    pub container: Option<String>,
    pub size: Option<i64>,
    pub bitrate: Option<i64>,
    pub runtime_ticks: Option<i64>,

    #[serde(rename = "Type")]
    pub source_type: String, // "Default", "Grouping", "Placeholder"

    pub is_remote: bool,
    pub read_at_native_framerate: bool,
    pub supports_transcoding: bool,
    pub supports_direct_stream: bool,
    pub supports_direct_play: bool,
    pub is_infinite_stream: bool,
    pub requires_opening: bool,
    pub requires_closing: bool,
    pub requires_looping: bool,
    pub supports_probing: bool,

    pub media_streams: Vec<MediaStreamInfo>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub direct_stream_url: Option<String>,

    // Transcoding info (we don't support but clients may expect these)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcoding_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcoding_sub_protocol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transcoding_container: Option<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStreamInfo {
    #[serde(rename = "Type")]
    pub stream_type: String, // "Video", "Audio", "Subtitle"

    pub codec: Option<String>,
    pub index: i32,
    pub is_default: bool,
    pub is_forced: bool,
    pub is_external: bool,

    // Video specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_rate: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspect_ratio: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub average_frame_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub real_frame_rate: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_range: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_range_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pixel_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,

    // Audio specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channels: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_layout: Option<String>,

    // Subtitle specific
    #[serde(skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub display_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_method: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_text_subtitle_stream: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_external_stream: Option<bool>,
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

async fn get_playback_info(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(item_id): Path<String>,
    Query(_query): Query<PlaybackInfoQuery>,
) -> Result<Json<PlaybackInfoResponse>, (StatusCode, String)> {
    let _user = require_auth(&state, &headers).await?;

    // Get the media item
    let item: MediaItem = sqlx::query_as("SELECT * FROM media_items WHERE id = ?")
        .bind(&item_id)
        .fetch_optional(&state.db)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item not found".to_string()))?;

    // Get the file path
    let file_path = item
        .path
        .as_ref()
        .ok_or_else(|| (StatusCode::NOT_FOUND, "Item has no file path".to_string()))?;

    // Get file size
    let file_size = tokio::fs::metadata(file_path)
        .await
        .ok()
        .map(|m| m.len() as i64);

    // Extract detailed media info using ffprobe
    let media_info = mediainfo::extract_media_info_async(std::path::Path::new(file_path))
        .await
        .ok();

    // Build media streams from ffprobe output
    let mut media_streams = Vec::new();

    // Add video stream
    if let Some(ref info) = media_info {
        if info.video_codec.is_some() {
            media_streams.push(MediaStreamInfo {
                stream_type: "Video".to_string(),
                codec: info.video_codec.clone(),
                index: 0,
                is_default: true,
                is_forced: false,
                is_external: false,
                width: info.width,
                height: info.height,
                bit_rate: info.bitrate.map(|b| b as i64),
                aspect_ratio: info
                    .width
                    .zip(info.height)
                    .map(|(w, h)| format!("{}:{}", w, h)),
                average_frame_rate: None,
                real_frame_rate: None,
                video_range: Some("SDR".to_string()), // Default, could be detected
                video_range_type: Some("SDR".to_string()),
                pixel_format: None,
                level: None,
                profile: None,
                channels: None,
                sample_rate: None,
                channel_layout: None,
                language: None,
                title: None,
                display_title: info.video_codec.as_ref().map(|c| {
                    if let (Some(w), Some(h)) = (info.width, info.height) {
                        format!("{} - {}x{}", c.to_uppercase(), w, h)
                    } else {
                        c.to_uppercase()
                    }
                }),
                delivery_method: None,
                delivery_url: None,
                is_text_subtitle_stream: None,
                supports_external_stream: None,
            });
        }

        // Add audio streams (supports multiple tracks)
        for audio in &info.audio_streams {
            let channel_layout = audio.channels.map(|ch| match ch {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                6 => "5.1".to_string(),
                8 => "7.1".to_string(),
                _ => format!("{} channels", ch),
            });

            media_streams.push(MediaStreamInfo {
                stream_type: "Audio".to_string(),
                codec: Some(audio.codec.clone()),
                index: audio.index,
                is_default: audio.is_default,
                is_forced: false,
                is_external: false,
                width: None,
                height: None,
                bit_rate: None,
                aspect_ratio: None,
                average_frame_rate: None,
                real_frame_rate: None,
                video_range: None,
                video_range_type: None,
                pixel_format: None,
                level: None,
                profile: None,
                channels: audio.channels,
                sample_rate: audio.sample_rate,
                channel_layout,
                language: audio.language.clone(),
                title: audio.title.clone(),
                display_title: Some(audio.display_title()),
                delivery_method: None,
                delivery_url: None,
                is_text_subtitle_stream: None,
                supports_external_stream: None,
            });
        }

        // Add subtitle streams
        for sub in &info.subtitle_streams {
            let is_text = sub.is_text_based();
            // Use native format extension for the delivery URL
            let format_ext = match sub.codec.as_str() {
                "ass" | "ssa" => "ass",
                "subrip" | "srt" => "srt",
                "webvtt" | "vtt" => "vtt",
                _ => "srt", // fallback
            };
            media_streams.push(MediaStreamInfo {
                stream_type: "Subtitle".to_string(),
                codec: Some(sub.codec.clone()),
                index: sub.index,
                is_default: sub.is_default,
                is_forced: sub.is_forced,
                is_external: false,
                width: None,
                height: None,
                bit_rate: None,
                aspect_ratio: None,
                average_frame_rate: None,
                real_frame_rate: None,
                video_range: None,
                video_range_type: None,
                pixel_format: None,
                level: None,
                profile: None,
                channels: None,
                sample_rate: None,
                channel_layout: None,
                language: sub.language.clone(),
                title: sub.title.clone(),
                display_title: Some(sub.display_title()),
                delivery_method: if is_text {
                    Some("External".to_string())
                } else {
                    Some("Embed".to_string()) // Bitmap subs need to be embedded/burned
                },
                delivery_url: if is_text {
                    Some(format!(
                        "/Videos/{}/{}/Subtitles/{}/0/Stream.{}",
                        item.id, item.id, sub.index, format_ext
                    ))
                } else {
                    None
                },
                is_text_subtitle_stream: Some(is_text),
                supports_external_stream: Some(is_text),
            });
        }
    }

    // Determine container from path
    let container = file_path.rsplit('.').next().map(|s| s.to_lowercase());

    let media_source = MediaSourceInfo {
        id: item.id.clone(),
        name: item.name.clone(),
        path: item.path.clone(),
        protocol: "File".to_string(),
        container,
        size: file_size,
        bitrate: media_info
            .as_ref()
            .and_then(|i| i.bitrate.map(|b| b as i64)),
        runtime_ticks: item
            .runtime_ticks
            .or_else(|| media_info.as_ref().and_then(|i| i.duration_ticks)),
        source_type: "Default".to_string(),
        is_remote: false,
        read_at_native_framerate: false,
        supports_transcoding: false, // We don't support transcoding
        supports_direct_stream: true,
        supports_direct_play: true,
        is_infinite_stream: false,
        requires_opening: false,
        requires_closing: false,
        requires_looping: false,
        supports_probing: true,
        media_streams,
        direct_stream_url: Some(format!("/Videos/{}/stream", item.id)),
        transcoding_url: None,
        transcoding_sub_protocol: None,
        transcoding_container: None,
    };

    // Generate a play session ID
    let play_session_id = uuid::Uuid::new_v4().to_string().replace("-", "");

    Ok(Json(PlaybackInfoResponse {
        media_sources: vec![media_source],
        play_session_id,
    }))
}
