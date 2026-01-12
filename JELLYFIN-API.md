# Jellyfin-Rust API Reference

This document lists all Jellyfin-compatible API endpoints currently implemented in jellyfin-rust.

## Compatibility

This server aims for compatibility with Jellyfin clients, particularly **Fladder** (Flutter-based Jellyfin client). It mimics Jellyfin API version **10.11.5**.

---

## Authentication

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/Users/AuthenticateByName` | Login with username/password |
| GET | `/Users/Me` | Get current authenticated user |
| GET | `/Users` | List all users (admin) |
| GET | `/Users/Public` | List public users for login screen |
| POST | `/Sessions/Logout` | Logout current session |

---

## System

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/` | Server alive check |
| HEAD | `/` | Server alive check (no body) |
| GET | `/health` | Health check endpoint |
| GET | `/System/Info` | Full system information |
| GET | `/System/Info/Public` | Public system info (no auth required) |
| GET | `/System/Info/Storage` | Storage/disk usage info (admin) |
| GET | `/System/Configuration` | Server configuration |
| POST | `/System/Restart` | Restart server (admin) |
| POST | `/System/Shutdown` | Shutdown server (admin) |

---

## Scheduled Tasks

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/ScheduledTasks` | List all scheduled tasks |
| POST | `/ScheduledTasks/{taskId}/Triggers` | Update task triggers |
| POST | `/ScheduledTasks/Running/{taskId}` | Start a task |
| DELETE | `/ScheduledTasks/Running/{taskId}` | Stop a running task |

---

## Libraries

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Library/VirtualFolders` | List all libraries |
| POST | `/Library/VirtualFolders` | Create a new library |
| DELETE | `/Library/VirtualFolders` | Delete a library |
| POST | `/Library/VirtualFolders/LibraryOptions` | Update library options |
| POST | `/Library/VirtualFolders/Refresh` | Refresh all libraries |

---

## Items

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Items` | Query items with filters |
| GET | `/Items/Counts` | Get item counts by type (movies, series, episodes) |
| GET | `/Items/{id}` | Get single item details |
| GET | `/Items/{id}/Similar` | Get similar items (by genre) |
| POST | `/Items/{id}/Refresh` | Refresh item/library metadata |
| GET | `/Users/{userId}/Items` | Query items for a user |
| GET | `/Users/{userId}/Items/{itemId}` | Get item with user data |
| GET | `/Users/{userId}/Items/Latest` | Get latest added items |
| GET | `/Search/Hints` | Search items by query |

### Item Refresh Modes

The `/Items/{id}/Refresh` endpoint supports:
- `metadataRefreshMode=Default` - Scan for new files only
- `metadataRefreshMode=ValidationOnly` - Fill missing metadata
- `metadataRefreshMode=FullRefresh` - Replace all metadata

---

## Images

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Items/{id}/Images/{type}` | Get item image (Primary, Backdrop, etc.) |
| GET | `/Items/{id}/Images/{type}/{index}` | Get item image by index |
| GET | `/Users/{userId}/Images/{type}` | Get user profile image |
| GET | `/Persons/{id}/Images/{type}` | Get person/actor image |

Image types: `Primary`, `Backdrop`, `Banner`, `Thumb`, `Logo`

---

## TV Shows

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Shows/{seriesId}/Seasons` | Get seasons for a series |
| GET | `/Shows/{seriesId}/Episodes` | Get episodes (optionally by season) |
| GET | `/Shows/NextUp` | Get next unwatched episodes |

---

## Playback

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Items/{id}/PlaybackInfo` | Get playback info for item |
| POST | `/Items/{id}/PlaybackInfo` | Get playback info (POST variant) |
| GET | `/Videos/{id}/stream` | Stream video file |
| GET | `/Videos/{id}/stream.{container}` | Stream with container hint |
| GET | `/Videos/{id}/original` | Direct stream original file |
| GET | `/Videos/{id}/original.{container}` | Direct stream with container |
| GET | `/Videos/{itemId}/{mediaSourceId}/Subtitles/{index}/Stream.{format}` | Get subtitle stream |

---

## Playback Reporting

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/Sessions/Playing` | Report playback started |
| POST | `/Sessions/Playing/Progress` | Report playback progress |
| POST | `/Sessions/Playing/Stopped` | Report playback stopped |
| POST | `/Users/{userId}/PlayedItems/{itemId}` | Mark item as played |
| DELETE | `/Users/{userId}/PlayedItems/{itemId}` | Mark item as unplayed |

---

## Resume & Continue Watching

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/UserItems/Resume` | Get items to resume watching |
| GET | `/Shows/NextUp` | Get next episode to watch |

---

