use anyhow::Result;
use axum::{routing::get, Router};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

mod api;
mod config;
mod db;
mod models;
mod scanner;
mod services;

use config::AppConfig;

pub struct AppState {
    pub db: sqlx::SqlitePool,
    pub config: AppConfig,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "jellyfin_rust=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Load .env file if present
    dotenvy::dotenv().ok();

    let config = AppConfig::load();

    config.paths.ensure_dirs().await?;

    config.log_config();

    // Database setup with optimized connection pool
    let database_url = config.database_url();
    tracing::debug!("Database URL: {}", database_url);

    let connect_options = SqliteConnectOptions::from_str(&database_url)?
        .create_if_missing(true)
        // Enable WAL mode for better concurrent performance
        .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
        // NORMAL sync is safe with WAL and much faster
        .synchronous(sqlx::sqlite::SqliteSynchronous::Normal)
        // Increase page size for better I/O performance (4KB -> 8KB)
        .page_size(8192)
        // Enable foreign key enforcement
        .foreign_keys(true)
        // Busy timeout for concurrent access (5 seconds)
        .busy_timeout(Duration::from_secs(5));

    let pool = SqlitePoolOptions::new()
        .max_connections(10)
        .min_connections(2)
        .acquire_timeout(Duration::from_secs(5))
        .idle_timeout(Duration::from_secs(600))
        .max_lifetime(Duration::from_secs(1800))
        .test_before_acquire(true)
        .connect_with(connect_options)
        .await?;

    // Configure additional SQLite PRAGMAs not available in connect options
    db::configure_connection(&pool).await?;

    db::migrate(&pool).await?;

    // Create default admin user if no users exist
    let user_count: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM users")
        .fetch_one(&pool)
        .await?;

    if user_count.0 == 0 {
        tracing::info!("No users found, creating default admin user");
        services::auth::create_user(&pool, "admin", "admin", true).await?;
        tracing::info!("Created default admin user (username: admin, password: admin)");
    }

    let state = std::sync::Arc::new(AppState {
        db: pool.clone(),
        config: config.clone(),
    });

    // Detect CPU cores and calculate optimal batch sizes for background tasks
    let cpu_cores = std::thread::available_parallelism()
        .map(|p| p.get())
        .unwrap_or(4);

    let thumbnail_batch_size = (cpu_cores * 2).clamp(10, 40) as i32;
    let image_batch_size = (cpu_cores * 3).clamp(15, 60) as i32;

    tracing::info!(
        "Detected {} CPU cores: thumbnail batch={}, image batch={}",
        cpu_cores,
        thumbnail_batch_size,
        image_batch_size
    );

