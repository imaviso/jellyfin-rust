use anyhow::{Context, Result};
use sqlx::SqlitePool;

/// Configure SQLite for optimal performance
/// This should be called once per connection, typically via connection options
pub async fn configure_connection(pool: &SqlitePool) -> Result<()> {
    // WAL mode for better concurrent read/write performance
    sqlx::query("PRAGMA journal_mode = WAL")
        .execute(pool)
        .await?;

    // Synchronous NORMAL is safe with WAL and much faster than FULL
    sqlx::query("PRAGMA synchronous = NORMAL")
        .execute(pool)
        .await?;

    // Cache size: -32000 = 32MB (negative = KB)
    sqlx::query("PRAGMA cache_size = -32000")
        .execute(pool)
        .await?;

    // Memory-mapped I/O: 64MB (reduced from 256MB for lower memory footprint)
    sqlx::query("PRAGMA mmap_size = 67108864")
        .execute(pool)
        .await?;

    // Store temp tables in memory
    sqlx::query("PRAGMA temp_store = MEMORY")
        .execute(pool)
        .await?;

    // Enable foreign keys
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(pool)
        .await?;

    // Busy timeout to handle concurrent access (5 seconds)
    sqlx::query("PRAGMA busy_timeout = 5000")
        .execute(pool)
        .await?;

    tracing::info!("SQLite configured: WAL mode, 32MB cache, 64MB mmap");

    Ok(())
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS users (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            password_hash TEXT NOT NULL,
            is_admin INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS sessions (
            token TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            device_id TEXT NOT NULL,
            device_name TEXT NOT NULL,
            client TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS libraries (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            path TEXT NOT NULL,
            library_type TEXT NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS media_items (
            id TEXT PRIMARY KEY,
            library_id TEXT NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
            parent_id TEXT REFERENCES media_items(id) ON DELETE CASCADE,
            item_type TEXT NOT NULL,
            name TEXT NOT NULL,
            path TEXT,
            overview TEXT,
            year INTEGER,
            runtime_ticks INTEGER,
            premiere_date TEXT,
            community_rating REAL,
            tmdb_id TEXT,
            imdb_id TEXT,
            anilist_id TEXT,
            mal_id TEXT,
            anidb_id TEXT,
            kitsu_id TEXT,
            sort_name TEXT,
            index_number INTEGER,
            parent_index_number INTEGER,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS images (
            id TEXT PRIMARY KEY,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            image_type TEXT NOT NULL,
            path TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS playback_progress (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            position_ticks INTEGER NOT NULL DEFAULT 0,
            played INTEGER NOT NULL DEFAULT 0,
            play_count INTEGER NOT NULL DEFAULT 0,
            last_played TEXT,
            PRIMARY KEY (user_id, item_id)
        );

        -- User favorites
        CREATE TABLE IF NOT EXISTS user_favorites (
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            PRIMARY KEY (user_id, item_id)
        );

        -- Display preferences (per user, per client)
        CREATE TABLE IF NOT EXISTS display_preferences (
            id TEXT NOT NULL,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            client TEXT NOT NULL,
            view_type TEXT,
            sort_by TEXT DEFAULT 'SortName',
            sort_order TEXT DEFAULT 'Ascending',
            remember_sorting INTEGER DEFAULT 0,
            index_by TEXT,
            remember_indexing INTEGER DEFAULT 0,
            primary_image_height INTEGER DEFAULT 250,
            primary_image_width INTEGER DEFAULT 250,
            scroll_direction TEXT DEFAULT 'Horizontal',
            show_backdrop INTEGER DEFAULT 1,
            show_sidebar INTEGER DEFAULT 1,
            custom_prefs TEXT,
            PRIMARY KEY (user_id, client, id)
        );

        -- Genres (normalized)
        CREATE TABLE IF NOT EXISTS genres (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS item_genres (
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            genre_id TEXT NOT NULL REFERENCES genres(id) ON DELETE CASCADE,
            PRIMARY KEY (item_id, genre_id)
        );

        -- Studios (normalized)
        CREATE TABLE IF NOT EXISTS studios (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE
        );

        CREATE TABLE IF NOT EXISTS item_studios (
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            studio_id TEXT NOT NULL REFERENCES studios(id) ON DELETE CASCADE,
            PRIMARY KEY (item_id, studio_id)
        );

        -- Image download queue for background processing
        CREATE TABLE IF NOT EXISTS image_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            image_type TEXT NOT NULL,
            url TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(item_id, image_type)
        );

        -- Thumbnail generation queue for video files
        CREATE TABLE IF NOT EXISTS thumbnail_queue (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            video_path TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            attempts INTEGER NOT NULL DEFAULT 0,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(item_id)
        );

        -- Collections (user-created groupings of items)
        CREATE TABLE IF NOT EXISTS collections (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            overview TEXT,
            sort_name TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS collection_items (
            collection_id TEXT NOT NULL REFERENCES collections(id) ON DELETE CASCADE,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            sort_order INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (collection_id, item_id)
        );

        -- Media segments (intro/outro/recap markers for skip functionality)
        CREATE TABLE IF NOT EXISTS media_segments (
            id TEXT PRIMARY KEY,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            segment_type TEXT NOT NULL,  -- Intro, Outro, Recap, Preview, Commercial
            start_ticks INTEGER NOT NULL,
            end_ticks INTEGER NOT NULL,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        -- Active playback sessions (for multi-device tracking)
        CREATE TABLE IF NOT EXISTS active_sessions (
            id TEXT PRIMARY KEY,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            device_id TEXT NOT NULL,
            device_name TEXT NOT NULL,
            client TEXT NOT NULL,
            client_version TEXT,
            app_icon_url TEXT,
            now_playing_item_id TEXT REFERENCES media_items(id) ON DELETE SET NULL,
            now_playing_position_ticks INTEGER DEFAULT 0,
            is_paused INTEGER DEFAULT 0,
            is_muted INTEGER DEFAULT 0,
            volume_level INTEGER DEFAULT 100,
            play_method TEXT,
            play_state TEXT,  -- playing, paused, stopped
            last_activity TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(user_id, device_id)
        );

        -- Full-text search virtual table for fast searching
        -- We use FTS5 with content-less mode (external content)
        CREATE VIRTUAL TABLE IF NOT EXISTS media_items_fts USING fts5(
            name,
            overview,
            sort_name,
            content='media_items',
            content_rowid='rowid'
        );

        -- Track series that failed metadata lookup so we can retry them later
        CREATE TABLE IF NOT EXISTS unmatched_series (
            id TEXT PRIMARY KEY,
            library_id TEXT NOT NULL REFERENCES libraries(id) ON DELETE CASCADE,
            series_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            folder_name TEXT NOT NULL,
            attempted_title TEXT,
            attempted_year INTEGER,
            failure_reason TEXT,
            attempt_count INTEGER NOT NULL DEFAULT 1,
            last_attempt_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
            UNIQUE(library_id, series_id)
        );

        -- Playlists (user-created ordered lists of items)
        CREATE TABLE IF NOT EXISTS playlists (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            user_id TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
            media_type TEXT,  -- Video, Audio, Book
            sort_name TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        CREATE TABLE IF NOT EXISTS playlist_items (
            playlist_id TEXT NOT NULL REFERENCES playlists(id) ON DELETE CASCADE,
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            sort_order INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (playlist_id, item_id)
        );

        -- Persons (actors, directors, voice actors, etc.)
        CREATE TABLE IF NOT EXISTS persons (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            role TEXT,  -- Actor, Director, VoiceActor, etc.
            image_url TEXT,
            anilist_id TEXT,
            tmdb_id TEXT,
            sort_name TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        );

        -- Many-to-many relationship between items and persons
        CREATE TABLE IF NOT EXISTS item_persons (
            item_id TEXT NOT NULL REFERENCES media_items(id) ON DELETE CASCADE,
            person_id TEXT NOT NULL REFERENCES persons(id) ON DELETE CASCADE,
            role TEXT,  -- Character name or role in production
            sort_order INTEGER NOT NULL DEFAULT 0,
            PRIMARY KEY (item_id, person_id, role)
        );
        "#,
    )
    .execute(pool)
    .await?;

    // Create indexes in separate statements for better error handling
    create_indexes(pool).await?;

    Ok(())
}

/// Create all database indexes for optimal query performance
async fn create_indexes(pool: &SqlitePool) -> Result<()> {
    let indexes = [
        // =========================================
        // Core media_items indexes
        // =========================================

        // Library browsing: filter by library
        "CREATE INDEX IF NOT EXISTS idx_media_items_library ON media_items(library_id)",

        // Parent-child relationships (episodes -> series, etc.)
        "CREATE INDEX IF NOT EXISTS idx_media_items_parent ON media_items(parent_id)",

        // Filter by type (Series, Episode, Movie, Season)
        "CREATE INDEX IF NOT EXISTS idx_media_items_type ON media_items(item_type)",

        // Composite: library + type (common filter combination)
        "CREATE INDEX IF NOT EXISTS idx_media_items_library_type ON media_items(library_id, item_type)",

        // Sort by name
        "CREATE INDEX IF NOT EXISTS idx_media_items_sort_name ON media_items(sort_name)",

        // Sort by year
        "CREATE INDEX IF NOT EXISTS idx_media_items_year ON media_items(year)",

        // Sort by community rating
        "CREATE INDEX IF NOT EXISTS idx_media_items_rating ON media_items(community_rating)",

        // Sort by date added (created_at)
        "CREATE INDEX IF NOT EXISTS idx_media_items_created ON media_items(created_at)",

        // Sort by premiere date
        "CREATE INDEX IF NOT EXISTS idx_media_items_premiere ON media_items(premiere_date)",

        // Episode ordering within a series
        "CREATE INDEX IF NOT EXISTS idx_media_items_episode_order ON media_items(parent_id, parent_index_number, index_number)",

        // Provider ID lookups (for metadata matching)
        "CREATE INDEX IF NOT EXISTS idx_media_items_tmdb ON media_items(tmdb_id) WHERE tmdb_id IS NOT NULL",
        "CREATE INDEX IF NOT EXISTS idx_media_items_imdb ON media_items(imdb_id) WHERE imdb_id IS NOT NULL",
        "CREATE INDEX IF NOT EXISTS idx_media_items_anilist ON media_items(anilist_id) WHERE anilist_id IS NOT NULL",

        // =========================================
        // Images indexes
        // =========================================

        // Get images for an item
        "CREATE INDEX IF NOT EXISTS idx_images_item ON images(item_id)",

        // Get specific image type for an item
        "CREATE INDEX IF NOT EXISTS idx_images_item_type ON images(item_id, image_type)",

        // =========================================
        // Playback progress indexes
        // =========================================

        // Get user's playback progress (already has PK, but add for played status queries)
        "CREATE INDEX IF NOT EXISTS idx_playback_user ON playback_progress(user_id)",

        // Resume watching: find items with progress
        "CREATE INDEX IF NOT EXISTS idx_playback_position ON playback_progress(user_id, position_ticks) WHERE position_ticks > 0",

        // Recently played
        "CREATE INDEX IF NOT EXISTS idx_playback_last_played ON playback_progress(user_id, last_played) WHERE last_played IS NOT NULL",

        // Played items (for filtering)
        "CREATE INDEX IF NOT EXISTS idx_playback_played ON playback_progress(user_id, played) WHERE played = 1",

        // =========================================
        // User favorites indexes
        // =========================================

        // Get user's favorites
        "CREATE INDEX IF NOT EXISTS idx_favorites_user ON user_favorites(user_id)",

        // Check if specific item is favorite
        "CREATE INDEX IF NOT EXISTS idx_favorites_item ON user_favorites(item_id)",

        // =========================================
        // Sessions indexes
        // =========================================

        // Find sessions by user
        "CREATE INDEX IF NOT EXISTS idx_sessions_user ON sessions(user_id)",

        // =========================================
        // Genre/Studio relationship indexes
        // =========================================

        // Find items by genre
        "CREATE INDEX IF NOT EXISTS idx_item_genres_genre ON item_genres(genre_id)",

        // Find items by studio
        "CREATE INDEX IF NOT EXISTS idx_item_studios_studio ON item_studios(studio_id)",

        // =========================================
        // Libraries indexes
        // =========================================

        // Find library by path (for auto-creation check)
        "CREATE INDEX IF NOT EXISTS idx_libraries_path ON libraries(path)",

        // =========================================
        // Collections indexes
        // =========================================

        // Find items in a collection
        "CREATE INDEX IF NOT EXISTS idx_collection_items_collection ON collection_items(collection_id)",

        // Find collections containing an item
        "CREATE INDEX IF NOT EXISTS idx_collection_items_item ON collection_items(item_id)",

        // =========================================
        // Media segments indexes
        // =========================================

        // Find segments for an item (for skip functionality)
        "CREATE INDEX IF NOT EXISTS idx_media_segments_item ON media_segments(item_id)",

        // Find by segment type
        "CREATE INDEX IF NOT EXISTS idx_media_segments_type ON media_segments(item_id, segment_type)",

        // =========================================
        // Active sessions indexes
        // =========================================

        // Find sessions by user
        "CREATE INDEX IF NOT EXISTS idx_active_sessions_user ON active_sessions(user_id)",

        // Find sessions with active playback
        "CREATE INDEX IF NOT EXISTS idx_active_sessions_playing ON active_sessions(now_playing_item_id) WHERE now_playing_item_id IS NOT NULL",

        // Session cleanup (by last activity)
        "CREATE INDEX IF NOT EXISTS idx_active_sessions_activity ON active_sessions(last_activity)",

        // =========================================
        // Unmatched series indexes
        // =========================================

        // Find unmatched series by library
        "CREATE INDEX IF NOT EXISTS idx_unmatched_series_library ON unmatched_series(library_id)",

        // Find unmatched series for retry (oldest attempt first)
        "CREATE INDEX IF NOT EXISTS idx_unmatched_series_retry ON unmatched_series(library_id, last_attempt_at) WHERE attempt_count < 3",

        // =========================================
        // Playlists indexes
        // =========================================

        // Find playlists by user
        "CREATE INDEX IF NOT EXISTS idx_playlists_user ON playlists(user_id)",

        // Find items in a playlist
        "CREATE INDEX IF NOT EXISTS idx_playlist_items_playlist ON playlist_items(playlist_id)",

        // Find playlists containing an item
        "CREATE INDEX IF NOT EXISTS idx_playlist_items_item ON playlist_items(item_id)",

        // =========================================
        // Persons indexes
        // =========================================

        // Find persons by name
        "CREATE INDEX IF NOT EXISTS idx_persons_name ON persons(name)",

        // Find persons by AniList ID
        "CREATE INDEX IF NOT EXISTS idx_persons_anilist ON persons(anilist_id) WHERE anilist_id IS NOT NULL",

        // Find persons for an item
        "CREATE INDEX IF NOT EXISTS idx_item_persons_item ON item_persons(item_id)",

        // Find items featuring a person
        "CREATE INDEX IF NOT EXISTS idx_item_persons_person ON item_persons(person_id)",
    ];

    for index_sql in indexes {
        if let Err(e) = sqlx::query(index_sql).execute(pool).await {
            tracing::warn!("Failed to create index: {} - {}", index_sql, e);
        }
    }

    tracing::debug!("Database indexes created/verified");

    Ok(())
}

/// Optimize the database (run periodically or on demand)
pub async fn optimize(pool: &SqlitePool) -> Result<()> {
    tracing::info!("Running database optimization...");

    sqlx::query("ANALYZE").execute(pool).await?;
    sqlx::query("PRAGMA optimize").execute(pool).await?;

    tracing::info!("Database optimization complete");

    Ok(())
}

/// Release unused memory back to the OS
/// Call after large operations like full library scans
pub async fn shrink_memory(pool: &SqlitePool) -> Result<()> {
    sqlx::query("PRAGMA shrink_memory").execute(pool).await?;
    tracing::debug!("SQLite memory shrunk");
    Ok(())
}

/// Queue an image for background download
pub async fn queue_image(
    pool: &SqlitePool,
    item_id: &str,
    image_type: &str,
    url: &str,
) -> Result<()> {
    sqlx::query(
        r#"
        INSERT INTO image_queue (item_id, image_type, url, status)
        VALUES (?, ?, ?, 'pending')
        ON CONFLICT(item_id, image_type) DO UPDATE SET
            url = excluded.url,
            status = 'pending',
            attempts = 0
        "#,
    )
    .bind(item_id)
    .bind(image_type)
    .bind(url)
    .execute(pool)
    .await?;

    Ok(())
}

/// Get pending images from the queue (batch)
pub async fn get_pending_images(pool: &SqlitePool, limit: i32) -> Result<Vec<PendingImage>> {
    let rows: Vec<PendingImage> = sqlx::query_as(
        r#"
        SELECT id, item_id, image_type, url, attempts
        FROM image_queue
        WHERE status = 'pending' AND attempts < 3
        ORDER BY id ASC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(rows)
}

/// Mark an image as downloaded (remove from queue)
pub async fn mark_image_downloaded(pool: &SqlitePool, queue_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM image_queue WHERE id = ?")
        .bind(queue_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark an image download as failed (increment attempts)
pub async fn mark_image_failed(pool: &SqlitePool, queue_id: i64) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE image_queue
        SET attempts = attempts + 1,
            status = CASE WHEN attempts >= 2 THEN 'failed' ELSE 'pending' END
        WHERE id = ?
        "#,
    )
    .bind(queue_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get count of pending images
pub async fn get_pending_image_count(pool: &SqlitePool) -> Result<i64> {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM image_queue WHERE status = 'pending'")
        .fetch_one(pool)
        .await?;
    Ok(row.0)
}

#[derive(Debug, sqlx::FromRow)]
pub struct PendingImage {
    pub id: i64,
    pub item_id: String,
    pub image_type: String,
    pub url: String,
    pub attempts: i32,
}

/// Queue a video file for thumbnail generation
pub async fn queue_thumbnail(pool: &SqlitePool, item_id: &str, video_path: &str) -> Result<()> {
    sqlx::query(
        r#"
        INSERT OR IGNORE INTO thumbnail_queue (item_id, video_path, status)
        VALUES (?, ?, 'pending')
        "#,
    )
    .bind(item_id)
    .bind(video_path)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get pending thumbnails to generate
pub async fn get_pending_thumbnails(
    pool: &SqlitePool,
    limit: i32,
) -> Result<Vec<PendingThumbnail>> {
    let rows = sqlx::query_as::<_, PendingThumbnail>(
        r#"
        SELECT id, item_id, video_path, attempts
        FROM thumbnail_queue
        WHERE status = 'pending'
        ORDER BY created_at ASC
        LIMIT ?
        "#,
    )
    .bind(limit)
    .fetch_all(pool)
    .await?;
    Ok(rows)
}

/// Mark a thumbnail as successfully generated
pub async fn mark_thumbnail_complete(pool: &SqlitePool, queue_id: i64) -> Result<()> {
    sqlx::query("DELETE FROM thumbnail_queue WHERE id = ?")
        .bind(queue_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Mark a thumbnail generation as failed (will retry if attempts < 2)
pub async fn mark_thumbnail_failed(pool: &SqlitePool, queue_id: i64) -> Result<()> {
    sqlx::query(
        r#"
        UPDATE thumbnail_queue SET
            attempts = attempts + 1,
            status = CASE WHEN attempts >= 2 THEN 'failed' ELSE 'pending' END
        WHERE id = ?
        "#,
    )
    .bind(queue_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Get count of pending thumbnails
pub async fn get_pending_thumbnail_count(pool: &SqlitePool) -> Result<i64> {
    let row: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM thumbnail_queue WHERE status = 'pending'")
            .fetch_one(pool)
            .await?;
    Ok(row.0)
}

#[derive(Debug, sqlx::FromRow)]
pub struct PendingThumbnail {
    pub id: i64,
    pub item_id: String,
    pub video_path: String,
    pub attempts: i32,
}

// ============================================================================
// Full-Text Search helpers
// ============================================================================

/// Rebuild the FTS index from scratch (use after bulk inserts)
/// If the FTS table is corrupted, it will be dropped and recreated
pub async fn rebuild_fts_index(pool: &SqlitePool) -> Result<()> {
    tracing::info!("Rebuilding full-text search index...");

    // Try to delete existing content first
    let delete_result = sqlx::query("DELETE FROM media_items_fts")
        .execute(pool)
        .await;

    // If delete failed (corrupted table), drop and recreate
    if delete_result.is_err() {
        tracing::warn!("FTS table appears corrupted, recreating...");

        // Drop the corrupted table
        if let Err(e) = sqlx::query("DROP TABLE IF EXISTS media_items_fts")
            .execute(pool)
            .await
        {
            tracing::error!("Failed to drop corrupted FTS table: {}", e);
        }

        // Recreate the FTS table
        sqlx::query(
            r#"CREATE VIRTUAL TABLE IF NOT EXISTS media_items_fts USING fts5(
                name,
                overview,
                sort_name,
                content='media_items',
                content_rowid='rowid'
            )"#,
        )
        .execute(pool)
        .await
        .context("Failed to recreate FTS table")?;

        tracing::info!("FTS table recreated");
    }

    // Rebuild from media_items
    sqlx::query(
        r#"
        INSERT INTO media_items_fts(rowid, name, overview, sort_name)
        SELECT rowid, name, COALESCE(overview, ''), COALESCE(sort_name, name)
        FROM media_items
        "#,
    )
    .execute(pool)
    .await?;

    tracing::info!("Full-text search index rebuilt");
    Ok(())
}

/// Update FTS index for a single item (use after individual inserts/updates)
pub async fn update_fts_item(pool: &SqlitePool, item_id: &str) -> Result<()> {
    // Get the rowid for this item
    let row: Option<(i64, String, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT rowid, name, overview, sort_name FROM media_items WHERE id = ?")
            .bind(item_id)
            .fetch_optional(pool)
            .await?;

    if let Some((rowid, name, overview, sort_name)) = row {
        // Delete old FTS entry if exists
        sqlx::query("DELETE FROM media_items_fts WHERE rowid = ?")
            .bind(rowid)
            .execute(pool)
            .await?;

        // Insert new FTS entry
        sqlx::query(
            "INSERT INTO media_items_fts(rowid, name, overview, sort_name) VALUES (?, ?, ?, ?)",
        )
        .bind(rowid)
        .bind(&name)
        .bind(overview.as_deref().unwrap_or(""))
        .bind(sort_name.as_deref().unwrap_or(&name))
        .execute(pool)
        .await?;
    }

    Ok(())
}

/// Search using FTS with ranking
pub async fn search_items_fts(pool: &SqlitePool, query: &str, limit: i32) -> Result<Vec<String>> {
    // Clean and prepare the search query for FTS5
    let fts_query = prepare_fts_query(query);

    if fts_query.is_empty() {
        return Ok(vec![]);
    }

    // Search using FTS5 with BM25 ranking
    let results: Vec<(String,)> = sqlx::query_as(
        r#"
        SELECT m.id
        FROM media_items m
        JOIN media_items_fts f ON m.rowid = f.rowid
        WHERE media_items_fts MATCH ?
        ORDER BY bm25(media_items_fts)
        LIMIT ?
        "#,
    )
    .bind(&fts_query)
    .bind(limit)
    .fetch_all(pool)
    .await?;

    Ok(results.into_iter().map(|(id,)| id).collect())
}

/// Prepare a user query for FTS5
/// Converts "hello world" -> "hello* OR world*" for prefix matching
fn prepare_fts_query(query: &str) -> String {
    let terms: Vec<String> = query
        .split_whitespace()
        .filter(|s| !s.is_empty() && s.len() >= 2)
        .map(|s| {
            // Escape special FTS5 characters and add prefix matching
            let escaped = s.replace(['"', '\'', '*'], "").replace('-', " ");
            format!("\"{}\"*", escaped)
        })
        .collect();

    terms.join(" OR ")
}