## User Views

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/UserViews` | Get user's library views |

---

## Favorites

| Method | Endpoint | Description |
|--------|----------|-------------|
| POST | `/UserFavoriteItems/{itemId}` | Add item to favorites |
| DELETE | `/UserFavoriteItems/{itemId}` | Remove from favorites |

---

## Collections

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Collections` | List collections |
| POST | `/Collections` | Create collection |
| GET | `/Collections/{id}` | Get collection details |
| DELETE | `/Collections/{id}` | Delete collection |
| GET | `/Collections/{id}/Items` | Get items in collection |
| POST | `/Collections/{id}/Items` | Add items to collection |
| DELETE | `/Collections/{id}/Items` | Remove items from collection |

---

## Playlists

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Playlists` | List user's playlists |
| POST | `/Playlists` | Create playlist |
| GET | `/Playlists/{id}` | Get playlist details |
| DELETE | `/Playlists/{id}` | Delete playlist |
| GET | `/Playlists/{id}/Items` | Get items in playlist |
| POST | `/Playlists/{id}/Items` | Add items to playlist |
| DELETE | `/Playlists/{id}/Items` | Remove items from playlist |

---

## Persons (Cast & Crew)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Persons` | List persons/actors |
| GET | `/Persons/{id}` | Get person details |
| GET | `/Persons/{id}/Images/{type}` | Get person image |

---

## Genres & Studios

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Genres` | List all genres |
| GET | `/Genres/{name}` | Get genre details |
| GET | `/Studios` | List all studios |
| GET | `/Studios/{name}` | Get studio details |

---

## Media Segments (Intro/Outro Skip)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/MediaSegments/{itemId}` | Get segments for item |
| POST | `/MediaSegments/{itemId}` | Create segment |
| DELETE | `/MediaSegments/{itemId}/{segmentId}` | Delete segment |

Supports EDL file import for intro/outro times.

---

## Sessions

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Sessions` | List active sessions |
| POST | `/Sessions/{sessionId}/Playing/{command}` | Send playback command |
| POST | `/Sessions/{sessionId}/System/{command}` | Send system command |
| POST | `/Sessions/{sessionId}/Message` | Send message to session |

---

## Localization

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Localization/Cultures` | List available cultures/languages |
| GET | `/Localization/Countries` | List countries |
| GET | `/Localization/ParentalRatings` | List parental ratings |
| GET | `/Localization/Options` | Get localization options |

---

## Branding

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/Branding/Configuration` | Get branding configuration |
| GET | `/Branding/Css` | Get custom CSS |
| GET | `/Branding/Css.css` | Get custom CSS (alternate) |

---

## Display Preferences (Stub)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/DisplayPreferences/{id}` | Get display preferences |
| POST | `/DisplayPreferences/{id}` | Update display preferences |

---

## QuickConnect (Stub)

| Method | Endpoint | Description |
|--------|----------|-------------|
| GET | `/QuickConnect/Enabled` | Check if QuickConnect is enabled (returns false) |

---

## Not Yet Implemented

The following Jellyfin APIs are **not yet implemented**:

| API | Endpoint | Notes |
|-----|----------|-------|
| Movie Recommendations | `/Movies/Recommendations` | Planned |
| Remote Images | `/Items/{id}/RemoteImages` | Search for remote images |
| Item Download | `/Items/{id}/Download` | Download media files |
| Trickplay | `/Videos/{id}/Trickplay/*` | Seek preview thumbnails |
| External IDs | `/Items/{id}/ExternalIdInfos` | AniList/MAL/TMDB IDs |
| Activity Log | `/System/ActivityLog/Entries` | Activity logging |
| Notifications | `/Notifications` | User notifications |
| Live TV | `/LiveTv/*` | Not planned |
| Sync | `/Sync/*` | Not planned |
| Devices | `/Devices` | Device management |

---

## Metadata Providers

Currently supported metadata providers:

- **AniList** - Anime series metadata and images
- **TMDB** - Movies and TV series (requires API key in config)

Person/cast data is fetched from:
- AniList (voice actors for anime)
- TMDB (cast for movies/TV)

---

## Query Parameters

Common query parameters supported across endpoints:

| Parameter | Description |
|-----------|-------------|
| `userId` | User ID for personalized data |
| `parentId` | Filter by parent item (library) |
| `includeItemTypes` | Comma-separated types (Series, Movie, Episode, etc.) |
| `excludeItemTypes` | Types to exclude |
| `recursive` | Include nested items |
| `sortBy` | Sort field (SortName, DateCreated, PremiereDate, etc.) |
| `sortOrder` | Ascending or Descending |
| `startIndex` | Pagination offset |
| `limit` | Max items to return |
| `searchTerm` | Search query |
| `fields` | Additional fields to include |
| `isFavorite` | Filter by favorite status |
| `genres` | Filter by genre |
| `years` | Filter by year |
| `filters` | Special filters (IsPlayed, IsUnplayed, IsFavorite, etc.) |

---

*Last updated: January 2026*
