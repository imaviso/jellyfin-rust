# jellyfin-rust

A lightweight Jellyfin-compatible media server written in Rust. Designed for personal use with direct play only (no transcoding).

## Features

- **Jellyfin API compatible** - Works with Jellyfin clients (Fladder, etc.)
- **Anime-focused metadata** - AniList, AniDB, anime-offline-database
- **TMDB integration** - For movies and non-anime series
- **Auto thumbnail generation** - Extracts frames from videos via ffmpeg
- **Background processing** - Image downloads and thumbnails generated asynchronously
- **SQLite database** - Simple, portable storage
- **Direct play only** - No transcoding overhead

## Requirements

- Rust 1.70+
- ffmpeg/ffprobe (for media info and thumbnails)
- SQLite

## Quick Start

```bash
# Build
cargo build --release

# Run
RUST_LOG=info ./target/release/jellyfin-rust

# Server starts on port 8096
# Default credentials: admin/admin
```

### Systemd Service

```bash
# Create user
sudo useradd -r -s /bin/false jellyfin

# Copy binary
sudo mkdir -p /opt/jellyfin-rust
sudo cp target/release/jellyfin-rust /opt/jellyfin-rust/

# Install service (edit paths in file first)
sudo cp jellyfin-rust.service /etc/systemd/system/
sudo systemctl daemon-reload
sudo systemctl enable jellyfin-rust
sudo systemctl start jellyfin-rust

# Check status
sudo systemctl status jellyfin-rust
sudo journalctl -u jellyfin-rust -f
```

## Configuration

Config file: `~/.config/jellyfin-rust/config.toml`

```toml
# Server settings
[server]
port = 8096
bind_address = "0.0.0.0"

# Override default paths (optional)
[paths]
data_dir = "/path/to/data"      # Database location
cache_dir = "/path/to/cache"    # Image cache
config_dir = "/path/to/config"  # Config files

# Metadata providers
[metadata]
tmdb_api_key = "your-api-key"   # Optional, enables TMDB
enable_anime_db = true          # Use anime-offline-database

# External tools
[tools]
ffmpeg_path = "/usr/bin/ffmpeg"
ffprobe_path = "/usr/bin/ffprobe"

# Library scanner
[scanner]
enabled = true
quick_scan_interval_minutes = 15  # 0 to disable
full_scan_interval_hours = 24     # 0 to disable
scan_on_startup = false

# Auto-create libraries on startup
[[libraries]]
name = "Anime"
path = "/mnt/media/Anime"
type = "tvshows"

[[libraries]]
name = "Movies"
path = "/mnt/media/Movies"
type = "movies"
```

### Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level: error, warn, info, debug, trace |
| `TMDB_API_KEY` | TMDB API key (alternative to config) |
| `ENABLE_ANIME_DB` | Enable anime-offline-database (true/false) |
| `FFMPEG_PATH` | Path to ffmpeg binary |
| `FFPROBE_PATH` | Path to ffprobe binary |

## Paths

| Path | Purpose |
|------|---------|
| `~/.config/jellyfin-rust/` | Configuration |
| `~/.local/share/jellyfin-rust/` | Database |
| `~/.local/cache/jellyfin-rust/` | Image cache, anime-offline-database |

## API

Standard Jellyfin endpoints:
- `POST /Users/AuthenticateByName` - Login
- `GET /Items` - Browse library
- `GET /Shows/{id}/Seasons` - Get seasons
- `GET /Shows/{id}/Episodes` - Get episodes
- `GET /Items/{id}/Images/{type}` - Get images
- `GET /Videos/{id}/stream` - Stream video
- `POST /Library/Refresh` - Trigger scan

## Metadata Providers

Priority order for anime:
1. anime-offline-database (local, fast)
2. AniList (by ID if found locally)
3. AniList (search)
4. AniDB
5. TMDB

### anime-offline-database

When `enable_anime_db = true`, the server downloads and caches the [anime-offline-database](https://github.com/manami-project/anime-offline-database) (~58 MB JSON with ~40k entries). This provides:
- Fast local title matching
- Cross-referenced IDs (AniList, AniDB, MAL, Kitsu)
- Year validation to avoid incorrect matches

The database is loaded into memory during library scans (~150-250 MB RAM) and automatically unloaded afterward to free memory.

## License

MIT
