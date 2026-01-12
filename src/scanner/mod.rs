use anyhow::Result;
use futures::{stream, StreamExt};
use regex::Regex;
use sqlx::SqlitePool;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tokio::fs;
use uuid::Uuid;

use crate::api::filters::{
    get_or_create_genre, get_or_create_person, get_or_create_studio, link_item_genre,
    link_item_person, link_item_studio,
};
use crate::services::mediainfo;
use crate::services::metadata::{MetadataService, UnifiedMetadata};

/// Concurrency limit for parallel operations (metadata fetch, ffprobe, etc.)
const SCAN_CONCURRENCY: usize = 4;

/// Batch size for database inserts
const DB_BATCH_SIZE: usize = 50;

/// Information about a discovered video file for batch processing
#[derive(Debug, Clone)]
struct DiscoveredEpisode {
    path: PathBuf,
    parsed: ParsedEpisode,
}

/// Information about a discovered movie file for batch processing
#[derive(Debug, Clone)]
struct DiscoveredMovie {
    path: PathBuf,
    parsed: ParsedMovie,
}

/// Collected media info for an episode (after parallel ffprobe)
#[derive(Debug)]
struct EpisodeMediaInfo {
    path: PathBuf,
    parsed: ParsedEpisode,
    runtime_ticks: Option<i64>,
}

/// Collected media info for a movie (after parallel ffprobe)
#[derive(Debug)]
struct MovieMediaInfo {
    path: PathBuf,
    parsed: ParsedMovie,
    runtime_ticks: Option<i64>,
}

/// Recursively collect all video files in a directory, with symlink loop protection
async fn collect_video_files(path: &Path, visited: &mut HashSet<PathBuf>) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();

    // Canonicalize path to detect symlink loops
    let canonical = match tokio::fs::canonicalize(path).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("Cannot canonicalize path {:?}: {}", path, e);
            return Ok(files);
        }
    };

    // Check for symlink loop
    if !visited.insert(canonical.clone()) {
        tracing::warn!("Symlink loop detected, skipping: {:?}", path);
        return Ok(files);
    }

    let mut entries = match fs::read_dir(path).await {
        Ok(e) => e,
        Err(e) => {
            tracing::warn!("Cannot read directory {:?}: {}", path, e);
            return Ok(files);
        }
    };

    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();

        if entry_path.is_file() && is_video_file(&entry_path) {
            files.push(entry_path);
        } else if entry_path.is_dir() {
            let folder_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            // Skip special folders
            if should_skip_folder(folder_name) {
                continue;
            }

            // Check for .ignore file
            if should_ignore_path(&entry_path).await {
                continue;
            }

            // Recursively collect from subdirectory
            let mut sub_files = Box::pin(collect_video_files(&entry_path, visited)).await?;
            files.append(&mut sub_files);
        }
    }

    Ok(files)
}

/// Extract media info for multiple files in parallel
async fn parallel_extract_media_info(
    files: Vec<(PathBuf, ParsedEpisode)>,
) -> Vec<EpisodeMediaInfo> {
    stream::iter(files)
        .map(|(path, parsed)| async move {
            let runtime_ticks = match mediainfo::extract_media_info_async(&path).await {
                Ok(info) => info.duration_ticks,
                Err(e) => {
                    tracing::debug!("Failed to extract media info for {:?}: {}", path, e);
                    None
                }
            };
            EpisodeMediaInfo {
                path,
                parsed,
                runtime_ticks,
            }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .collect()
        .await
}

/// Extract media info for multiple movie files in parallel
async fn parallel_extract_movie_info(files: Vec<(PathBuf, ParsedMovie)>) -> Vec<MovieMediaInfo> {
    stream::iter(files)
        .map(|(path, parsed)| async move {
            let runtime_ticks = match mediainfo::extract_media_info_async(&path).await {
                Ok(info) => info.duration_ticks,
                Err(e) => {
                    tracing::debug!("Failed to extract media info for {:?}: {}", path, e);
                    None
                }
            };
            MovieMediaInfo {
                path,
                parsed,
                runtime_ticks,
            }
        })
        .buffer_unordered(SCAN_CONCURRENCY)
        .collect()
        .await
}

/// Default video extensions (used when config is not available)
pub const DEFAULT_VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "wmv", "flv", "webm", "m4v", "mpg", "mpeg", "ts", "m2ts", "mts",
    "vob", "ogm", "ogv", "divx", "xvid", "rmvb", "rm", "asf", "3gp", "3g2", "f4v",
];

/// Thread-local storage for configured video extensions
/// This allows the scanner to use custom extensions without passing them through every function
use std::sync::OnceLock;
static CONFIGURED_EXTENSIONS: OnceLock<Vec<String>> = OnceLock::new();

/// Set the video extensions to use for scanning
pub fn set_video_extensions(extensions: Vec<String>) {
    let _ = CONFIGURED_EXTENSIONS.set(extensions);
}

/// Get the configured video extensions, or fall back to defaults
fn get_video_extensions() -> &'static [String] {
    CONFIGURED_EXTENSIONS
        .get()
        .map(|v| v.as_slice())
        .unwrap_or(&[])
}
static RE_SEASON_EP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[Ss](\d{1,2})[Ee](\d{1,3})").unwrap());
static RE_ALT_EP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[\s\-])[Ee]?(\d{1,2})[Ee](\d{1,3})(?:\s|[\[\(]|$)").unwrap()
});
static RE_ANIME_EP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\s\-]+[Ee]?(\d{1,3})(?:\s*[\[\(]|$)").unwrap());
static RE_GROUP_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[.*?\]\s*[\-]?\s*").unwrap());
static RE_RELEASE_INFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\s*(1080p|720p|480p|2160p|4k|bluray|blu-ray|webrip|web-dl|hdtv|dvdrip|bdrip|x264|x265|h\.?264|h\.?265|hevc|avc|aac|opus|flac|dts|atmos|10bit|hdr|sdr|remux|proper|repack|multi|dual|dubbed|subbed|raw|opus2|aac2|batch|dvd9|dvd5|complete).*$"
    ).unwrap()
});
static RE_SPACE_COLLAPSE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());
static RE_SEASON_INFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)\s+S\d{1,2}(?:-S?\d{1,2})?(?:\s|$).*$").unwrap());
static RE_FOLDER_RELEASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)[\s\.](1080p|720p|480p|2160p|4k|bluray|blu-ray|webrip|web-dl|web|hdtv|dvdrip|bdrip|x264|x265|h\.?264|h\.?265|hevc|avc|aac|opus|flac|dts|atmos|10bit|10-bit|hdr|sdr|remux|proper|repack|multi|dual|dubbed|subbed|raw|nf|cr|amzn|dsnp|hmax|hulu|complete|batch).*$"
    ).unwrap()
});
static RE_GROUP_SUFFIX: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*-[A-Za-z0-9]+$").unwrap());
/// Matches bracketed info like [BDRip], [1080p], [Dual Audio], etc.
static RE_BRACKETED_INFO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\[[^\]]*\]\s*").unwrap());
/// Matches parenthesized release info like (BD 720p), (1080p), (V2), etc.
/// But NOT years like (2023) which are 4 digits only
static RE_PAREN_RELEASE_INFO: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\s*\((?:BD|DVD|BluRay|BDRip|WEB|HDTV|V\d+|\d{3,4}p)[^\)]*\)\s*").unwrap()
});
static RE_MOVIE_YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(.+?)[\s\.\-]*[\(\[]?(\d{4})[\)\]]?\s*$").unwrap());

pub fn is_video_file(path: &Path) -> bool {
    let ext = match path.extension().and_then(|ext| ext.to_str()) {
        Some(e) => e.to_lowercase(),
        None => return false,
    };

    // Check configured extensions first
    let configured = get_video_extensions();
    if !configured.is_empty() {
        return configured.iter().any(|e| e == &ext);
    }

    // Fall back to defaults
    DEFAULT_VIDEO_EXTENSIONS.contains(&ext.as_str())
}

#[derive(Debug, Clone)]
pub struct ParsedEpisode {
    pub show_name: String,
    pub season: i32,
    pub episode: i32,
}

#[derive(Debug, Clone)]
pub struct ParsedMovie {
    pub title: String,
    pub year: Option<i32>,
}

/// Parse episode info from filename
/// Supports multiple formats:
/// - "Show Name S01E05.mkv" (standard)
/// - "[Group] Show Name - E05 [quality].mkv" (anime style)
/// - "Show Name - 05.mkv" (simple numbered)
/// - "Show.Name.S01E01.mkv" (dot-separated)
pub fn parse_episode_filename(filename: &str) -> Option<ParsedEpisode> {
    let name = filename
        .rsplit_once('.')
        .map(|(name, _)| name)
        .unwrap_or(filename);

    if let Some(caps) = RE_SEASON_EP.captures(name) {
        let season: i32 = caps.get(1)?.as_str().parse().ok()?;
        let episode: i32 = caps.get(2)?.as_str().parse().ok()?;

        let show_name = extract_show_name(name, caps.get(0)?.start());

        return Some(ParsedEpisode {
            show_name,
            season,
            episode,
        });
    }

    if let Some(caps) = RE_ALT_EP.captures(name) {
        if let (Ok(season), Ok(episode)) = (
            caps.get(1)?.as_str().parse::<i32>(),
            caps.get(2)?.as_str().parse::<i32>(),
        ) {
            if (1..=20).contains(&season) && (1..=999).contains(&episode) {
                let show_name = extract_show_name(name, caps.get(0)?.start());
                return Some(ParsedEpisode {
                    show_name,
                    season,
                    episode,
                });
            }
        }
    }

    if let Some(caps) = RE_ANIME_EP.captures(name) {
        let episode: i32 = caps.get(1)?.as_str().parse().ok()?;
        if (1..=999).contains(&episode) {
            let show_name = extract_show_name(name, caps.get(0)?.start());
            return Some(ParsedEpisode {
                show_name,
                season: 1,
                episode,
            });
        }
    }

    None
}

/// Extract and clean show name from filename
fn extract_show_name(filename: &str, end_pos: usize) -> String {
    let name = &filename[..end_pos];

    let name = RE_GROUP_TAG.replace(name, "");
    let name = name.trim();

    let name = name.replace('.', " ");

    let name = RE_RELEASE_INFO.replace(&name, "");

    let name = name.trim();
    let name = name.trim_end_matches(['-', ' ', '_']);

    RE_SPACE_COLLAPSE.replace_all(name, " ").to_string()
}

