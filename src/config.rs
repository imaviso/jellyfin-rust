// Configuration module for jellyfin-rust
// Handles XDG-compliant directory paths and TOML configuration file

use serde::Deserialize;
use std::path::PathBuf;

const APP_NAME: &str = "jellyfin-rust";
const CONFIG_FILENAME: &str = "config.toml";

/// TOML configuration file structure
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ConfigFile {
    /// Server configuration
    pub server: ServerConfig,

    /// Directory paths (overrides XDG defaults)
    pub paths: PathsConfig,

    /// Metadata provider configuration
    pub metadata: MetadataConfig,

    /// External tools configuration
    pub tools: ToolsConfig,

    /// Scanner/library refresh configuration
    pub scanner: ScannerConfig,

    /// Media libraries to auto-create on startup
    pub libraries: Vec<LibraryConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    /// Server port (default: 8096)
    pub port: u16,

    /// Bind address (default: 0.0.0.0)
    pub bind_address: String,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 8096,
            bind_address: "0.0.0.0".to_string(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PathsConfig {
    /// Override data directory (database location)
    pub data_dir: Option<PathBuf>,

    /// Override cache directory (images, anime-offline-database)
    pub cache_dir: Option<PathBuf>,

    /// Override config directory
    pub config_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct MetadataConfig {
    /// TMDB API key (optional, enables TMDB metadata)
    pub tmdb_api_key: Option<String>,

    /// Enable anime-offline-database for AniDB cross-referencing
    pub enable_anime_db: bool,

    /// Fetch per-episode metadata from providers (default: false)
    /// When disabled, episodes only get basic info (name, season/episode number)
    /// Disabling reduces API calls significantly for large libraries
    pub fetch_episode_metadata: bool,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    /// Path to ffmpeg binary
    pub ffmpeg_path: Option<PathBuf>,

    /// Path to ffprobe binary
    pub ffprobe_path: Option<PathBuf>,
}

/// Library configuration for auto-creation on startup
#[derive(Debug, Clone, Deserialize)]
pub struct LibraryConfig {
    /// Library display name
    pub name: String,

    /// Path to the media folder
    pub path: PathBuf,

    /// Library type: "tvshows" or "movies"
    #[serde(rename = "type")]
    pub library_type: String,
}

/// Scanner/library refresh configuration
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ScannerConfig {
    /// Enable periodic background scanning (default: true)
    pub enabled: bool,

    /// Quick scan interval in minutes (default: 15, 0 to disable)
    /// Quick scans only look for new/removed files, no metadata refresh
    pub quick_scan_interval_minutes: u64,

    /// Full scan interval in hours (default: 24, 0 to disable)
    /// Full scans re-fetch metadata for all items
    pub full_scan_interval_hours: u64,

    /// Run a quick scan on startup after library initialization (default: false)
    /// Note: New libraries are always scanned on creation
    pub scan_on_startup: bool,

    /// Video file extensions to scan (lowercase, without dots)
    /// Default: mkv, mp4, avi, mov, wmv, flv, webm, m4v, mpg, mpeg, ts
    pub video_extensions: Vec<String>,

    /// Interval in minutes to check for missing thumbnails (default: 60, 0 to disable)
    /// This task finds episodes/movies without thumbnails and queues them for generation
    pub missing_thumbnail_check_minutes: u64,

    /// Whether to automatically retry failed thumbnail generations (default: true)
    pub retry_failed_thumbnails: bool,
}

impl Default for ScannerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            quick_scan_interval_minutes: 15,
            full_scan_interval_hours: 24,
            scan_on_startup: false,
            video_extensions: vec![
                "mkv".to_string(),
                "mp4".to_string(),
                "avi".to_string(),
                "mov".to_string(),
                "wmv".to_string(),
                "flv".to_string(),
                "webm".to_string(),
                "m4v".to_string(),
                "mpg".to_string(),
                "mpeg".to_string(),
                "ts".to_string(),
                // Additional formats often requested
                "m2ts".to_string(),
                "mts".to_string(),
                "vob".to_string(),
                "ogm".to_string(),
                "ogv".to_string(),
                "divx".to_string(),
                "xvid".to_string(),
                "rmvb".to_string(),
                "rm".to_string(),
                "asf".to_string(),
                "3gp".to_string(),
                "3g2".to_string(),
                "f4v".to_string(),
            ],
            missing_thumbnail_check_minutes: 60,
            retry_failed_thumbnails: true,
        }
    }
}

/// Application paths following XDG Base Directory Specification on Unix
/// On other platforms, falls back to the current directory or platform-specific locations
#[derive(Debug, Clone)]
pub struct AppPaths {
    /// Directory for configuration files (config.toml)
    /// XDG: $XDG_CONFIG_HOME/jellyfin-rust or ~/.config/jellyfin-rust
    pub config_dir: PathBuf,

