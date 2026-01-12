# jellyfin-rust

A lightweight Jellyfin-compatible media server written in Rust. Designed for personal use with direct play only (no transcoding).

## Features

- **Jellyfin API compatible** - Works with Jellyfin clients (Fladder, Jellyfin Media Player, etc.)
- **Anime-focused metadata** - AniList, Jikan (MyAnimeList), AniDB, anime-offline-database
- **TMDB integration** - For movies and non-anime series
- **Auto thumbnail generation** - Extracts frames from videos via ffmpeg
- **Background processing** - Image downloads and thumbnails generated asynchronously
- **SQLite database** - Simple, portable storage
- **Direct play only** - No transcoding overhead
- **Memory efficient** - Automatically unloads large datasets after scans

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
tmdb_api_key = "your-api-key"     # Optional, enables TMDB
enable_anime_db = true            # Use anime-offline-database for ID lookup
fetch_episode_metadata = false    # Fetch per-episode metadata (slower, more API calls)

# External tools
[tools]
ffmpeg_path = "/usr/bin/ffmpeg"
ffprobe_path = "/usr/bin/ffprobe"

# Library scanner
[scanner]
enabled = true
quick_scan_interval_minutes = 15      # 0 to disable
full_scan_interval_hours = 24         # 0 to disable
scan_on_startup = false
missing_thumbnail_check_minutes = 60  # Check for missing thumbnails (0 to disable)
retry_failed_thumbnails = true        # Auto-retry failed thumbnail generation

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
| `FETCH_EPISODE_METADATA` | Fetch per-episode metadata (true/false) |
| `FFMPEG_PATH` | Path to ffmpeg binary |
| `FFPROBE_PATH` | Path to ffprobe binary |

## Paths

| Path | Purpose |
|------|---------|
| `~/.config/jellyfin-rust/` | Configuration |
| `~/.local/share/jellyfin-rust/` | Database |
| `~/.local/cache/jellyfin-rust/` | Image cache, anime-offline-database |

## Library Structure

### Recommended TV Show Structure

```
Shows/
├── Show Name (Year)/
│   ├── Season 01/
│   │   ├── Show Name (Year) - S01E01 - Episode Title.mkv
│   │   └── ...
│   ├── Season 02/
│   └── Specials/              # Scanned as Season 0
│       ├── Show Name - S00E01 - OVA.mkv
│       └── ...
```

### Folders That Are Skipped

- `Extras/`, `Extra/`, `Bonus/` - Behind-the-scenes content
- `NCED/`, `NCOP/`, `NC/` - Creditless openings/endings
- Any folder ending with ` - NCED`, ` - NCOP`, etc.
- `Trailers/`, `Featurettes/`, `Samples/`

### Specials Folder

The `Specials/` folder is **not skipped** - it contains legitimate content (OVAs, movies) that are scanned as Season 0 episodes.

## API

Standard Jellyfin endpoints:
- `POST /Users/AuthenticateByName` - Login
- `GET /Items` - Browse library
- `GET /Shows/{id}/Seasons` - Get seasons
- `GET /Shows/{id}/Episodes` - Get episodes
- `GET /Items/{id}/Images/{type}` - Get images
- `GET /Videos/{id}/stream` - Stream video
- `POST /Library/Refresh` - Trigger scan
- `POST /Items/{id}/Refresh` - Refresh item metadata

### Refresh Modes

| Mode | Client Action | Behavior |
|------|---------------|----------|
| `Default` | "Scan for new content" | Quick scan - only finds new/removed files |
| `ValidationOnly` | "Search for missing metadata" | Only scans items missing metadata |
| `FullRefresh` | "Replace all metadata" | Full scan - rescans everything |

## Metadata Providers

### Priority Order for Anime

1. **anime-offline-database** (local, fast ID lookup)
2. **AniList** (by ID if found locally, then search)
3. **Jikan/MyAnimeList** (fallback, no auth required)
4. **AniDB** (by ID only)
5. **TMDB** (if configured)

### Priority Order for Movies/TV

1. **TMDB** (if configured)
2. **AniList** (for anime-style content)
3. **Jikan/MyAnimeList** (fallback)

### anime-offline-database

When `enable_anime_db = true`, the server downloads and caches the [anime-offline-database](https://github.com/manami-project/anime-offline-database) (~58 MB JSON with ~40k entries). This provides:
- Fast local title matching
- Cross-referenced IDs (AniList, AniDB, MAL, Kitsu)
- Year validation to avoid incorrect matches

The database is loaded into memory during library scans and automatically unloaded afterward to free memory.

### Jikan (MyAnimeList)

Jikan is used as a fallback provider when AniList doesn't have a match. It provides:
- No authentication required
- Access to MyAnimeList's extensive database
- Rate limited to 3 requests/second

## Memory Management

The server is designed to be memory-efficient:

- **anime-offline-database** (~50-60 MB) is loaded only during scans and unloaded after
- **SQLite memory** is shrunk after large operations
- **malloc_trim** is called on Linux to return memory to the OS
- Background tasks use separate, lightweight metadata services

## Troubleshooting

### High Memory Usage After Scan

Memory should drop after scans complete. Check logs for:
```
Anime offline database unloaded from memory
Called malloc_trim to release memory to OS
SQLite memory shrunk
```

### Missing Metadata

1. Check if anime-offline-database is enabled: `enable_anime_db = true`
2. Try "Search for missing metadata" scan in your client
3. Check logs for rate limiting messages from AniList/Jikan

### Duplicate Series

The scanner normalizes series names to prevent duplicates. If you have:
- `Scissor Seven (2018)/` AND `Scissor.Seven.S01-S03.../`

Consider consolidating into one folder structure.

## License

MIT