/// Parse movie name and year from filename
/// e.g., "The Matrix (1999).mkv" -> ("The Matrix", Some(1999))
pub fn parse_movie_filename(filename: &str) -> ParsedMovie {
    let name = filename
        .rsplit_once('.')
        .map(|(name, _)| name)
        .unwrap_or(filename);

    if let Some(caps) = RE_MOVIE_YEAR.captures(name) {
        let title = caps.get(1).map(|m| m.as_str().trim()).unwrap_or(name);
        let year = caps.get(2).and_then(|m| m.as_str().parse().ok());

        let title = title.trim_end_matches(['-', ' ', '.']);

        return ParsedMovie {
            title: title.to_string(),
            year,
        };
    }

    ParsedMovie {
        title: name.to_string(),
        year: None,
    }
}

/// Scan a library directory and add all media items to the database
pub async fn scan_library(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
    library_type: &str,
) -> Result<ScanResult> {
    scan_library_with_cache_dir(
        pool,
        library_id,
        path,
        library_type,
        PathBuf::from("cache"),
        None,
        None,
    )
    .await
}

/// Scan a library directory with explicit cache directory
pub async fn scan_library_with_cache_dir(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
    library_type: &str,
    cache_dir: PathBuf,
    anime_db_enabled: Option<bool>,
    fetch_episode_metadata: Option<bool>,
) -> Result<ScanResult> {
    let image_cache_dir = cache_dir.join("images");
    let metadata_service = MetadataService::from_env(image_cache_dir, anime_db_enabled);
    let fetch_ep_meta = fetch_episode_metadata.unwrap_or(false);

    if metadata_service.has_tmdb() {
        tracing::info!("Metadata providers: AniList + TMDB");
    } else {
        tracing::info!("Metadata providers: AniList only (set TMDB_API_KEY for more coverage)");
    }

    if fetch_ep_meta {
        tracing::info!("Episode metadata fetching: enabled");
    } else {
        tracing::debug!("Episode metadata fetching: disabled (reduces API calls)");
    }

    if metadata_service.has_anime_db() {
        tracing::info!("Anime offline database: enabled, preloading...");
        match metadata_service.preload_anime_db().await {
            Ok(()) => {
                tracing::info!("Anime offline database: ready");
            }
            Err(e) => {
                tracing::warn!("Failed to preload anime offline database: {}", e);
            }
        }
    }

    let scan_result = scan_library_with_metadata(
        pool,
        library_id,
        path,
        library_type,
        Some(&metadata_service),
        fetch_ep_meta,
    )
    .await;

    // Unload anime database after scan to free memory
    metadata_service.unload_anime_db().await;

    // Shrink SQLite memory after large scan operation
    if let Err(e) = crate::db::shrink_memory(pool).await {
        tracing::warn!("Failed to shrink SQLite memory: {}", e);
    }

    scan_result
}

/// Scan a library with optional metadata service
pub async fn scan_library_with_metadata(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
    library_type: &str,
    metadata: Option<&MetadataService>,
    fetch_episode_metadata: bool,
) -> Result<ScanResult> {
    let mut result = ScanResult::default();

    tracing::info!("Scanning library '{}' at path: {}", library_id, path);

    let path = Path::new(path);
    // Use async check to avoid blocking
    if !fs::try_exists(path).await.unwrap_or(false) {
        tracing::warn!("Library path does not exist: {:?}", path);
        return Ok(result);
    }

    // Pre-cache existing series to avoid redundant lookups
    let series_cache = build_series_cache(pool, library_id).await?;

    match library_type {
        "tvshows" | "tvshow" => {
            scan_tv_library_with_cache(
                pool,
                library_id,
                path,
                &mut result,
                metadata,
                &series_cache,
                fetch_episode_metadata,
            )
            .await?;
        }
        "movies" | "movie" => {
            scan_movie_library(pool, library_id, path, &mut result, metadata).await?;
        }
        _ => {
            tracing::warn!("Unknown library type: {}", library_type);
        }
    }

    tracing::info!(
        "Scan complete: {} series added, {} series reused, {} episodes added, {} episodes in existing series, {} movies added",
        result.series_added,
        result.series_reused,
        result.episodes_added,
        result.episodes_from_existing_series,
        result.movies_added
    );

    Ok(result)
}

#[derive(Debug, Default)]
pub struct ScanResult {
    pub series_added: i32,
    pub episodes_added: i32,
    pub movies_added: i32,
    pub series_reused: i32,
    pub episodes_from_existing_series: i32,
}

/// Type alias for series row data from database
type SeriesRow = (
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
);

/// Cache of existing series to avoid redundant database lookups
#[derive(Debug, Clone)]
struct SeriesCache {
    /// Map of folder path -> (series_id, metadata)
    by_path: std::collections::HashMap<String, (String, Option<UnifiedMetadata>)>,
    /// Map of provider ID -> series_id for deduplication
    by_provider: std::collections::HashMap<String, String>,
}

/// Folders to skip during scanning (case-insensitive check)
const SKIP_FOLDER_NAMES: &[&str] = &[
    "nced",
    "ncop",
    "nc",
    "creditless",
    "extras",
    "extra",
    "bonus",
    "specials",
    "behind the scenes",
    "deleted scenes",
    "interviews",
    "scenes",
    "shorts",
    "trailers",
    "featurettes",
    "other",
    "sample",
    "samples",
    ".unwatched",
];

/// Patterns that indicate a folder is for openings/endings/extras (suffix patterns)
const SKIP_FOLDER_SUFFIXES: &[&str] = &[
    " - nced",
    " - ncop",
    " - nc",
    " - ending",
    " - opening",
    " - op",
    " - ed",
    " nced",
    " ncop",
    "-nced",
    "-ncop",
    "_nced",
    "_ncop",
    " creditless",
    " textless",
    " - ova",
    " - special",
    " - specials",
    " - extra",
    " - extras",
    " battle stage",
    " extra stage",
];

/// Check if a folder should be skipped (special folders like NCED, NCOP, etc.)
fn should_skip_folder(folder_name: &str) -> bool {
    let name_lower = folder_name.to_lowercase();

    // Check exact matches
    if SKIP_FOLDER_NAMES.contains(&name_lower.as_str()) {
        return true;
    }

    // Check prefix patterns
    if name_lower.starts_with("ncop") || name_lower.starts_with("nced") {
        return true;
    }

    // Check suffix patterns (e.g., "Bocchi the Rock! - NCED")
    for suffix in SKIP_FOLDER_SUFFIXES {
        if name_lower.ends_with(suffix) {
            return true;
        }
    }

    // Check contains patterns
    if name_lower.contains("creditless") || name_lower.contains("textless") {
        return true;
    }

    // Skip folders that look like release groups with no actual show name
    // e.g., just "[SubGroup]" or "1080p.x265"
    if name_lower.starts_with('[') && !name_lower.contains(" - ") && name_lower.len() < 30 {
        return true;
    }

    false
}

/// Check if a .ignore file exists in this directory or any parent directory
async fn should_ignore_path(path: &Path) -> bool {
    let mut current = path.to_path_buf();

    // Walk up the directory tree looking for .ignore file
    loop {
        // Use async check to avoid blocking
        if fs::try_exists(current.join(".ignore"))
            .await
            .unwrap_or(false)
        {
            return true;
        }

        if !current.pop() {
            break;
        }
    }

    false
}

/// Pre-cache all existing series in the library to avoid redundant lookups
async fn build_series_cache(pool: &SqlitePool, library_id: &str) -> Result<SeriesCache> {
    let mut cache = SeriesCache {
        by_path: std::collections::HashMap::new(),
        by_provider: std::collections::HashMap::new(),
    };

    // Fetch all existing series
    let series_rows: Vec<SeriesRow> = sqlx::query_as(
        r#"SELECT id, name, anilist_id, tmdb_id, mal_id, anidb_id 
           FROM media_items 
           WHERE library_id = ? AND item_type = 'Series'
           ORDER BY created_at DESC"#,
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    tracing::info!("Pre-cached {} existing series", series_rows.len());

    for (id, _name, anilist_id, tmdb_id, mal_id, anidb_id) in series_rows {
        // Index by provider IDs for fast deduplication
        if let Some(anilist) = &anilist_id {
            cache
                .by_provider
                .insert(format!("anilist:{}", anilist), id.clone());
        }
        if let Some(tmdb) = &tmdb_id {
            cache
                .by_provider
                .insert(format!("tmdb:{}", tmdb), id.clone());
        }
        if let Some(mal) = &mal_id {
            cache.by_provider.insert(format!("mal:{}", mal), id.clone());
        }
        if let Some(anidb) = &anidb_id {
            cache
                .by_provider
                .insert(format!("anidb:{}", anidb), id.clone());
        }
    }

    Ok(cache)
}

/// Find a series in cache by provider IDs
fn find_series_in_cache(cache: &SeriesCache, metadata: &UnifiedMetadata) -> Option<String> {
    // Check in priority order
    if let Some(id) = metadata.anilist_id.as_ref().and_then(|anilist| {
        cache
            .by_provider
            .get(&format!("anilist:{}", anilist))
            .cloned()
    }) {
        return Some(id);
    }

    if let Some(id) = metadata
        .tmdb_id
        .as_ref()
        .and_then(|tmdb| cache.by_provider.get(&format!("tmdb:{}", tmdb)).cloned())
    {
        return Some(id);
    }

    if let Some(id) = metadata
        .mal_id
        .as_ref()
        .and_then(|mal| cache.by_provider.get(&format!("mal:{}", mal)).cloned())
    {
        return Some(id);
    }

    if let Some(id) = metadata
        .anidb_id
        .as_ref()
        .and_then(|anidb| cache.by_provider.get(&format!("anidb:{}", anidb)).cloned())
    {
        return Some(id);
    }

    None
}

async fn scan_tv_library(
    pool: &SqlitePool,
    library_id: &str,
    path: &Path,
    result: &mut ScanResult,
    metadata: Option<&MetadataService>,
) -> Result<()> {
    // Create an empty cache for backward compatibility
    let cache = SeriesCache {
        by_path: std::collections::HashMap::new(),
        by_provider: std::collections::HashMap::new(),
    };
    // Default to no episode metadata fetching for backward compatibility
    scan_tv_library_with_cache(pool, library_id, path, result, metadata, &cache, false).await
}

async fn scan_tv_library_with_cache(
    pool: &SqlitePool,
    library_id: &str,
    path: &Path,
    result: &mut ScanResult,
    metadata: Option<&MetadataService>,
    series_cache: &SeriesCache,
    fetch_episode_metadata: bool,
) -> Result<()> {
    // For TV shows, we expect a structure like:
    // library_path/
    //   Show Name 1/
    //     Season 1/ (optional)
    //       S01E01.mkv
    //     [Group] Show Name 1 - 01.mkv
    //   Show Name 2 (2023)/
    //     [Group] Show Name 2 - 01.mkv
    //
    // Each top-level folder = one series
    // We use the folder name for metadata lookup, NOT the parsed filename

    let mut entries = fs::read_dir(path).await?;

    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();

        if entry_path.is_dir() {
            let folder_name = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            // Check for .ignore file (can skip entire subtrees)
            if should_ignore_path(&entry_path).await {
                tracing::debug!("Skipping ignored folder: {}", folder_name);
                continue;
            }

            // Skip special folders
            if should_skip_folder(folder_name) {
                tracing::debug!("Skipping special folder: {}", folder_name);
                continue;
            }

            // This is a show folder - create a series for it
            tracing::info!("Scanning show folder: {}", folder_name);

            // Create the series using the folder name for metadata lookup
            let (series_id, series_metadata, is_new_series) = create_or_get_series_with_cache(
                pool,
                library_id,
                folder_name,
                folder_name, // Use folder name for anime detection too
                metadata,
                series_cache,
            )
            .await?;
            if is_new_series {
                result.series_added += 1;
            } else {
                result.series_reused += 1;
            }

            // Now recursively scan this folder for episodes
            scan_show_folder(
                pool,
                library_id,
                &series_id,
                series_metadata.as_ref(),
                &entry_path,
                result,
                metadata,
                fetch_episode_metadata,
            )
            .await?;
        } else if entry_path.is_file() && is_video_file(&entry_path) {
            // Video files directly in the library root are unusual for TV shows
            // but we'll handle them - try to parse and create as standalone series
            let filename = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            if let Some(parsed) = parse_episode_filename(filename) {
                tracing::warn!(
                    "Found episode file in library root (expected in show folder): {}",
                    filename
                );

                // Create series using parsed show name since we don't have a folder
                let (series_id, series_metadata, is_new_series) =
                    create_or_get_series(pool, library_id, &parsed.show_name, filename, metadata)
                        .await?;
                if is_new_series {
                    result.series_added += 1;
                }

                create_episode(
                    pool,
                    library_id,
                    &series_id,
                    &parsed,
                    entry_path.to_str().unwrap_or_default(),
                    series_metadata.as_ref(),
                    metadata,
                    fetch_episode_metadata,
                )
                .await?;
                result.episodes_added += 1;
            }
        }
    }

    Ok(())
}