    /// Directory for persistent data (database, etc.)
    /// XDG: $XDG_DATA_HOME/jellyfin-rust or ~/.local/share/jellyfin-rust
    pub data_dir: PathBuf,

    /// Directory for cache files (images, anime-offline-database, etc.)
    /// XDG: $XDG_CACHE_HOME/jellyfin-rust or ~/.cache/jellyfin-rust
    pub cache_dir: PathBuf,
}

impl AppPaths {
    /// Create application paths using XDG directories (or fallbacks)
    ///
    /// Priority order:
    /// 1. Environment variables (JELLYFIN_RUST_CONFIG_DIR, JELLYFIN_RUST_DATA_DIR, JELLYFIN_RUST_CACHE_DIR)
    /// 2. Config file overrides
    /// 3. XDG directories (Linux/Unix)
    /// 4. Platform-specific directories (macOS, Windows)
    /// 5. Current directory fallback
    pub fn new(config_overrides: &PathsConfig) -> Self {
        let config_dir = Self::resolve_config_dir(&config_overrides.config_dir);
        let data_dir = Self::resolve_data_dir(&config_overrides.data_dir);
        let cache_dir = Self::resolve_cache_dir(&config_overrides.cache_dir);

        Self {
            config_dir,
            data_dir,
            cache_dir,
        }
    }

    /// Create application paths using current directory (legacy/portable mode)
    /// Useful for development or portable installations
    pub fn current_dir() -> Self {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        Self {
            config_dir: cwd.clone(),
            data_dir: cwd.clone(),
            cache_dir: cwd.join("cache"),
        }
    }

