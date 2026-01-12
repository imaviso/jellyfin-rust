// Media info extraction using ffprobe

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;
use std::process::Command;

/// Media information extracted from a file
#[derive(Debug, Clone, Default)]
pub struct MediaInfo {
    /// Duration in ticks (1 tick = 100 nanoseconds, 10,000,000 ticks = 1 second)
    pub duration_ticks: Option<i64>,
    pub duration_seconds: Option<f64>,
    /// Video codec (e.g., "hevc", "h264")
    pub video_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Container format (e.g., "matroska", "mp4")
    pub container: Option<String>,
    pub bitrate: Option<u64>,
    pub audio_streams: Vec<AudioStream>,
    pub subtitle_streams: Vec<SubtitleStream>,
}

/// Information about an audio stream
#[derive(Debug, Clone)]
pub struct AudioStream {
    pub index: i32,
    /// Codec name (e.g., "aac", "flac", "opus")
    pub codec: String,
    /// Language code (e.g., "eng", "jpn")
    pub language: Option<String>,
    /// Title/label for the stream (e.g., "English Dub", "Japanese")
    pub title: Option<String>,
    pub channels: Option<i32>,
    pub sample_rate: Option<i32>,
    pub is_default: bool,
}

impl AudioStream {
    /// Generate a display title for this audio stream
    pub fn display_title(&self) -> String {
        let mut parts = Vec::new();

        if let Some(lang) = &self.language {
            parts.push(language_name(lang));
        }

        if let Some(title) = &self.title {
            if !title.is_empty() {
                parts.push(title.clone());
            }
        }

        // Add codec name
        let codec_name = match self.codec.as_str() {
            "aac" => "AAC",
            "ac3" => "AC3",
            "eac3" => "E-AC3",
            "dts" => "DTS",
            "flac" => "FLAC",
            "opus" => "Opus",
            "vorbis" => "Vorbis",
            "mp3" => "MP3",
            "truehd" => "TrueHD",
            "pcm_s16le" | "pcm_s24le" | "pcm_s32le" => "PCM",
            _ => &self.codec,
        };
        parts.push(codec_name.to_string());

        // Add channel info
        if let Some(ch) = self.channels {
            let channel_desc = match ch {
                1 => "Mono",
                2 => "Stereo",
                6 => "5.1",
                8 => "7.1",
                _ => "",
            };
            if !channel_desc.is_empty() {
                parts.push(channel_desc.to_string());
            }
        }

        if self.is_default {
            parts.push("Default".to_string());
        }

        parts.join(" - ")
    }
}

/// Information about a subtitle stream
#[derive(Debug, Clone)]
pub struct SubtitleStream {
    pub index: i32,
    /// Codec name (e.g., "subrip", "ass", "hdmv_pgs_subtitle")
    pub codec: String,
    /// Language code (e.g., "eng", "jpn")
    pub language: Option<String>,
    pub title: Option<String>,
    pub is_default: bool,
    pub is_forced: bool,
}

impl SubtitleStream {
    /// Check if this is a text-based subtitle (can be converted to VTT/SRT)
    pub fn is_text_based(&self) -> bool {
        let c = self.codec.to_lowercase();
        // PGS, VobSub, DVB are bitmap formats that can't be converted to text
        !c.contains("pgs")
            && !c.contains("dvdsub")
            && !c.contains("dvbsub")
            && !c.contains("dvd_subtitle")
            && c != "sup"
            && c != "sub"
            && c != "hdmv_pgs_subtitle"
    }

    /// Generate a display title for this subtitle
    pub fn display_title(&self) -> String {
        let mut parts = Vec::new();

        if let Some(lang) = &self.language {
            parts.push(language_name(lang));
        }

        if let Some(title) = &self.title {
            if !title.is_empty() {
                parts.push(title.clone());
            }
        }

        let codec_name = match self.codec.as_str() {
            "subrip" | "srt" => "SRT",
            "ass" | "ssa" => "ASS",
            "webvtt" | "vtt" => "VTT",
            "hdmv_pgs_subtitle" | "pgssub" => "PGS",
            "dvd_subtitle" | "dvdsub" => "VobSub",
            _ => &self.codec,
        };
        parts.push(codec_name.to_string());

        if self.is_default {
            parts.push("Default".to_string());
        }
        if self.is_forced {
            parts.push("Forced".to_string());
        }

        parts.join(" - ")
    }
}

/// Convert language code to human-readable name
fn language_name(code: &str) -> String {
    match code {
        "eng" | "en" => "English".to_string(),
        "jpn" | "ja" => "Japanese".to_string(),
        "spa" | "es" => "Spanish".to_string(),
        "fre" | "fra" | "fr" => "French".to_string(),
        "ger" | "deu" | "de" => "German".to_string(),
        "ita" | "it" => "Italian".to_string(),
        "por" | "pt" => "Portuguese".to_string(),
        "rus" | "ru" => "Russian".to_string(),
        "chi" | "zho" | "zh" => "Chinese".to_string(),
        "kor" | "ko" => "Korean".to_string(),
        "ara" | "ar" => "Arabic".to_string(),
        "und" => "Unknown".to_string(),
        _ => code.to_uppercase(),
    }
}