    // Spawn background task for library auto-creation and scanning
    if !config.libraries.is_empty() {
        let bg_pool = pool.clone();
        let bg_config = config.clone();
        tokio::spawn(async move {
            // Small delay to let the server fully start
            tokio::time::sleep(Duration::from_millis(500)).await;

            tracing::info!(
                "Background: Checking {} configured libraries...",
                bg_config.libraries.len()
            );

            for lib in &bg_config.libraries {
                // Check if library with same path already exists
                let existing: Option<(String,)> =
                    match sqlx::query_as("SELECT id FROM libraries WHERE path = ?")
                        .bind(lib.path.to_str().unwrap_or_default())
                        .fetch_optional(&bg_pool)
                        .await
                    {
                        Ok(result) => result,
                        Err(e) => {
                            tracing::error!("Failed to check library '{}': {}", lib.name, e);
                            continue;
                        }
                    };

                if existing.is_none() {
                    let lib_type = lib.library_type.to_lowercase();
                    if lib_type != "tvshows" && lib_type != "movies" {
                        tracing::warn!(
                            "Skipping library '{}': invalid type '{}' (must be 'tvshows' or 'movies')",
                            lib.name,
                            lib.library_type
                        );
                        continue;
                    }

                    if !lib.path.exists() {
                        tracing::warn!(
                            "Skipping library '{}': path does not exist: {}",
                            lib.name,
                            lib.path.display()
                        );
                        continue;
                    }

                    let library_id = uuid::Uuid::new_v4().to_string();
                    tracing::info!(
                        "Creating library '{}' ({}) at {}",
                        lib.name,
                        lib_type,
                        lib.path.display()
                    );

                    if let Err(e) = sqlx::query(
                        "INSERT INTO libraries (id, name, path, library_type) VALUES (?, ?, ?, ?)",
                    )
                    .bind(&library_id)
                    .bind(&lib.name)
                    .bind(lib.path.to_str().unwrap_or_default())
                    .bind(&lib_type)
                    .execute(&bg_pool)
                    .await
                    {
                        tracing::error!("Failed to create library '{}': {}", lib.name, e);
                        continue;
                    }

                    tracing::info!("Background: Scanning new library '{}'...", lib.name);
                    match scanner::scan_library_with_cache_dir(
                        &bg_pool,
                        &library_id,
                        lib.path.to_str().unwrap_or_default(),
                        &lib_type,
                        bg_config.paths.cache_dir.clone(),
                        Some(bg_config.anime_db_enabled),
                    )
                    .await
                    {
                        Ok(result) => {
                            tracing::info!(
                                "Background: Library '{}' scan complete: {} series, {} episodes, {} movies",
                                lib.name,
                                result.series_added,
                                result.episodes_added,
                                result.movies_added
                            );
                        }
                        Err(e) => {
                            tracing::error!(
                                "Background: Failed to scan library '{}': {}",
                                lib.name,
                                e
                            );
                        }
                    }
                } else {
                    tracing::debug!("Library '{}' already exists, skipping", lib.name);
                }
            }

            tracing::info!("Background: Library initialization complete");

            tracing::info!("Background: Rebuilding full-text search index...");
            if let Err(e) = db::rebuild_fts_index(&bg_pool).await {
                tracing::error!("Failed to rebuild FTS index: {}", e);
            }
        });
    } else {
        // Even if no libraries configured, rebuild FTS on startup for existing data
        let fts_pool = pool.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            tracing::info!("Background: Rebuilding full-text search index...");
            if let Err(e) = db::rebuild_fts_index(&fts_pool).await {
                tracing::error!("Failed to rebuild FTS index: {}", e);
            }
        });
    }

    // Spawn periodic scanner task if enabled
    if config.scanner.enabled {
        let scanner_pool = pool.clone();
        let scanner_config = config.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;

            let quick_interval =
                Duration::from_secs(scanner_config.scanner.quick_scan_interval_minutes * 60);
            let full_interval =
                Duration::from_secs(scanner_config.scanner.full_scan_interval_hours * 3600);

            let mut last_quick_scan = std::time::Instant::now();
            let mut last_full_scan = std::time::Instant::now();

            if scanner_config.scanner.scan_on_startup {
                tracing::info!("Running startup quick scan...");
                match scanner::quick_scan_all_libraries(
                    &scanner_pool,
                    scanner_config.paths.cache_dir.clone(),
                )
                .await
                {
                    Ok(result) => {
                        if result.files_added > 0 || result.files_removed > 0 {
                            tracing::info!(
                                "Startup quick scan complete: {} added, {} removed",
                                result.files_added,
                                result.files_removed
                            );
                        } else {
                            tracing::debug!("Startup quick scan: no changes");
                        }
                    }
                    Err(e) => tracing::error!("Startup quick scan failed: {}", e),
                }
            }

            if scanner_config.scanner.quick_scan_interval_minutes > 0 {
                tracing::info!(
                    "Periodic quick scan enabled: every {} minutes",
                    scanner_config.scanner.quick_scan_interval_minutes
                );
            }
            if scanner_config.scanner.full_scan_interval_hours > 0 {
                tracing::info!(
                    "Periodic full scan enabled: every {} hours",
                    scanner_config.scanner.full_scan_interval_hours
                );
            }

            // Main scanner loop - check every minute
            let check_interval = Duration::from_secs(60);
            loop {
                tokio::time::sleep(check_interval).await;

                let now = std::time::Instant::now();

                if scanner_config.scanner.quick_scan_interval_minutes > 0
                    && now.duration_since(last_quick_scan) >= quick_interval
                {
                    tracing::debug!("Running periodic quick scan...");
                    match scanner::quick_scan_all_libraries(
                        &scanner_pool,
                        scanner_config.paths.cache_dir.clone(),
                    )
                    .await
                    {
                        Ok(result) => {
                            if result.files_added > 0 || result.files_removed > 0 {
                                tracing::info!(
                                    "Periodic quick scan: {} added, {} removed",
                                    result.files_added,
                                    result.files_removed
                                );
                            }
                        }
                        Err(e) => tracing::error!("Periodic quick scan failed: {}", e),
                    }
                    last_quick_scan = now;
                }

                if scanner_config.scanner.full_scan_interval_hours > 0
                    && now.duration_since(last_full_scan) >= full_interval
                {
                    tracing::info!("Running periodic full library refresh...");
                    if let Err(e) = scanner::refresh_all_libraries(&scanner_pool).await {
                        tracing::error!("Periodic full scan failed: {}", e);
                    } else {
                        tracing::info!("Periodic full scan complete");
                    }
                    last_full_scan = now;
                    // Reset quick scan timer since full scan covers everything
                    last_quick_scan = now;
                }
            }
        });
    }

    // Spawn background image downloader task
    {
        let image_pool = pool.clone();
        let image_config = config.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(10)).await;

            let image_cache_dir = image_config.paths.cache_dir.join("images");
            let metadata_service = services::metadata::MetadataService::from_env(
                image_cache_dir.clone(),
                Some(image_config.anime_db_enabled),
            );

            tracing::info!("Background image downloader started");

            loop {
                // Get pending images in batches
                match db::get_pending_images(&image_pool, image_batch_size).await {
                    Ok(pending) if !pending.is_empty() => {
                        let count = pending.len();
                        tracing::debug!("Processing {} queued images", count);

                        for image in pending {
                            match metadata_service
                                .download_image_to_cache(
                                    &image.url,
                                    &image.item_id,
                                    &image.image_type,
                                )
                                .await
                            {
                                Ok(path) => {
                                    let image_id = uuid::Uuid::new_v4().to_string();
                                    if let Err(e) = sqlx::query(
                                        "INSERT OR REPLACE INTO images (id, item_id, image_type, path) VALUES (?, ?, ?, ?)",
                                    )
                                    .bind(&image_id)
                                    .bind(&image.item_id)
                                    .bind(&image.image_type)
                                    .bind(path.to_str().unwrap_or_default())
                                    .execute(&image_pool)
                                    .await
                                    {
                                        tracing::warn!("Failed to save image path: {}", e);
                                    }

                                    let _ = db::mark_image_downloaded(&image_pool, image.id).await;
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "Failed to download image for {}: {}",
                                        image.item_id,
                                        e
                                    );
                                    let _ = db::mark_image_failed(&image_pool, image.id).await;
                                }
                            }

                            // Small delay between downloads to avoid rate limiting
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    Ok(_) => {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to get pending images: {}", e);
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }

                tokio::time::sleep(Duration::from_millis(200)).await;
            }
        });
    }

    // Spawn background thumbnail generator task
    {
        let thumb_pool = pool.clone();
        let thumb_config = config.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;

            let image_cache_dir = thumb_config.paths.cache_dir.join("images");

            tracing::info!("Background thumbnail generator started");

            loop {
                // Get pending thumbnails in batches
                match db::get_pending_thumbnails(&thumb_pool, thumbnail_batch_size).await {
                    Ok(pending) if !pending.is_empty() => {
                        let count = pending.len();
                        tracing::debug!("Processing {} queued thumbnails", count);

                        for thumb in pending {
                            let video_path = std::path::Path::new(&thumb.video_path);

                            let timestamp =
                                match services::mediainfo::extract_media_info_async(video_path)
                                    .await
                                {
                                    Ok(info) => info
                                        .duration_seconds
                                        .map(services::mediainfo::calculate_thumbnail_timestamp)
                                        .unwrap_or(30.0),
                                    Err(_) => 30.0,
                                };

                            let item_dir = image_cache_dir.join(&thumb.item_id);
                            let output_path = item_dir.join("Primary.jpg");

                            match services::mediainfo::extract_thumbnail_async(
                                video_path,
                                &output_path,
                                timestamp,
                                Some(480), // 480px wide thumbnails
                            )
                            .await
                            {
                                Ok(()) => {
                                    let image_id = uuid::Uuid::new_v4().to_string();
                                    if let Err(e) = sqlx::query(
                                        "INSERT OR REPLACE INTO images (id, item_id, image_type, path) VALUES (?, ?, ?, ?)",
                                    )
                                    .bind(&image_id)
                                    .bind(&thumb.item_id)
                                    .bind("Primary")
                                    .bind(output_path.to_str().unwrap_or_default())
                                    .execute(&thumb_pool)
                                    .await
                                    {
                                        tracing::warn!("Failed to save thumbnail path: {}", e);
                                    }

                                    let _ =
                                        db::mark_thumbnail_complete(&thumb_pool, thumb.id).await;
                                    tracing::debug!("Generated thumbnail for {}", thumb.item_id);
                                }
                                Err(e) => {
                                    tracing::debug!(
                                        "Failed to generate thumbnail for {}: {}",
                                        thumb.item_id,
                                        e
                                    );
                                    let _ = db::mark_thumbnail_failed(&thumb_pool, thumb.id).await;
                                }
                            }

                            // Pause between thumbnails (ffmpeg is CPU intensive)
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    }
                    Ok(_) => {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                    Err(e) => {
                        tracing::warn!("Failed to get pending thumbnails: {}", e);
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                }

                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        });
    }

    // Spawn periodic session cleanup task
    {
        let session_pool = pool.clone();
        tokio::spawn(async move {
            // Wait for server to start
            tokio::time::sleep(Duration::from_secs(30)).await;

            tracing::info!("Session cleanup task started (runs every 5 minutes)");

            loop {
                // Clean up sessions older than 1 hour with no activity
                match api::sessions::cleanup_stale_sessions(&session_pool, 3600).await {
                    Ok(removed) if removed > 0 => {
                        tracing::info!("Cleaned up {} stale sessions", removed);
                    }
                    Ok(_) => {
                        // No stale sessions, nothing to log
                    }
                    Err(e) => {
                        tracing::warn!("Session cleanup failed: {}", e);
                    }
                }

                // Run every 5 minutes
                tokio::time::sleep(Duration::from_secs(300)).await;
            }
        });
    }

    // Root handler for GET / and HEAD / - confirms server is running
    async fn root_handler() -> &'static str {
        "Jellyfin Rust Server"
    }

    // Build router
    let app = Router::new()
        .route("/", get(root_handler).head(root_handler))
        .route("/health", get(|| async { "OK" }))
        .nest("/", api::routes())
        .layer(CorsLayer::permissive())
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("Starting server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