    /// Resolve config directory
    fn resolve_config_dir(config_override: &Option<PathBuf>) -> PathBuf {
        // 1. Check environment variable
        if let Ok(path) = std::env::var("JELLYFIN_RUST_CONFIG_DIR") {
            return PathBuf::from(path);
        }

        // 2. Check config file override
        if let Some(ref path) = config_override {
            return path.clone();
        }

        // 3. Use XDG/platform config dir
        if let Some(dir) = dirs::config_dir() {
            return dir.join(APP_NAME);
        }

        // 4. Fallback to current directory
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    /// Resolve data directory
    fn resolve_data_dir(config_override: &Option<PathBuf>) -> PathBuf {
        // 1. Check environment variable
        if let Ok(path) = std::env::var("JELLYFIN_RUST_DATA_DIR") {
            return PathBuf::from(path);
        }

        // 2. Check config file override
        if let Some(ref path) = config_override {
            return path.clone();
        }

        // 3. Use XDG/platform data dir
        if let Some(dir) = dirs::data_dir() {
            return dir.join(APP_NAME);
        }

        // 4. Fallback to current directory
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    /// Resolve cache directory
    fn resolve_cache_dir(config_override: &Option<PathBuf>) -> PathBuf {
        // 1. Check environment variable
        if let Ok(path) = std::env::var("JELLYFIN_RUST_CACHE_DIR") {
            return PathBuf::from(path);
        }

        // 2. Check config file override
        if let Some(ref path) = config_override {
            return path.clone();
        }

        // 3. Use XDG/platform cache dir
        if let Some(dir) = dirs::cache_dir() {
            return dir.join(APP_NAME);
        }

        // 4. Fallback to cache subdirectory in current directory
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join("cache")
    }

    /// Get the database file path
    pub fn database_path(&self) -> PathBuf {
        self.data_dir.join("jellyfin.db")
    }

    /// Get the database URL for SQLite
    pub fn database_url(&self) -> String {
        format!("sqlite:{}?mode=rwc", self.database_path().display())
    }

    /// Get the image cache directory
    pub fn image_cache_dir(&self) -> PathBuf {
        self.cache_dir.join("images")
    }

    /// Get the anime database cache path
    pub fn anime_db_cache_dir(&self) -> PathBuf {
        self.cache_dir.clone()
    }

    /// Get the config file path
    pub fn config_file_path(&self) -> PathBuf {
        self.config_dir.join(CONFIG_FILENAME)
    }

    /// Ensure all directories exist
    pub async fn ensure_dirs(&self) -> std::io::Result<()> {
        tokio::fs::create_dir_all(&self.config_dir).await?;
        tokio::fs::create_dir_all(&self.data_dir).await?;
        tokio::fs::create_dir_all(&self.cache_dir).await?;
        tokio::fs::create_dir_all(self.image_cache_dir()).await?;
        Ok(())
    }

    /// Log the configured paths
    pub fn log_paths(&self) {
        tracing::info!("Configuration directory: {}", self.config_dir.display());
        tracing::info!("Data directory: {}", self.data_dir.display());
        tracing::info!("Cache directory: {}", self.cache_dir.display());
        tracing::debug!("Database path: {}", self.database_path().display());
    }
}

impl Default for AppPaths {
    fn default() -> Self {
        Self::new(&PathsConfig::default())
    }
}

/// Application configuration - combines TOML file with environment overrides
#[derive(Debug, Clone)]
pub struct AppConfig {
    /// Application paths
    pub paths: AppPaths,

    /// Server port
    pub port: u16,

    /// Bind address
    pub bind_address: String,

    /// TMDB API key (optional)
    pub tmdb_api_key: Option<String>,

    /// Whether anime offline database is enabled
    pub anime_db_enabled: bool,

    /// Whether to fetch per-episode metadata
    pub fetch_episode_metadata: bool,

    /// Path to ffmpeg binary
    pub ffmpeg_path: Option<PathBuf>,

    /// Path to ffprobe binary
    pub ffprobe_path: Option<PathBuf>,

    /// Libraries to auto-create on startup
    pub libraries: Vec<LibraryConfig>,

    /// Scanner configuration
    pub scanner: ScannerConfig,
}

impl AppConfig {
    /// Load configuration from TOML file and environment
    ///
    /// Priority (highest to lowest):
    /// 1. Environment variables
    /// 2. TOML config file
    /// 3. Default values
    pub fn load() -> Self {
        // Check if we should use portable mode (current directory for everything)
        let portable_mode = std::env::var("JELLYFIN_RUST_PORTABLE")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false);

        if portable_mode {
            tracing::info!("Running in portable mode (using current directory)");
            return Self::portable();
        }

        // First, determine config directory to find config.toml
        let config_dir = Self::find_config_dir();

        // Try to load config file
        let config_file = Self::load_config_file(&config_dir);

        // Build final configuration with environment overrides
        Self::build(config_file)
    }

    /// Create a portable configuration (current directory for everything)
    fn portable() -> Self {
        let paths = AppPaths::current_dir();
        Self {
            paths,
            port: Self::env_port().unwrap_or(8096),
            bind_address: Self::env_bind_address().unwrap_or_else(|| "0.0.0.0".to_string()),
            tmdb_api_key: std::env::var("TMDB_API_KEY").ok(),
            anime_db_enabled: Self::env_anime_db_enabled(),
            fetch_episode_metadata: Self::env_fetch_episode_metadata(),
            ffmpeg_path: std::env::var("FFMPEG_PATH").ok().map(PathBuf::from),
            ffprobe_path: std::env::var("FFPROBE_PATH").ok().map(PathBuf::from),
            libraries: Vec::new(),
            scanner: ScannerConfig::default(),
        }
    }

    /// Find the config directory (for locating config.toml)
    fn find_config_dir() -> PathBuf {
        // Environment variable takes priority
        if let Ok(path) = std::env::var("JELLYFIN_RUST_CONFIG_DIR") {
            return PathBuf::from(path);
        }

        // Then XDG config dir
        if let Some(dir) = dirs::config_dir() {
            return dir.join(APP_NAME);
        }

        // Fallback to current directory
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    /// Load and parse the TOML config file
    fn load_config_file(config_dir: &std::path::Path) -> ConfigFile {
        let config_path = config_dir.join(CONFIG_FILENAME);

        if !config_path.exists() {
            tracing::debug!(
                "No config file found at {}, using defaults",
                config_path.display()
            );
            return ConfigFile::default();
        }

        match std::fs::read_to_string(&config_path) {
            Ok(contents) => match toml::from_str(&contents) {
                Ok(config) => {
                    tracing::info!("Loaded configuration from {}", config_path.display());
                    config
                }
                Err(e) => {
                    tracing::warn!(
                        "Failed to parse config file {}: {}. Using defaults.",
                        config_path.display(),
                        e
                    );
                    ConfigFile::default()
                }
            },
            Err(e) => {
                tracing::warn!(
                    "Failed to read config file {}: {}. Using defaults.",
                    config_path.display(),
                    e
                );
                ConfigFile::default()
            }
        }
    }

    /// Build configuration from config file with environment overrides
    fn build(config_file: ConfigFile) -> Self {
        let paths = AppPaths::new(&config_file.paths);

        // Port: env > config > default
        let port = Self::env_port().unwrap_or(config_file.server.port);

        // Bind address: env > config > default
        let bind_address =
            Self::env_bind_address().unwrap_or_else(|| config_file.server.bind_address.clone());

        // TMDB API key: env > config
        let tmdb_api_key = std::env::var("TMDB_API_KEY")
            .ok()
            .or(config_file.metadata.tmdb_api_key);

        // Anime DB enabled: env > config
        let anime_db_enabled = if std::env::var("ENABLE_ANIME_DB").is_ok() {
            Self::env_anime_db_enabled()
        } else {
            config_file.metadata.enable_anime_db
        };

        // Fetch episode metadata: env > config
        let fetch_episode_metadata = if std::env::var("FETCH_EPISODE_METADATA").is_ok() {
            Self::env_fetch_episode_metadata()
        } else {
            config_file.metadata.fetch_episode_metadata
        };

        // FFmpeg path: env > config
        let ffmpeg_path = std::env::var("FFMPEG_PATH")
            .ok()
            .map(PathBuf::from)
            .or(config_file.tools.ffmpeg_path);

        // FFprobe path: env > config
        let ffprobe_path = std::env::var("FFPROBE_PATH")
            .ok()
            .map(PathBuf::from)
            .or(config_file.tools.ffprobe_path);

        Self {
            paths,
            port,
            bind_address,
            tmdb_api_key,
            anime_db_enabled,
            fetch_episode_metadata,
            ffmpeg_path,
            ffprobe_path,
            libraries: config_file.libraries,
            scanner: config_file.scanner,
        }
    }

    fn env_port() -> Option<u16> {
        std::env::var("JELLYFIN_RUST_PORT")
            .ok()
            .and_then(|p| p.parse().ok())
    }

    fn env_bind_address() -> Option<String> {
        std::env::var("JELLYFIN_RUST_BIND_ADDRESS").ok()
    }

    fn env_anime_db_enabled() -> bool {
        std::env::var("ENABLE_ANIME_DB")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    }

    fn env_fetch_episode_metadata() -> bool {
        std::env::var("FETCH_EPISODE_METADATA")
            .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
            .unwrap_or(false)
    }

    /// Get the database URL, with override from DATABASE_URL env var
    pub fn database_url(&self) -> String {
        std::env::var("DATABASE_URL").unwrap_or_else(|_| self.paths.database_url())
    }

    /// Log configuration status
    pub fn log_config(&self) {
        self.paths.log_paths();
        tracing::info!("Server listening on {}:{}", self.bind_address, self.port);

        if self.tmdb_api_key.is_some() {
            tracing::info!("Metadata providers: AniList + TMDB");
        } else {
            tracing::info!("Metadata providers: AniList only");
            tracing::info!("Hint: Add tmdb_api_key to config.toml or set TMDB_API_KEY env var");
        }

        if self.anime_db_enabled {
            tracing::info!("Anime offline database: ENABLED");
        } else {
            tracing::debug!("Anime offline database: disabled");
        }

        if self.fetch_episode_metadata {
            tracing::info!("Episode metadata fetching: ENABLED");
        } else {
            tracing::debug!("Episode metadata fetching: disabled (reduces API calls)");
        }

        if let Some(ref path) = self.ffmpeg_path {
            tracing::debug!("FFmpeg: {}", path.display());
        }
        if let Some(ref path) = self.ffprobe_path {
            tracing::debug!("FFprobe: {}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_current_dir_paths() {
        let paths = AppPaths::current_dir();
        assert!(paths.config_dir.is_absolute() || paths.config_dir == PathBuf::from("."));
        assert!(paths.cache_dir.ends_with("cache"));
    }

    #[test]
    fn test_database_url_format() {
        let paths = AppPaths::current_dir();
        let url = paths.database_url();
        assert!(url.starts_with("sqlite:"));
        assert!(url.ends_with("?mode=rwc"));
    }

    #[test]
    fn test_default_config_file() {
        let config = ConfigFile::default();
        assert_eq!(config.server.port, 8096);
        assert_eq!(config.server.bind_address, "0.0.0.0");
        assert!(!config.metadata.enable_anime_db);
        assert!(config.metadata.tmdb_api_key.is_none());
    }

    #[test]
    fn test_parse_config_toml() {
        let toml_str = r#"
[server]
port = 9000
bind_address = "127.0.0.1"

[metadata]
tmdb_api_key = "test_key"
enable_anime_db = true

[paths]
data_dir = "/custom/data"

[tools]
ffmpeg_path = "/usr/bin/ffmpeg"
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.port, 9000);
        assert_eq!(config.server.bind_address, "127.0.0.1");
        assert_eq!(config.metadata.tmdb_api_key, Some("test_key".to_string()));
        assert!(config.metadata.enable_anime_db);
        assert_eq!(config.paths.data_dir, Some(PathBuf::from("/custom/data")));
        assert_eq!(
            config.tools.ffmpeg_path,
            Some(PathBuf::from("/usr/bin/ffmpeg"))
        );
    }

    #[test]
    fn test_partial_config_toml() {
        // Test that partial configs work (only specify what you need)
        let toml_str = r#"
[metadata]
enable_anime_db = true
"#;
        let config: ConfigFile = toml::from_str(toml_str).unwrap();
        assert_eq!(config.server.port, 8096); // default
        assert!(config.metadata.enable_anime_db); // from file
    }
}