/// Scan a show folder for episodes with parallel media info extraction
///
/// This function:
/// 1. Recursively collects all video files (with symlink loop protection)
/// 2. Parses episode info from filenames
/// 3. Extracts media info (ffprobe) in parallel
/// 4. Inserts episodes into the database
async fn scan_show_folder(
    pool: &SqlitePool,
    library_id: &str,
    series_id: &str,
    series_metadata: Option<&UnifiedMetadata>,
    path: &Path,
    result: &mut ScanResult,
    metadata_service: Option<&MetadataService>,
    fetch_episode_metadata: bool,
) -> Result<()> {
    // Phase 1: Collect all video files recursively with symlink protection
    let mut visited = HashSet::new();
    let video_files = collect_video_files(path, &mut visited).await?;

    if video_files.is_empty() {
        return Ok(());
    }

    tracing::debug!("Found {} video files in {:?}", video_files.len(), path);

    // Phase 2: Parse episode info from filenames
    let parseable_files: Vec<(PathBuf, ParsedEpisode)> = video_files
        .into_iter()
        .filter_map(|file_path| {
            let filename = file_path.file_name()?.to_str()?;
            let parsed = parse_episode_filename(filename)?;
            Some((file_path, parsed))
        })
        .collect();

    if parseable_files.is_empty() {
        tracing::debug!("No parseable episodes found in {:?}", path);
        return Ok(());
    }

    // Phase 3: Extract media info in parallel (ffprobe is the bottleneck)
    let episodes_with_info = parallel_extract_media_info(parseable_files).await;

    // Phase 4: Insert episodes into database
    // We process in batches for better memory management, but each episode
    // still needs individual metadata fetch (for episode-specific info) if enabled
    for episode_info in episodes_with_info {
        // Fetch episode metadata if available and enabled (e.g., from TMDB)
        let (episode_name, overview, premiere_date, rating) = if fetch_episode_metadata {
            if let Some(service) = metadata_service {
                match service
                    .get_episode_metadata(
                        series_metadata,
                        episode_info.parsed.season,
                        episode_info.parsed.episode,
                    )
                    .await
                {
                    Ok(Some(ep_meta)) => {
                        let name = ep_meta
                            .name
                            .unwrap_or_else(|| format!("Episode {}", episode_info.parsed.episode));
                        (
                            name,
                            ep_meta.overview,
                            ep_meta.premiere_date,
                            ep_meta.community_rating,
                        )
                    }
                    _ => (
                        format!("Episode {}", episode_info.parsed.episode),
                        None,
                        None,
                        None,
                    ),
                }
            } else {
                (
                    format!("Episode {}", episode_info.parsed.episode),
                    None,
                    None,
                    None,
                )
            }
        } else {
            (
                format!("Episode {}", episode_info.parsed.episode),
                None,
                None,
                None,
            )
        };

        let id = Uuid::new_v4().to_string();
        let file_path = episode_info.path.to_str().unwrap_or_default();

        // Check if this episode already exists (by path) to avoid duplicates
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM media_items WHERE path = ?")
                .bind(file_path)
                .fetch_optional(pool)
                .await?;

        if let Some((existing_id,)) = existing {
            // Episode exists, but make sure it has a thumbnail queued
            if !crate::db::has_thumbnail(pool, &existing_id)
                .await
                .unwrap_or(true)
            {
                let _ = crate::db::queue_thumbnail(pool, &existing_id, file_path).await;
            }
            tracing::debug!("Skipping duplicate episode: {}", file_path);
            continue;
        }

        sqlx::query(
            r#"INSERT INTO media_items 
               (id, library_id, parent_id, item_type, name, path, index_number, parent_index_number, runtime_ticks, overview, premiere_date, community_rating)
               VALUES (?, ?, ?, 'Episode', ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(library_id)
        .bind(series_id)
        .bind(&episode_name)
        .bind(file_path)
        .bind(episode_info.parsed.episode)
        .bind(episode_info.parsed.season)
        .bind(episode_info.runtime_ticks)
        .bind(&overview)
        .bind(&premiere_date)
        .bind(rating)
        .execute(pool)
        .await?;

        // Queue thumbnail generation
        if let Err(e) = crate::db::queue_thumbnail(pool, &id, file_path).await {
            tracing::warn!("Failed to queue thumbnail for episode {}: {}", id, e);
        }

        result.episodes_added += 1;
    }

    Ok(())
}

/// Scan a movie library with parallel media info extraction
///
/// This function:
/// 1. Recursively collects all video files (with symlink loop protection)
/// 2. Parses movie info from filenames
/// 3. Extracts media info (ffprobe) in parallel
/// 4. Fetches metadata and inserts movies into the database
async fn scan_movie_library(
    pool: &SqlitePool,
    library_id: &str,
    path: &Path,
    result: &mut ScanResult,
    metadata_service: Option<&MetadataService>,
) -> Result<()> {
    // Phase 1: Collect all video files recursively with symlink protection
    let mut visited = HashSet::new();
    let video_files = collect_video_files(path, &mut visited).await?;

    if video_files.is_empty() {
        return Ok(());
    }

    tracing::debug!(
        "Found {} video files in movie library {:?}",
        video_files.len(),
        path
    );

    // Phase 2: Parse movie info from filenames
    let parseable_files: Vec<(PathBuf, ParsedMovie)> = video_files
        .into_iter()
        .map(|file_path| {
            let filename = file_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let parsed = parse_movie_filename(filename);
            (file_path, parsed)
        })
        .collect();

    // Phase 3: Extract media info in parallel
    let movies_with_info = parallel_extract_movie_info(parseable_files).await;

    // Phase 4: Fetch metadata and insert movies
    for movie_info in movies_with_info {
        let file_path = movie_info.path.to_str().unwrap_or_default();

        // Check if this movie already exists (by path) to avoid duplicates
        let existing: Option<(String,)> =
            sqlx::query_as("SELECT id FROM media_items WHERE path = ?")
                .bind(file_path)
                .fetch_optional(pool)
                .await?;

        if let Some((existing_id,)) = existing {
            // Movie exists, but make sure it has a thumbnail queued
            if !crate::db::has_thumbnail(pool, &existing_id)
                .await
                .unwrap_or(true)
            {
                let _ = crate::db::queue_thumbnail(pool, &existing_id, file_path).await;
            }
            tracing::debug!("Skipping duplicate movie: {}", file_path);
            continue;
        }

        // Fetch metadata from providers
        let metadata = if let Some(service) = metadata_service {
            match service
                .get_movie_metadata(&movie_info.parsed.title, movie_info.parsed.year)
                .await
            {
                Ok(Some(meta)) => {
                    tracing::debug!(
                        "Found metadata for movie: {} -> {}",
                        movie_info.parsed.title,
                        meta.name.as_deref().unwrap_or("Unknown")
                    );
                    Some(meta)
                }
                Ok(None) => None,
                Err(e) => {
                    tracing::debug!(
                        "Failed to fetch metadata for {}: {}",
                        movie_info.parsed.title,
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        let id = Uuid::new_v4().to_string();
        let sort_name = movie_info.parsed.title.to_lowercase();

        let (
            final_name,
            overview,
            year,
            premiere_date,
            rating,
            tmdb_id,
            imdb_id,
            anilist_id,
            mal_id,
        ) = if let Some(ref meta) = metadata {
            (
                meta.name.as_deref().unwrap_or(&movie_info.parsed.title),
                meta.overview.as_deref(),
                meta.year.or(movie_info.parsed.year),
                meta.premiere_date.as_deref(),
                meta.community_rating,
                meta.tmdb_id.as_deref(),
                meta.imdb_id.as_deref(),
                meta.anilist_id.as_deref(),
                meta.mal_id.as_deref(),
            )
        } else {
            (
                &movie_info.parsed.title as &str,
                None,
                movie_info.parsed.year,
                None,
                None,
                None,
                None,
                None,
                None,
            )
        };

        // Use runtime from ffprobe (parallel extraction) or fallback to metadata
        let runtime_ticks = movie_info.runtime_ticks;

        sqlx::query(
            r#"INSERT INTO media_items 
               (id, library_id, item_type, name, path, year, sort_name, runtime_ticks, overview, premiere_date, community_rating, tmdb_id, imdb_id, anilist_id, mal_id)
               VALUES (?, ?, 'Movie', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
        )
        .bind(&id)
        .bind(library_id)
        .bind(final_name)
        .bind(file_path)
        .bind(year)
        .bind(&sort_name)
        .bind(runtime_ticks)
        .bind(overview)
        .bind(premiere_date)
        .bind(rating)
        .bind(tmdb_id)
        .bind(imdb_id)
        .bind(anilist_id)
        .bind(mal_id)
        .execute(pool)
        .await?;

        // Queue images for background download
        if let Some(ref meta) = metadata {
            if let Some(ref url) = meta.poster_url {
                let _ = crate::db::queue_image(pool, &id, "Primary", url).await;
            }
            if let Some(ref url) = meta.backdrop_url {
                let _ = crate::db::queue_image(pool, &id, "Backdrop", url).await;
            }
        }

        // Queue thumbnail generation
        if let Err(e) = crate::db::queue_thumbnail(pool, &id, file_path).await {
            tracing::warn!("Failed to queue thumbnail for movie {}: {}", id, e);
        }

        // Save genres to normalized tables
        if let Some(ref meta) = metadata {
            if let Some(ref genres) = meta.genres {
                for genre_name in genres {
                    if let Ok(genre_id) = get_or_create_genre(pool, genre_name).await {
                        let _ = link_item_genre(pool, &id, &genre_id).await;
                    }
                }
            }
            if let Some(ref studio_name) = meta.studio {
                if let Ok(studio_id) = get_or_create_studio(pool, studio_name).await {
                    let _ = link_item_studio(pool, &id, &studio_id).await;
                }
            }
        }

        result.movies_added += 1;
    }

    Ok(())
}

/// Clean a folder name by removing release group info and normalizing
/// e.g., "Himouto.Umaru.chan.S01.1080p.BluRay.x265-smol" -> "Himouto Umaru-chan"
fn clean_folder_name(name: &str) -> String {
    // Replace dots with spaces (before other processing)
    let name = name.replace('.', " ");

    // Remove bracketed info FIRST like [BDRip], [1080p], [Dual Audio], etc.
    // This must happen before release pattern matching to avoid matching inside brackets
    let name = RE_BRACKETED_INFO.replace_all(&name, " ");

    // Remove parenthesized release info like (BD 720p), (1080p), (V2) but NOT years
    let name = RE_PAREN_RELEASE_INFO.replace_all(&name, " ");

    // Remove season info (S01, S02, S01-S03, etc.) and everything after
    let name = RE_SEASON_INFO.replace(&name, "");

    // Remove common release group patterns (e.g., "1080p.BluRay.x265")
    let name = RE_FOLDER_RELEASE.replace(&name, "");

    // Remove trailing group tags like "-smol" or "-VARYG" or "-EMBER"
    let name = RE_GROUP_SUFFIX.replace(&name, "");

    // Remove trailing underscores and dashes
    let name = name.trim_end_matches(|c| c == '-' || c == '_' || c == ' ');

    // Collapse multiple spaces
    let name = RE_SPACE_COLLAPSE.replace_all(name, " ");

    name.trim().to_string()
}

/// Extract year from a name like "Show Name (2023)" -> ("Show Name", Some(2023))
fn extract_year_from_name(name: &str) -> (String, Option<i32>) {
    // First, clean the folder name
    let cleaned = clean_folder_name(name);

    // Match pattern: "Name (YYYY)" at the end
    if let Some(paren_start) = cleaned.rfind('(') {
        let potential_year =
            cleaned[paren_start..].trim_matches(|c| c == '(' || c == ')' || c == ' ');
        if potential_year.len() == 4 {
            if let Ok(year) = potential_year.parse::<i32>() {
                if (1900..=2100).contains(&year) {
                    let clean_name = cleaned[..paren_start].trim();
                    return (clean_name.to_string(), Some(year));
                }
            }
        }
    }
    (cleaned, None)
}

/// Check if a series with matching provider IDs already exists in the database
/// Returns the existing series ID if found
async fn find_existing_series_by_provider_ids(
    pool: &SqlitePool,
    library_id: &str,
    anilist_id: Option<&str>,
    mal_id: Option<&str>,
    anidb_id: Option<&str>,
    tmdb_id: Option<&str>,
) -> Result<Option<String>> {
    // Build a query to check for any matching provider ID
    // We prioritize checking in order of reliability: AniList, TMDB, MAL, AniDB

    if let Some(id) = anilist_id {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM media_items WHERE library_id = ? AND item_type = 'Series' AND anilist_id = ?"
        )
        .bind(library_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;

        if let Some((existing_id,)) = result {
            tracing::debug!(
                "Found existing series by AniList ID {}: {}",
                id,
                existing_id
            );
            return Ok(Some(existing_id));
        }
    }

    if let Some(id) = tmdb_id {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM media_items WHERE library_id = ? AND item_type = 'Series' AND tmdb_id = ?"
        )
        .bind(library_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;

        if let Some((existing_id,)) = result {
            tracing::debug!("Found existing series by TMDB ID {}: {}", id, existing_id);
            return Ok(Some(existing_id));
        }
    }

    if let Some(id) = mal_id {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM media_items WHERE library_id = ? AND item_type = 'Series' AND mal_id = ?"
        )
        .bind(library_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;

        if let Some((existing_id,)) = result {
            tracing::debug!("Found existing series by MAL ID {}: {}", id, existing_id);
            return Ok(Some(existing_id));
        }
    }

    if let Some(id) = anidb_id {
        let result: Option<(String,)> = sqlx::query_as(
            "SELECT id FROM media_items WHERE library_id = ? AND item_type = 'Series' AND anidb_id = ?"
        )
        .bind(library_id)
        .bind(id)
        .fetch_optional(pool)
        .await?;

        if let Some((existing_id,)) = result {
            tracing::debug!("Found existing series by AniDB ID {}: {}", id, existing_id);
            return Ok(Some(existing_id));
        }
    }

    Ok(None)
}

/// Track a series that failed to get metadata
async fn mark_series_unmatched(
    pool: &SqlitePool,
    library_id: &str,
    series_id: &str,
    folder_name: &str,
    attempted_title: &str,
    attempted_year: Option<i32>,
    failure_reason: &str,
) -> Result<()> {
    let id = Uuid::new_v4().to_string();

    sqlx::query(
        r#"INSERT INTO unmatched_series 
           (id, library_id, series_id, folder_name, attempted_title, attempted_year, failure_reason, attempt_count)
           VALUES (?, ?, ?, ?, ?, ?, ?, 1)
           ON CONFLICT(library_id, series_id) DO UPDATE SET
              attempt_count = attempt_count + 1,
              last_attempt_at = CURRENT_TIMESTAMP,
              failure_reason = excluded.failure_reason"#
    )
    .bind(id)
    .bind(library_id)
    .bind(series_id)
    .bind(folder_name)
    .bind(attempted_title)
    .bind(attempted_year)
    .bind(failure_reason)
    .execute(pool)
    .await?;

    tracing::info!(
        "Marked series {} as unmatched: {}",
        series_id,
        failure_reason
    );

    Ok(())
}

/// Get series that should be retried (attempt_count < 3, oldest first)
async fn get_unmatched_series_for_retry(
    pool: &SqlitePool,
    library_id: &str,
) -> Result<Vec<(String, String, String, Option<i32>)>> {
    // Returns: (series_id, folder_name, attempted_title, attempted_year)
    let rows = sqlx::query_as(
        r#"SELECT series_id, folder_name, attempted_title, attempted_year 
           FROM unmatched_series 
           WHERE library_id = ? AND attempt_count < 3
           ORDER BY last_attempt_at ASC
           LIMIT 50"#,
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    if !rows.is_empty() {
        tracing::info!(
            "Found {} unmatched series to retry for library {}",
            rows.len(),
            library_id
        );
    }

    Ok(rows)
}

/// Clear unmatched tracking for a series (call when metadata is successfully found)
async fn clear_unmatched_tracking(pool: &SqlitePool, series_id: &str) -> Result<()> {
    sqlx::query("DELETE FROM unmatched_series WHERE series_id = ?")
        .bind(series_id)
        .execute(pool)
        .await?;

    Ok(())
}

/// Normalize a series name for comparison purposes
/// This helps detect duplicates like "Blue Box" vs "Blue Box (2024)"
fn normalize_series_name(name: &str) -> String {
    let (clean_name, _year) = extract_year_from_name(name);

    // Additional normalization:
    // - lowercase
    // - remove punctuation except hyphens
    // - collapse whitespace
    let normalized = clean_name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect::<String>();

    RE_SPACE_COLLAPSE
        .replace_all(&normalized, " ")
        .trim()
        .to_string()
}

/// Find an existing series by normalized name (for duplicate detection when no provider IDs match)
async fn find_existing_series_by_name(
    pool: &SqlitePool,
    library_id: &str,
    name: &str,
) -> Result<Option<(String, Option<UnifiedMetadata>)>> {
    let normalized = normalize_series_name(name);

    // Get all series in this library with their names and provider IDs
    let series: Vec<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        r#"SELECT id, name, anilist_id, tmdb_id, mal_id, anidb_id 
           FROM media_items 
           WHERE library_id = ? AND item_type = 'Series'
           ORDER BY created_at ASC"#, // Prefer older entries
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    for (id, existing_name, anilist_id, tmdb_id, mal_id, anidb_id) in series {
        let existing_normalized = normalize_series_name(&existing_name);

        // Check if normalized names match
        if normalized == existing_normalized {
            tracing::info!(
                "Found existing series by normalized name match: '{}' -> '{}' ({})",
                name,
                existing_name,
                id
            );

            // Reconstruct minimal metadata if we have provider IDs
            let metadata = if anilist_id.is_some()
                || tmdb_id.is_some()
                || mal_id.is_some()
                || anidb_id.is_some()
            {
                Some(UnifiedMetadata {
                    anilist_id,
                    tmdb_id,
                    mal_id,
                    anidb_id,
                    ..Default::default()
                })
            } else {
                None
            };

            return Ok(Some((id, metadata)));
        }
    }

    Ok(None)
}

/// Update an existing series with new metadata
async fn update_series_metadata(
    pool: &SqlitePool,
    series_id: &str,
    metadata: &UnifiedMetadata,
) -> Result<()> {
    // Build dynamic UPDATE query based on what's available
    sqlx::query(
        r#"UPDATE media_items SET
            name = COALESCE(?, name),
            overview = COALESCE(?, overview),
            year = COALESCE(?, year),
            premiere_date = COALESCE(?, premiere_date),
            community_rating = COALESCE(?, community_rating),
            anilist_id = COALESCE(?, anilist_id),
            mal_id = COALESCE(?, mal_id),
            anidb_id = COALESCE(?, anidb_id),
            kitsu_id = COALESCE(?, kitsu_id),
            tmdb_id = COALESCE(?, tmdb_id),
            imdb_id = COALESCE(?, imdb_id),
            updated_at = CURRENT_TIMESTAMP
        WHERE id = ?"#,
    )
    .bind(metadata.name.as_deref())
    .bind(metadata.overview.as_deref())
    .bind(metadata.year)
    .bind(metadata.premiere_date.as_deref())
    .bind(metadata.community_rating)
    .bind(metadata.anilist_id.as_deref())
    .bind(metadata.mal_id.as_deref())
    .bind(metadata.anidb_id.as_deref())
    .bind(metadata.kitsu_id.as_deref())
    .bind(metadata.tmdb_id.as_deref())
    .bind(metadata.imdb_id.as_deref())
    .bind(series_id)
    .execute(pool)
    .await?;

    // Queue images if available
    if let Some(ref url) = metadata.poster_url {
        let _ = crate::db::queue_image(pool, series_id, "Primary", url).await;
    }
    if let Some(ref url) = metadata.backdrop_url {
        let _ = crate::db::queue_image(pool, series_id, "Backdrop", url).await;
    }

    // Update genres
    if let Some(ref genres) = metadata.genres {
        for genre_name in genres {
            if let Ok(genre_id) = get_or_create_genre(pool, genre_name).await {
                let _ = link_item_genre(pool, series_id, &genre_id).await;
            }
        }
    }

    // Update studio
    if let Some(ref studio_name) = metadata.studio {
        if let Ok(studio_id) = get_or_create_studio(pool, studio_name).await {
            let _ = link_item_studio(pool, series_id, &studio_id).await;
        }
    }

    tracing::debug!("Updated series {} with new metadata", series_id);
    Ok(())
}

async fn create_or_get_series(
    pool: &SqlitePool,
    library_id: &str,
    name: &str,
    filename: &str,
    metadata_service: Option<&MetadataService>,
) -> Result<(String, Option<UnifiedMetadata>, bool)> {
    // Create an empty cache for backward compatibility
    let cache = SeriesCache {
        by_path: std::collections::HashMap::new(),
        by_provider: std::collections::HashMap::new(),
    };
    create_or_get_series_with_cache(pool, library_id, name, filename, metadata_service, &cache)
        .await
}

async fn create_or_get_series_with_cache(
    pool: &SqlitePool,
    library_id: &str,
    name: &str,
    filename: &str,
    metadata_service: Option<&MetadataService>,
    series_cache: &SeriesCache,
) -> Result<(String, Option<UnifiedMetadata>, bool)> {
    // Returns (series_id, metadata, is_new_series)
    // is_new_series is true if a new series was created, false if an existing one was reused
    let sort_name = name.to_lowercase();

    // Extract year from folder name (e.g., "My Happy Marriage (2023)" -> 2023)
    let (clean_name, folder_year) = extract_year_from_name(name);

    // Detect if this looks like anime (use filename for better detection)
    let is_anime = MetadataService::is_likely_anime(filename);

    // Try to fetch metadata using the unified service
    let metadata = if let Some(service) = metadata_service {
        let result = if is_anime {
            // For anime: prioritize AniList
            tracing::debug!(
                "Detected anime, using anime metadata providers for: {} (year: {:?})",
                clean_name,
                folder_year
            );
            service.get_anime_metadata(&clean_name, folder_year).await
        } else {
            // For regular series: prioritize TMDB
            service.get_series_metadata(&clean_name, folder_year).await
        };

        match result {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found metadata via {} for series: {} -> {}",
                    meta.provider,
                    name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );
                Some(meta)
            }
            Ok(None) => {
                tracing::debug!("No metadata match found for series: {}", name);
                None
            }
            Err(e) => {
                tracing::warn!("Failed to fetch metadata for {}: {}", name, e);
                None
            }
        }
    } else {
        None
    };

    // Check if a series with the same provider IDs already exists
    // This prevents duplicate series entries for the same show
    if let Some(ref meta) = metadata {
        if let Some(cached_id) = find_series_in_cache(series_cache, meta) {
            tracing::info!(
                "Reusing cached series {} for folder '{}' (matched by provider ID)",
                cached_id,
                name
            );
            return Ok((cached_id, metadata, false));
        }

        if let Some(existing_id) = find_existing_series_by_provider_ids(
            pool,
            library_id,
            meta.anilist_id.as_deref(),
            meta.mal_id.as_deref(),
            meta.anidb_id.as_deref(),
            meta.tmdb_id.as_deref(),
        )
        .await?
        {
            tracing::info!(
                "Reusing existing series {} for folder '{}' (matched by provider ID)",
                existing_id,
                name
            );
            // Update the existing series with any new metadata we have
            if let Err(e) = update_series_metadata(pool, &existing_id, meta).await {
                tracing::warn!("Failed to update series metadata: {}", e);
            }
            return Ok((existing_id, metadata, false));
        }
    }

    // If no provider ID match, check by normalized name to avoid duplicates
    // This catches cases like "Blue Box" vs "Blue Box (2024)"
    if let Ok(Some((existing_id, existing_meta))) =
        find_existing_series_by_name(pool, library_id, name).await
    {
        tracing::info!(
            "Reusing existing series {} for folder '{}' (matched by normalized name)",
            existing_id,
            name
        );

        // If we have NEW metadata that's better than existing, update the series
        if let Some(ref new_meta) = metadata {
            let should_update = match &existing_meta {
                None => true, // No existing metadata, definitely update
                Some(existing) => {
                    // Update if new metadata has more info (e.g., has overview but existing doesn't)
                    (new_meta.overview.is_some() && existing.overview.is_none())
                        || (new_meta.poster_url.is_some() && existing.poster_url.is_none())
                        || (new_meta.anilist_id.is_some() && existing.anilist_id.is_none())
                        || (new_meta.mal_id.is_some() && existing.mal_id.is_none())
                }
            };

            if should_update {
                tracing::info!(
                    "Updating series {} with new metadata from {:?}",
                    existing_id,
                    new_meta.provider
                );
                if let Err(e) = update_series_metadata(pool, &existing_id, new_meta).await {
                    tracing::warn!("Failed to update series metadata: {}", e);
                }
                return Ok((existing_id, metadata, false));
            }
        }

        // Use existing metadata if we didn't update
        let final_metadata = existing_meta.or(metadata);
        return Ok((existing_id, final_metadata, false));
    }

    let id = Uuid::new_v4().to_string();

    // Extract values from metadata
    let (
        final_name,
        overview,
        year,
        premiere_date,
        rating,
        anilist_id,
        mal_id,
        anidb_id,
        kitsu_id,
        tmdb_id,
        imdb_id,
    ) = if let Some(ref meta) = metadata {
        (
            meta.name.as_deref().unwrap_or(name),
            meta.overview.as_deref(),
            meta.year,
            meta.premiere_date.as_deref(),
            meta.community_rating,
            meta.anilist_id.as_deref(),
            meta.mal_id.as_deref(),
            meta.anidb_id.as_deref(),
            meta.kitsu_id.as_deref(),
            meta.tmdb_id.as_deref(),
            meta.imdb_id.as_deref(),
        )
    } else {
        (
            name, None, None, None, None, None, None, None, None, None, None,
        )
    };

    sqlx::query(
        r#"INSERT INTO media_items 
           (id, library_id, item_type, name, sort_name, overview, year, premiere_date, community_rating, tmdb_id, imdb_id, anilist_id, mal_id, anidb_id, kitsu_id)
           VALUES (?, ?, 'Series', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(library_id)
    .bind(final_name)
    .bind(&sort_name)
    .bind(overview)
    .bind(year)
    .bind(premiere_date)
    .bind(rating)
    .bind(tmdb_id)
    .bind(imdb_id)
    .bind(anilist_id)
    .bind(mal_id)
    .bind(anidb_id)
    .bind(kitsu_id)
    .execute(pool)
    .await?;

    // Queue images for background download instead of blocking
    if let Some(ref meta) = metadata {
        if let Some(ref url) = meta.poster_url {
            if let Err(e) = crate::db::queue_image(pool, &id, "Primary", url).await {
                tracing::warn!("Failed to queue poster image for {}: {}", name, e);
            }
        }
        if let Some(ref url) = meta.backdrop_url {
            if let Err(e) = crate::db::queue_image(pool, &id, "Backdrop", url).await {
                tracing::warn!("Failed to queue backdrop image for {}: {}", name, e);
            }
        }
    }

    // Save genres to normalized tables
    if let Some(ref meta) = metadata {
        if let Some(ref genres) = meta.genres {
            for genre_name in genres {
                match get_or_create_genre(pool, genre_name).await {
                    Ok(genre_id) => {
                        if let Err(e) = link_item_genre(pool, &id, &genre_id).await {
                            tracing::warn!(
                                "Failed to link genre '{}' to series: {}",
                                genre_name,
                                e
                            );
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create genre '{}': {}", genre_name, e);
                    }
                }
            }
        }
        // Save studio to normalized table
        if let Some(ref studio_name) = meta.studio {
            match get_or_create_studio(pool, studio_name).await {
                Ok(studio_id) => {
                    if let Err(e) = link_item_studio(pool, &id, &studio_id).await {
                        tracing::warn!("Failed to link studio '{}' to series: {}", studio_name, e);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to create studio '{}': {}", studio_name, e);
                }
            }
        }

        // Save cast (voice actors) to normalized tables
        for (i, cast_member) in meta.cast.iter().enumerate() {
            match get_or_create_person(pool, cast_member).await {
                Ok(person_id) => {
                    if let Err(e) = link_item_person(
                        pool,
                        &id,
                        &person_id,
                        cast_member.character_name.as_deref(),
                        i as i32,
                    )
                    .await
                    {
                        tracing::warn!(
                            "Failed to link person '{}' to series: {}",
                            cast_member.person_name,
                            e
                        );
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to create person '{}': {}",
                        cast_member.person_name,
                        e
                    );
                }
            }
        }
    }

    tracing::debug!(
        "Created series: {} (Provider: {})",
        final_name,
        metadata
            .as_ref()
            .map(|m| m.provider.to_string())
            .unwrap_or_else(|| "None".to_string())
    );

    // Track if this series has no metadata
    if metadata.is_none() {
        if let Err(e) = mark_series_unmatched(
            pool,
            library_id,
            &id,
            name,
            name,
            extract_year_from_name(name).1,
            "No metadata match found",
        )
        .await
        {
            tracing::warn!("Failed to track unmatched series: {}", e);
        }
    }

    Ok((id, metadata, true)) // true = new series was created
}

async fn create_episode(
    pool: &SqlitePool,
    library_id: &str,
    series_id: &str,
    parsed: &ParsedEpisode,
    file_path: &str,
    series_metadata: Option<&UnifiedMetadata>,
    metadata_service: Option<&MetadataService>,
    fetch_episode_metadata: bool,
) -> Result<String> {
    // Check if this episode already exists (by path) to avoid duplicates
    let existing: Option<(String,)> = sqlx::query_as("SELECT id FROM media_items WHERE path = ?")
        .bind(file_path)
        .fetch_optional(pool)
        .await?;

    if let Some((existing_id,)) = existing {
        // Episode exists, but make sure it has a thumbnail queued
        if !crate::db::has_thumbnail(pool, &existing_id)
            .await
            .unwrap_or(true)
        {
            let _ = crate::db::queue_thumbnail(pool, &existing_id, file_path).await;
        }
        tracing::debug!("Episode already exists, skipping: {}", file_path);
        return Ok(existing_id);
    }

    let id = Uuid::new_v4().to_string();

    // Try to fetch episode metadata from TMDB if enabled and we have a TMDB ID for the series
    let (episode_name, overview, premiere_date, rating) = if fetch_episode_metadata {
        if let Some(service) = metadata_service {
            match service
                .get_episode_metadata(series_metadata, parsed.season, parsed.episode)
                .await
            {
                Ok(Some(ep_meta)) => {
                    let name = ep_meta
                        .name
                        .unwrap_or_else(|| format!("Episode {}", parsed.episode));
                    tracing::debug!(
                        "Found episode metadata: S{:02}E{:02} - {}",
                        parsed.season,
                        parsed.episode,
                        name
                    );
                    (
                        name,
                        ep_meta.overview,
                        ep_meta.premiere_date,
                        ep_meta.community_rating,
                    )
                }
                Ok(None) => {
                    tracing::debug!(
                        "No episode metadata found for S{:02}E{:02}",
                        parsed.season,
                        parsed.episode
                    );
                    (format!("Episode {}", parsed.episode), None, None, None)
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to fetch episode metadata for S{:02}E{:02}: {}",
                        parsed.season,
                        parsed.episode,
                        e
                    );
                    (format!("Episode {}", parsed.episode), None, None, None)
                }
            }
        } else {
            (format!("Episode {}", parsed.episode), None, None, None)
        }
    } else {
        (format!("Episode {}", parsed.episode), None, None, None)
    };

    // Extract media info (duration, etc.)
    let runtime_ticks = match mediainfo::extract_media_info_async(Path::new(file_path)).await {
        Ok(info) => {
            tracing::debug!(
                "Media info for {}: duration={:?}",
                file_path,
                info.duration_ticks
            );
            info.duration_ticks
        }
        Err(e) => {
            tracing::warn!("Failed to extract media info for {}: {}", file_path, e);
            None
        }
    };

    sqlx::query(
        r#"INSERT INTO media_items 
           (id, library_id, parent_id, item_type, name, path, index_number, parent_index_number, runtime_ticks, overview, premiere_date, community_rating)
           VALUES (?, ?, ?, 'Episode', ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(library_id)
    .bind(series_id)
    .bind(&episode_name)
    .bind(file_path)
    .bind(parsed.episode)
    .bind(parsed.season)
    .bind(runtime_ticks)
    .bind(&overview)
    .bind(&premiere_date)
    .bind(rating)
    .execute(pool)
    .await?;

    tracing::debug!(
        "Created episode: S{:02}E{:02} - {}",
        parsed.season,
        parsed.episode,
        episode_name
    );

    // Queue thumbnail generation for this episode
    if let Err(e) = crate::db::queue_thumbnail(pool, &id, file_path).await {
        tracing::warn!("Failed to queue thumbnail for episode {}: {}", id, e);
    }

    Ok(id)
}

async fn create_movie(
    pool: &SqlitePool,
    library_id: &str,
    parsed: &ParsedMovie,
    file_path: &str,
    metadata_service: Option<&MetadataService>,
) -> Result<String> {
    // Check if this movie already exists (by path) to avoid duplicates
    let existing: Option<(String,)> = sqlx::query_as("SELECT id FROM media_items WHERE path = ?")
        .bind(file_path)
        .fetch_optional(pool)
        .await?;

    if let Some((existing_id,)) = existing {
        // Movie exists, but make sure it has a thumbnail queued
        if !crate::db::has_thumbnail(pool, &existing_id)
            .await
            .unwrap_or(true)
        {
            let _ = crate::db::queue_thumbnail(pool, &existing_id, file_path).await;
        }
        tracing::debug!("Movie already exists, skipping: {}", file_path);
        return Ok(existing_id);
    }

    let id = Uuid::new_v4().to_string();
    let sort_name = parsed.title.to_lowercase();

    // Try to fetch metadata from unified service
    let metadata = if let Some(service) = metadata_service {
        match service.get_movie_metadata(&parsed.title, parsed.year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found metadata via {} for movie: {} -> {}",
                    meta.provider,
                    parsed.title,
                    meta.name.as_deref().unwrap_or("Unknown")
                );
                Some(meta)
            }
            Ok(None) => {
                tracing::debug!("No metadata match found for movie: {}", parsed.title);
                None
            }
            Err(e) => {
                tracing::warn!("Failed to fetch metadata for {}: {}", parsed.title, e);
                None
            }
        }
    } else {
        None
    };

    let (final_name, overview, year, premiere_date, rating, tmdb_id, imdb_id, anilist_id, mal_id) =
        if let Some(ref meta) = metadata {
            (
                meta.name.as_deref().unwrap_or(&parsed.title),
                meta.overview.as_deref(),
                meta.year.or(parsed.year),
                meta.premiere_date.as_deref(),
                meta.community_rating,
                meta.tmdb_id.as_deref(),
                meta.imdb_id.as_deref(),
                meta.anilist_id.as_deref(),
                meta.mal_id.as_deref(),
            )
        } else {
            (
                &parsed.title as &str,
                None,
                parsed.year,
                None,
                None,
                None,
                None,
                None,
                None,
            )
        };

    // Extract media info (duration, etc.)
    let runtime_ticks = match mediainfo::extract_media_info_async(Path::new(file_path)).await {
        Ok(info) => {
            tracing::debug!(
                "Media info for {}: duration={:?}",
                file_path,
                info.duration_ticks
            );
            info.duration_ticks
        }
        Err(e) => {
            tracing::warn!("Failed to extract media info for {}: {}", file_path, e);
            None
        }
    };

    sqlx::query(
        r#"INSERT INTO media_items 
           (id, library_id, item_type, name, path, year, sort_name, runtime_ticks, overview, premiere_date, community_rating, tmdb_id, imdb_id, anilist_id, mal_id)
           VALUES (?, ?, 'Movie', ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#,
    )
    .bind(&id)
    .bind(library_id)
    .bind(final_name)
    .bind(file_path)
    .bind(year)
    .bind(&sort_name)
    .bind(runtime_ticks)
    .bind(overview)
    .bind(premiere_date)
    .bind(rating)
    .bind(tmdb_id)
    .bind(imdb_id)
    .bind(anilist_id)
    .bind(mal_id)
    .execute(pool)
    .await?;

    // Queue images for background download instead of blocking
    if let Some(ref meta) = metadata {
        if let Some(ref url) = meta.poster_url {
            if let Err(e) = crate::db::queue_image(pool, &id, "Primary", url).await {
                tracing::warn!("Failed to queue poster image for {}: {}", parsed.title, e);
            }
        }
        if let Some(ref url) = meta.backdrop_url {
            if let Err(e) = crate::db::queue_image(pool, &id, "Backdrop", url).await {
                tracing::warn!("Failed to queue backdrop image for {}: {}", parsed.title, e);
            }
        }
    }

    // Save genres to normalized tables
    if let Some(ref meta) = metadata {
        if let Some(ref genres) = meta.genres {
            for genre_name in genres {
                match get_or_create_genre(pool, genre_name).await {
                    Ok(genre_id) => {
                        if let Err(e) = link_item_genre(pool, &id, &genre_id).await {
                            tracing::warn!("Failed to link genre '{}' to movie: {}", genre_name, e);
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Failed to create genre '{}': {}", genre_name, e);
                    }
                }
            }
        }
        // Save studio to normalized table
        if let Some(ref studio_name) = meta.studio {
            match get_or_create_studio(pool, studio_name).await {
                Ok(studio_id) => {
                    if let Err(e) = link_item_studio(pool, &id, &studio_id).await {
                        tracing::warn!("Failed to link studio '{}' to movie: {}", studio_name, e);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to create studio '{}': {}", studio_name, e);
                }
            }
        }
    }

    tracing::debug!("Created movie: {} ({:?})", final_name, year);

    // Queue thumbnail generation for this movie
    if let Err(e) = crate::db::queue_thumbnail(pool, &id, file_path).await {
        tracing::warn!("Failed to queue thumbnail for movie {}: {}", id, e);
    }

    Ok(id)
}

/// Refresh all libraries
pub async fn refresh_all_libraries(pool: &SqlitePool) -> Result<()> {
    refresh_all_libraries_with_settings(pool, PathBuf::from("cache"), None, None).await
}

/// Refresh all libraries with explicit settings
pub async fn refresh_all_libraries_with_settings(
    pool: &SqlitePool,
    cache_dir: PathBuf,
    anime_db_enabled: Option<bool>,
    fetch_episode_metadata: Option<bool>,
) -> Result<()> {
    let libraries: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, path, library_type FROM libraries")
            .fetch_all(pool)
            .await?;

    for (library_id, path, library_type) in libraries {
        // Clear existing items for this library
        sqlx::query("DELETE FROM media_items WHERE library_id = ?")
            .bind(&library_id)
            .execute(pool)
            .await?;

        scan_library_with_cache_dir(
            pool,
            &library_id,
            &path,
            &library_type,
            cache_dir.clone(),
            anime_db_enabled,
            fetch_episode_metadata,
        )
        .await?;
    }

    Ok(())
}

/// Quick scan result
#[derive(Debug, Default)]
pub struct QuickScanResult {
    pub files_added: i32,
    pub files_removed: i32,
    pub libraries_scanned: i32,
}

/// Quick scan all libraries - only adds new files and removes missing ones
/// This is much faster than a full refresh as it doesn't re-fetch metadata for existing items
pub async fn quick_scan_all_libraries(
    pool: &SqlitePool,
    cache_dir: PathBuf,
) -> Result<QuickScanResult> {
    let libraries: Vec<(String, String, String)> =
        sqlx::query_as("SELECT id, path, library_type FROM libraries")
            .fetch_all(pool)
            .await?;

    let mut total_result = QuickScanResult::default();

    for (library_id, path, library_type) in libraries {
        let result =
            quick_scan_library(pool, &library_id, &path, &library_type, cache_dir.clone()).await?;
        total_result.files_added += result.files_added;
        total_result.files_removed += result.files_removed;
        total_result.libraries_scanned += 1;
    }

    Ok(total_result)
}

/// Quick scan a single library - only adds new files, removes missing ones
pub async fn quick_scan_library(
    pool: &SqlitePool,
    library_id: &str,
    path: &str,
    library_type: &str,
    cache_dir: PathBuf,
) -> Result<QuickScanResult> {
    let mut result = QuickScanResult::default();

    tracing::info!("Quick scanning library '{}' at path: {}", library_id, path);

    // Get all existing file paths in this library
    let existing_paths: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, path FROM media_items WHERE library_id = ? AND path IS NOT NULL",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    let existing_path_set: std::collections::HashSet<String> =
        existing_paths.iter().map(|(_, p)| p.clone()).collect();

    // Check for removed files (use async to avoid blocking)
    for (item_id, item_path) in &existing_paths {
        if !fs::try_exists(Path::new(item_path)).await.unwrap_or(true) {
            tracing::info!("Removing missing file from database: {}", item_path);
            sqlx::query("DELETE FROM media_items WHERE id = ?")
                .bind(item_id)
                .execute(pool)
                .await?;
            result.files_removed += 1;
        }
    }

    // Create metadata service for new files
    let image_cache_dir = cache_dir.join("images");
    let metadata_service = MetadataService::from_env(image_cache_dir, None);

    // Scan for new files (use async check to avoid blocking)
    let path = Path::new(path);
    if !fs::try_exists(path).await.unwrap_or(false) {
        tracing::warn!("Library path does not exist: {:?}", path);
        return Ok(result);
    }

    match library_type {
        "tvshows" | "tvshow" => {
            // Quick scan skips episode metadata fetching for speed
            quick_scan_tv_library(
                pool,
                library_id,
                path,
                &existing_path_set,
                &mut result,
                Some(&metadata_service),
                false, // Skip episode metadata for quick scans
            )
            .await?;
        }
        "movies" | "movie" => {
            quick_scan_movie_library(
                pool,
                library_id,
                path,
                &existing_path_set,
                &mut result,
                Some(&metadata_service),
            )
            .await?;
        }
        _ => {
            tracing::warn!("Unknown library type for quick scan: {}", library_type);
        }
    }

    if result.files_added > 0 || result.files_removed > 0 {
        tracing::info!(
            "Quick scan complete for '{}': {} added, {} removed",
            library_id,
            result.files_added,
            result.files_removed
        );
    } else {
        tracing::debug!("Quick scan complete for '{}': no changes", library_id);
    }

    Ok(result)
}

/// Quick scan TV library - only process files not already in database
async fn quick_scan_tv_library(
    pool: &SqlitePool,
    library_id: &str,
    path: &Path,
    existing_paths: &std::collections::HashSet<String>,
    result: &mut QuickScanResult,
    metadata: Option<&MetadataService>,
    fetch_episode_metadata: bool,
) -> Result<()> {
    let mut entries = fs::read_dir(path).await?;

    // Track series we've created this scan: name -> (id, metadata)
    let mut series_map: std::collections::HashMap<String, (String, Option<UnifiedMetadata>)> =
        std::collections::HashMap::new();

    // Pre-load existing series from DB
    let existing_series: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, name FROM media_items WHERE library_id = ? AND item_type = 'Series'",
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    for (id, name) in existing_series {
        series_map.insert(name, (id, None));
    }

    let mut items_processed = 0u32;

    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();

        if entry_path.is_file() && is_video_file(&entry_path) {
            let path_str = entry_path.to_str().unwrap_or_default().to_string();

            // Skip if already in database
            if existing_paths.contains(&path_str) {
                continue;
            }

            let filename = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            if let Some(parsed) = parse_episode_filename(filename) {
                // Get or create series
                let (series_id, series_metadata) =
                    if let Some((id, meta)) = series_map.get(&parsed.show_name) {
                        (id.clone(), meta.clone())
                    } else {
                        let (id, meta, _is_new) = create_or_get_series(
                            pool,
                            library_id,
                            &parsed.show_name,
                            filename,
                            metadata,
                        )
                        .await?;
                        series_map.insert(parsed.show_name.clone(), (id.clone(), meta.clone()));
                        (id, meta)
                    };

                // Create episode
                create_episode(
                    pool,
                    library_id,
                    &series_id,
                    &parsed,
                    &path_str,
                    series_metadata.as_ref(),
                    metadata,
                    fetch_episode_metadata,
                )
                .await?;
                result.files_added += 1;

                tracing::debug!("Added new episode: {}", filename);
            }

            items_processed += 1;
            if items_processed.is_multiple_of(10) {
                tokio::task::yield_now().await;
            }
        } else if entry_path.is_dir() {
            Box::pin(quick_scan_tv_library(
                pool,
                library_id,
                &entry_path,
                existing_paths,
                result,
                metadata,
                fetch_episode_metadata,
            ))
            .await?;
        }
    }

    Ok(())
}

/// Quick scan movie library - only process files not already in database
async fn quick_scan_movie_library(
    pool: &SqlitePool,
    library_id: &str,
    path: &Path,
    existing_paths: &std::collections::HashSet<String>,
    result: &mut QuickScanResult,
    metadata: Option<&MetadataService>,
) -> Result<()> {
    let mut entries = fs::read_dir(path).await?;
    let mut items_processed = 0u32;

    while let Some(entry) = entries.next_entry().await? {
        let entry_path = entry.path();

        if entry_path.is_file() && is_video_file(&entry_path) {
            let path_str = entry_path.to_str().unwrap_or_default().to_string();

            // Skip if already in database
            if existing_paths.contains(&path_str) {
                continue;
            }

            let filename = entry_path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();

            let parsed = parse_movie_filename(filename);
            create_movie(pool, library_id, &parsed, &path_str, metadata).await?;
            result.files_added += 1;

            tracing::debug!("Added new movie: {}", filename);

            items_processed += 1;
            if items_processed.is_multiple_of(10) {
                tokio::task::yield_now().await;
            }
        } else if entry_path.is_dir() {
            Box::pin(quick_scan_movie_library(
                pool,
                library_id,
                &entry_path,
                existing_paths,
                result,
                metadata,
            ))
            .await?;
        }
    }

    Ok(())
}

/// Result of scanning for missing metadata
#[derive(Debug, Default)]
pub struct MissingMetadataResult {
    pub series_scanned: i32,
    pub series_updated: i32,
    pub movies_scanned: i32,
    pub movies_updated: i32,
}

/// Scan library for items missing metadata and fetch only for those
/// This is more efficient than a full library scan when most items already have metadata
pub async fn scan_missing_metadata(
    pool: &SqlitePool,
    library_id: &str,
    cache_dir: PathBuf,
    anime_db_enabled: Option<bool>,
) -> Result<MissingMetadataResult> {
    let image_cache_dir = cache_dir.join("images");
    let metadata_service = MetadataService::from_env(image_cache_dir, anime_db_enabled);

    // Preload anime database if enabled
    if metadata_service.has_anime_db() {
        let _ = metadata_service.preload_anime_db().await;
    }

    let mut result = MissingMetadataResult::default();

    // Find series missing metadata (no overview AND no poster image)
    let missing_series: Vec<(String, String, Option<i32>)> = sqlx::query_as(
        r#"
        SELECT m.id, m.name, m.year
        FROM media_items m
        WHERE m.library_id = ?
          AND m.item_type = 'Series'
          AND (
            m.overview IS NULL 
            OR m.overview = ''
            OR NOT EXISTS (
                SELECT 1 FROM images i WHERE i.item_id = m.id AND i.image_type = 'Primary'
            )
          )
        ORDER BY m.name
        "#,
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    tracing::info!(
        "Found {} series missing metadata in library {}",
        missing_series.len(),
        library_id
    );

    // Process series
    for (series_id, name, year) in missing_series {
        result.series_scanned += 1;

        // Detect if this looks like anime
        let is_anime = MetadataService::is_likely_anime(&name);

        let metadata_result = if is_anime {
            metadata_service.get_anime_metadata(&name, year).await
        } else {
            metadata_service.get_series_metadata(&name, year).await
        };

        match metadata_result {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found metadata for series '{}' via {:?}",
                    name,
                    meta.provider
                );

                if let Err(e) = update_series_metadata(pool, &series_id, &meta).await {
                    tracing::warn!("Failed to update series '{}': {}", name, e);
                } else {
                    result.series_updated += 1;
                }
            }
            Ok(None) => {
                tracing::debug!("No metadata found for series '{}'", name);
            }
            Err(e) => {
                tracing::warn!("Error fetching metadata for series '{}': {}", name, e);
            }
        }
    }

    // Find movies missing metadata
    let missing_movies: Vec<(String, String, Option<i32>)> = sqlx::query_as(
        r#"
        SELECT m.id, m.name, m.year
        FROM media_items m
        WHERE m.library_id = ?
          AND m.item_type = 'Movie'
          AND (
            m.overview IS NULL 
            OR m.overview = ''
            OR NOT EXISTS (
                SELECT 1 FROM images i WHERE i.item_id = m.id AND i.image_type = 'Primary'
            )
          )
        ORDER BY m.name
        "#,
    )
    .bind(library_id)
    .fetch_all(pool)
    .await?;

    tracing::info!(
        "Found {} movies missing metadata in library {}",
        missing_movies.len(),
        library_id
    );

    // Process movies
    for (movie_id, name, year) in missing_movies {
        result.movies_scanned += 1;

        match metadata_service.get_movie_metadata(&name, year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found metadata for movie '{}' via {:?}",
                    name,
                    meta.provider
                );

                // Update movie metadata
                sqlx::query(
                    r#"UPDATE media_items SET
                        name = COALESCE(?, name),
                        overview = COALESCE(?, overview),
                        year = COALESCE(?, year),
                        premiere_date = COALESCE(?, premiere_date),
                        community_rating = COALESCE(?, community_rating),
                        tmdb_id = COALESCE(?, tmdb_id),
                        imdb_id = COALESCE(?, imdb_id),
                        updated_at = CURRENT_TIMESTAMP
                    WHERE id = ?"#,
                )
                .bind(meta.name.as_deref())
                .bind(meta.overview.as_deref())
                .bind(meta.year)
                .bind(meta.premiere_date.as_deref())
                .bind(meta.community_rating)
                .bind(meta.tmdb_id.as_deref())
                .bind(meta.imdb_id.as_deref())
                .bind(&movie_id)
                .execute(pool)
                .await?;

                // Queue images
                if let Some(ref url) = meta.poster_url {
                    let _ = crate::db::queue_image(pool, &movie_id, "Primary", url).await;
                }
                if let Some(ref url) = meta.backdrop_url {
                    let _ = crate::db::queue_image(pool, &movie_id, "Backdrop", url).await;
                }

                // Update genres
                if let Some(ref genres) = meta.genres {
                    for genre_name in genres {
                        if let Ok(genre_id) = get_or_create_genre(pool, genre_name).await {
                            let _ = link_item_genre(pool, &movie_id, &genre_id).await;
                        }
                    }
                }

                result.movies_updated += 1;
            }
            Ok(None) => {
                tracing::debug!("No metadata found for movie '{}'", name);
            }
            Err(e) => {
                tracing::warn!("Error fetching metadata for movie '{}': {}", name, e);
            }
        }
    }

    tracing::info!(
        "Missing metadata scan complete: {}/{} series updated, {}/{} movies updated",
        result.series_updated,
        result.series_scanned,
        result.movies_updated,
        result.movies_scanned
    );

    Ok(result)
}

/// Update media info for items missing runtime_ticks
pub async fn update_missing_media_info(pool: &SqlitePool) -> Result<i32> {
    let items: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, path FROM media_items WHERE path IS NOT NULL AND runtime_ticks IS NULL",
    )
    .fetch_all(pool)
    .await?;

    let count = items.len();
    tracing::info!("Updating media info for {} items", count);

    let mut updated = 0;
    for (id, path) in items {
        match mediainfo::extract_media_info_async(Path::new(&path)).await {
            Ok(info) => {
                if let Some(ticks) = info.duration_ticks {
                    sqlx::query("UPDATE media_items SET runtime_ticks = ? WHERE id = ?")
                        .bind(ticks)
                        .bind(&id)
                        .execute(pool)
                        .await?;
                    updated += 1;
                    tracing::debug!("Updated runtime for {}: {} ticks", path, ticks);
                }
            }
            Err(e) => {
                tracing::warn!("Failed to extract media info for {}: {}", path, e);
            }
        }
    }

    tracing::info!("Updated media info for {} items", updated);
    Ok(updated)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_anime_episode() {
        let filename =
            "[Reaktor] BECK - Mongolian Chop Squad - E01 [1080p][x265][10-bit][Dual-Audio].mkv";
        let parsed = parse_episode_filename(filename).unwrap();

        assert_eq!(parsed.show_name, "BECK - Mongolian Chop Squad");
        assert_eq!(parsed.season, 1);
        assert_eq!(parsed.episode, 1);
    }

    #[test]
    fn test_parse_standard_episode() {
        let filename = "Breaking Bad S01E05.mkv";
        let parsed = parse_episode_filename(filename).unwrap();

        assert_eq!(parsed.show_name, "Breaking Bad");
        assert_eq!(parsed.season, 1);
        assert_eq!(parsed.episode, 5);
    }

    #[test]
    fn test_parse_movie() {
        let parsed = parse_movie_filename("The Matrix (1999).mkv");
        assert_eq!(parsed.title, "The Matrix");
        assert_eq!(parsed.year, Some(1999));
    }

    #[test]
    fn test_extract_year_from_name() {
        // Standard case
        let (name, year) = extract_year_from_name("My Happy Marriage (2023)");
        assert_eq!(name, "My Happy Marriage");
        assert_eq!(year, Some(2023));

        // No year
        let (name, year) = extract_year_from_name("Some Show");
        assert_eq!(name, "Some Show");
        assert_eq!(year, None);

        // Multiple parentheses
        let (name, year) = extract_year_from_name("Show (Reboot) (2024)");
        assert_eq!(name, "Show (Reboot)");
        assert_eq!(year, Some(2024));

        // Invalid year
        let (name, year) = extract_year_from_name("Show (abc)");
        assert_eq!(name, "Show (abc)");
        assert_eq!(year, None);

        // Scene-style folder name with dots
        let (name, year) = extract_year_from_name("Himouto.Umaru.chan.S01.1080p.BluRay.x265-smol");
        assert_eq!(name, "Himouto Umaru chan");
        assert_eq!(year, None);

        // Scene-style folder name with season info
        let (name, year) = extract_year_from_name(
            "Scissor.Seven.S01-S03.1080p.NF.WEB-DL.AAC2.0.H.264.MULTi-VARYG",
        );
        assert_eq!(name, "Scissor Seven");
        assert_eq!(year, None);
    }

    #[test]
    fn test_clean_folder_name() {
        assert_eq!(
            clean_folder_name("Himouto.Umaru.chan.S01.1080p.BluRay.x265-smol"),
            "Himouto Umaru chan"
        );
        assert_eq!(
            clean_folder_name("Re.ZERO.Starting.Life.in.Another.World."),
            "Re ZERO Starting Life in Another World"
        );
        assert_eq!(clean_folder_name("Link Click (2021)"), "Link Click (2021)");

        // Scene release with bracketed info
        assert_eq!(
            clean_folder_name("To Your Eternity S01 1080p Dual Audio BDRip 10 bits DD x265-EMBER"),
            "To Your Eternity"
        );
        assert_eq!(
            clean_folder_name("Shangri-La Frontier [BD 1080p x265 OPUS][DUAL][Anipakku]"),
            "Shangri-La Frontier"
        );
        // "Complete" is a release keyword and gets removed along with bracketed info
        assert_eq!(
            clean_folder_name(
                "Super Cub - Season 1 Complete [BDRip] [1080p Dual Audio (Eng + Jap)] [Eng Subs]"
            ),
            "Super Cub - Season 1"
        );
    }

    #[test]
    fn test_should_skip_folder() {
        // Exact matches
        assert!(should_skip_folder("NCED"));
        assert!(should_skip_folder("ncop"));
        assert!(should_skip_folder("Extras"));

        // Prefix patterns
        assert!(should_skip_folder("NCED01"));
        assert!(should_skip_folder("ncop2"));

        // Suffix patterns (new functionality)
        assert!(should_skip_folder("Bocchi the Rock! - NCED"));
        assert!(should_skip_folder("Super Cub - Nced"));
        assert!(should_skip_folder("Grand Blue - Ending"));
        assert!(should_skip_folder("Grand Blue - Opening"));
        assert!(should_skip_folder("Initial D - Battle Stage"));
        assert!(should_skip_folder("Initial D - Extra Stage"));

        // Should NOT skip normal folders
        assert!(!should_skip_folder("Blue Box"));
        assert!(!should_skip_folder("Chainsaw Man"));
        assert!(!should_skip_folder("My Happy Marriage (2023)"));
    }

    #[test]
    fn test_normalize_series_name() {
        // Year removal
        assert_eq!(normalize_series_name("Blue Box (2024)"), "blue box");
        assert_eq!(normalize_series_name("Blue Box"), "blue box");

        // Punctuation normalization (colons become spaces, trailing dashes removed by clean)
        assert_eq!(
            normalize_series_name("Re:ZERO -Starting Life in Another World-"),
            "re zero -starting life in another world"
        );

        // Scene release cleaning
        assert_eq!(
            normalize_series_name("Scissor.Seven.S01-S03.1080p.NF.WEB-DL.AAC2.0.H.264.MULTi-VARYG"),
            "scissor seven"
        );
    }

    #[test]
    fn test_folder_name_parsing() {
        // Test clean_folder_name and extract_year_from_name with real-world examples
        let test_cases = vec![
            // (input, expected_clean_name, expected_year)
            ("[Beatrice-Raws] Josee to Tora to Sakana-tachi [BDRip 1920x804 HEVC DTSHD]", "Josee to Tora to Sakana-tachi", None),
            ("[MTBB] Legend of the Galactic Heroes (BD 720p)", "Legend of the Galactic Heroes", None),
            ("[Reaktor] BECK - Mongolian Chop Squad Complete [1080p][x265][10-bit][Dual-Audio]", "BECK - Mongolian Chop Squad", None),
            ("A Wild Last Boss Appeared! (2025)", "A Wild Last Boss Appeared!", Some(2025)),
            ("Blue Box (2024)", "Blue Box", Some(2024)),
            ("BOCCHI THE ROCK! (2022)", "BOCCHI THE ROCK!", Some(2022)),
            ("Scissor.Seven.S01-S03.1080p.NF.WEB-DL.AAC2.0.H.264.MULTi-VARYG", "Scissor Seven", None),
            ("Scissor.Seven.S04.1080p.NF.WEB-DL.AAC2.0.H.264-VARYG", "Scissor Seven", None),
            ("Scissor Seven (2018)", "Scissor Seven", Some(2018)),
            ("Shangri-La Frontier [BD 1080p x265 OPUS][DUAL][Anipakku]", "Shangri-La Frontier", None),
            ("Super Cub - Season 1 Complete [BDRip] [1080p Dual Audio (Eng + Jap)] [Eng Subs]", "Super Cub - Season 1", None),
            ("Himouto.Umaru.chan.S01.1080p.BluRay.Opus2.0.x265-smol", "Himouto Umaru chan", None),
            ("Himouto.Umaru.chan.S02.1080p.BluRay.Opus2.0.x265-smol", "Himouto Umaru chan", None),
            ("Initial D - Complete (1080p) (V2)", "Initial D", None),
            ("JoJo's Bizarre Adventure (2012)", "JoJo's Bizarre Adventure", Some(2012)),
            ("Kimi no Koto ga Daidaidaidaidaisuki na 100-nin no Kanojo", "Kimi no Koto ga Daidaidaidaidaisuki na 100-nin no Kanojo", None),
            ("Grand Blue", "Grand Blue", None),
            ("To Your Eternity S01 1080p Dual Audio BDRip 10 bits DD x265-EMBER", "To Your Eternity", None),
            ("Trapped.in.a.Dating.Sim.S01.1080p.Bluray.Dual-Audio.Opus.2.0.10Bit.x264-Headpatter", "Trapped in a Dating Sim", None),
            ("Violet Evergarden (2018)", "Violet Evergarden", Some(2018)),
            ("Re - ZERO, Starting Life in Another World (2016)", "Re - ZERO, Starting Life in Another World", Some(2016)),
            ("Frieren - Beyond Journey's End (2023)", "Frieren - Beyond Journey's End", Some(2023)),
            ("The Apothecary Diaries (2023)", "The Apothecary Diaries", Some(2023)),
            ("Lycoris Recoil (2022)", "Lycoris Recoil", Some(2022)),
            ("My Dress-Up Darling (2022)", "My Dress-Up Darling", Some(2022)),
            ("Solo Leveling (2024)", "Solo Leveling", Some(2024)),
            ("Overlord (2015)", "Overlord", Some(2015)),
            ("Samurai Champloo (2004)", "Samurai Champloo", Some(2004)),
            ("Link Click (2021)", "Link Click", Some(2021)),
        ];

        println!("\n{:=<100}", "");
        println!("FOLDER NAME PARSING TEST RESULTS");
        println!("{:=<100}\n", "");

        for (input, expected_name, expected_year) in test_cases {
            let (actual_name, actual_year) = extract_year_from_name(input);

            println!("INPUT:    {}", input);
            println!("EXPECTED: {} (year: {:?})", expected_name, expected_year);
            println!("ACTUAL:   {} (year: {:?})", actual_name, actual_year);

            let name_match = actual_name == expected_name;
            let year_match = actual_year == expected_year;

            if name_match && year_match {
                println!("STATUS:   ✓ PASS");
            } else {
                println!("STATUS:   ✗ FAIL");
                if !name_match {
                    println!("          Name mismatch!");
                }
                if !year_match {
                    println!("          Year mismatch!");
                }
            }
            println!();

            assert_eq!(actual_name, expected_name, "Name mismatch for: {}", input);
            assert_eq!(actual_year, expected_year, "Year mismatch for: {}", input);
        }
    }
}