/// ffprobe JSON output structure
#[derive(Debug, Deserialize)]
struct FfprobeOutput {
    format: Option<FfprobeFormat>,
    streams: Option<Vec<FfprobeStream>>,
}

#[derive(Debug, Deserialize)]
struct FfprobeFormat {
    duration: Option<String>,
    format_name: Option<String>,
    bit_rate: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStream {
    index: Option<i32>,
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    channels: Option<i32>,
    sample_rate: Option<String>, // ffprobe returns this as a string
    tags: Option<FfprobeStreamTags>,
    disposition: Option<FfprobeDisposition>,
}

#[derive(Debug, Deserialize)]
struct FfprobeStreamTags {
    language: Option<String>,
    title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FfprobeDisposition {
    default: Option<i32>,
    forced: Option<i32>,
}

/// Find ffprobe binary - checks FFPROBE_PATH env var, then common locations
fn find_ffprobe() -> String {
    // Check environment variable first
    if let Ok(path) = std::env::var("FFPROBE_PATH") {
        return path;
    }

    // Common locations to check
    let paths = [
        "/nix/store/2v155vxx0l5ysxjpsw5hnxwjs2c5p785-ffmpeg-8.0-bin/bin/ffprobe",
        "/usr/bin/ffprobe",
        "/usr/local/bin/ffprobe",
        "/opt/homebrew/bin/ffprobe",
    ];

    for path in paths {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }

    // Fall back to PATH lookup
    "ffprobe".to_string()
}

/// Extract media information from a file using ffprobe
pub fn extract_media_info(path: &Path) -> Result<MediaInfo> {
    let ffprobe = find_ffprobe();

    let output = Command::new(&ffprobe)
        .args([
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
        ])
        .arg(path)
        .output()
        .with_context(|| {
            format!(
                "Failed to run ffprobe at '{}'. Is ffmpeg installed?",
                ffprobe
            )
        })?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("ffprobe failed: {}", stderr);
    }

    let json_output = String::from_utf8_lossy(&output.stdout);
    let probe: FfprobeOutput =
        serde_json::from_str(&json_output).context("Failed to parse ffprobe output")?;

    let mut info = MediaInfo::default();

    // Extract format info
    if let Some(format) = probe.format {
        if let Some(duration_str) = format.duration {
            if let Ok(duration) = duration_str.parse::<f64>() {
                info.duration_seconds = Some(duration);
                // Convert to ticks (1 second = 10,000,000 ticks)
                info.duration_ticks = Some((duration * 10_000_000.0) as i64);
            }
        }
        info.container = format.format_name;
        if let Some(bitrate_str) = format.bit_rate {
            info.bitrate = bitrate_str.parse().ok();
        }
    }

    // Extract stream info
    if let Some(streams) = probe.streams {
        for stream in streams {
            match stream.codec_type.as_deref() {
                Some("video") => {
                    if info.video_codec.is_none() {
                        info.video_codec = stream.codec_name;
                        info.width = stream.width;
                        info.height = stream.height;
                    }
                }
                Some("audio") => {
                    if let (Some(index), Some(codec)) = (stream.index, stream.codec_name) {
                        let is_default = stream
                            .disposition
                            .as_ref()
                            .and_then(|d| d.default)
                            .map(|v| v == 1)
                            .unwrap_or(false);

                        info.audio_streams.push(AudioStream {
                            index,
                            codec,
                            language: stream.tags.as_ref().and_then(|t| t.language.clone()),
                            title: stream.tags.as_ref().and_then(|t| t.title.clone()),
                            channels: stream.channels,
                            sample_rate: stream.sample_rate.as_ref().and_then(|s| s.parse().ok()),
                            is_default,
                        });
                    }
                }
                Some("subtitle") => {
                    if let (Some(index), Some(codec)) = (stream.index, stream.codec_name) {
                        let is_default = stream
                            .disposition
                            .as_ref()
                            .and_then(|d| d.default)
                            .map(|v| v == 1)
                            .unwrap_or(false);
                        let is_forced = stream
                            .disposition
                            .as_ref()
                            .and_then(|d| d.forced)
                            .map(|v| v == 1)
                            .unwrap_or(false);

                        info.subtitle_streams.push(SubtitleStream {
                            index,
                            codec,
                            language: stream.tags.as_ref().and_then(|t| t.language.clone()),
                            title: stream.tags.as_ref().and_then(|t| t.title.clone()),
                            is_default,
                            is_forced,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Ok(info)
}

/// Extract media info asynchronously (runs ffprobe in blocking task)
pub async fn extract_media_info_async(path: &Path) -> Result<MediaInfo> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || extract_media_info(&path))
        .await
        .context("Task join error")?
}

/// Format duration ticks as human-readable string (HH:MM:SS)
pub fn format_duration(ticks: i64) -> String {
    let total_seconds = ticks / 10_000_000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    } else {
        format!("{:02}:{:02}", minutes, seconds)
    }
}

/// Find ffmpeg binary - checks FFMPEG_PATH env var, then common locations
fn find_ffmpeg() -> String {
    // Check environment variable first
    if let Ok(path) = std::env::var("FFMPEG_PATH") {
        return path;
    }

    // Common locations to check
    let paths = [
        "/nix/store/2v155vxx0l5ysxjpsw5hnxwjs2c5p785-ffmpeg-8.0-bin/bin/ffmpeg",
        "/usr/bin/ffmpeg",
        "/usr/local/bin/ffmpeg",
        "/opt/homebrew/bin/ffmpeg",
    ];

    for path in paths {
        if std::path::Path::new(path).exists() {
            return path.to_string();
        }
    }

    // Fall back to PATH lookup
    "ffmpeg".to_string()
}

/// Extract a thumbnail from a video file at the specified timestamp
///
/// # Arguments
/// * `video_path` - Path to the video file
/// * `output_path` - Path where the thumbnail should be saved
/// * `timestamp_seconds` - Position in video to extract frame (in seconds)
/// * `width` - Optional max width (maintains aspect ratio)
///
/// # Returns
/// * `Ok(())` if successful
/// * `Err` if ffmpeg fails
pub fn extract_thumbnail(
    video_path: &Path,
    output_path: &Path,
    timestamp_seconds: f64,
    width: Option<u32>,
) -> Result<()> {
    let ffmpeg = find_ffmpeg();

    // Create parent directory if it doesn't exist
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Build filter string for scaling
    let scale_filter = match width {
        Some(w) => format!("scale={}:-1", w),
        None => "scale=320:-1".to_string(), // Default to 320px wide
    };

    // Try fast seeking first (-ss before -i)
    // This is much faster as it seeks by keyframes without decoding
    let output = Command::new(&ffmpeg)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-ss",
            &format!("{:.3}", timestamp_seconds),
            "-i",
        ])
        .arg(video_path)
        .args(["-vframes", "1", "-vf", &scale_filter, "-q:v", "5", "-y"])
        .arg(output_path)
        .output()
        .with_context(|| format!("Failed to run ffmpeg at '{}'. Is ffmpeg installed?", ffmpeg))?;

    // If fast seek failed, try slow seek (more reliable for problematic files)
    if !output.status.success() || !output_path.exists() {
        let output = Command::new(&ffmpeg)
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(video_path)
            .args([
                "-ss",
                &format!("{:.3}", timestamp_seconds),
                "-vframes",
                "1",
                "-vf",
                &scale_filter,
                "-q:v",
                "5",
                "-y",
            ])
            .arg(output_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("ffmpeg thumbnail extraction failed: {}", stderr);
        }
    }

    // Verify the output file was created
    if !output_path.exists() {
        anyhow::bail!("Thumbnail was not created at {:?}", output_path);
    }

    Ok(())
}

/// Extract a thumbnail asynchronously
pub async fn extract_thumbnail_async(
    video_path: &Path,
    output_path: &Path,
    timestamp_seconds: f64,
    width: Option<u32>,
) -> Result<()> {
    let video_path = video_path.to_path_buf();
    let output_path = output_path.to_path_buf();

    tokio::task::spawn_blocking(move || {
        extract_thumbnail(&video_path, &output_path, timestamp_seconds, width)
    })
    .await
    .context("Task join error")?
}

/// Calculate a good timestamp for thumbnail extraction
/// Uses ~10% into the video to avoid intros/black screens
pub fn calculate_thumbnail_timestamp(duration_seconds: f64) -> f64 {
    // Use 10% into the video, but at least 5 seconds and at most 5 minutes
    let timestamp = duration_seconds * 0.10;
    timestamp
        .clamp(5.0, 300.0)
        .min(duration_seconds - 1.0)
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        // 1 hour, 30 minutes, 45 seconds
        let ticks = (1 * 3600 + 30 * 60 + 45) * 10_000_000i64;
        assert_eq!(format_duration(ticks), "01:30:45");

        // 5 minutes, 30 seconds
        let ticks = (5 * 60 + 30) * 10_000_000i64;
        assert_eq!(format_duration(ticks), "05:30");
    }

    #[test]
    fn test_calculate_thumbnail_timestamp() {
        // 24 minute episode -> ~2.4 minutes = 144 seconds
        assert!((calculate_thumbnail_timestamp(1440.0) - 144.0).abs() < 0.1);

        // Very short video (30 sec) -> use near start
        assert!(calculate_thumbnail_timestamp(30.0) < 30.0);

        // Very long video (2 hours) -> cap at 5 minutes
        assert!((calculate_thumbnail_timestamp(7200.0) - 300.0).abs() < 0.1);
    }
}
