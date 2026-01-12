use anyhow::Result;
use axum::{routing::get, Router};
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::net::SocketAddr;
use std::str::FromStr;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
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

/// Tracks all background task handles for graceful shutdown
struct BackgroundTasks {
    handles: Vec<(&'static str, JoinHandle<()>)>,
    shutdown: CancellationToken,
}

impl BackgroundTasks {
    fn new() -> Self {
        Self {
            handles: Vec::new(),
            shutdown: CancellationToken::new(),
        }
    }

    fn token(&self) -> CancellationToken {
        self.shutdown.clone()
    }

    fn spawn<F>(&mut self, name: &'static str, future: F)
    where
        F: std::future::Future<Output = ()> + Send + 'static,
    {
        let handle = tokio::spawn(future);
        self.handles.push((name, handle));
    }

    async fn shutdown(self) {
        tracing::info!("Initiating graceful shutdown...");

        // Signal all tasks to stop
        self.shutdown.cancel();

        // Wait for all tasks with a timeout
        for (name, handle) in self.handles {
            tracing::debug!("Waiting for {} to finish...", name);
            match tokio::time::timeout(Duration::from_secs(10), handle).await {
                Ok(Ok(())) => tracing::debug!("{} finished cleanly", name),
                Ok(Err(e)) => tracing::warn!("{} panicked: {}", name, e),
                Err(_) => tracing::warn!("{} timed out during shutdown", name),
            }
        }

        tracing::info!("All background tasks stopped");
    }
}

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
        // Configure PRAGMAs on EVERY new connection via after_connect hook
        .after_connect(|conn, _meta| {
            Box::pin(async move {
                use sqlx::Executor;
                // Cache size: -32000 = 32MB (negative = KB)
                conn.execute("PRAGMA cache_size = -32000").await?;
                // Memory-mapped I/O: 64MB
                conn.execute("PRAGMA mmap_size = 67108864").await?;
                // Store temp tables in memory
                conn.execute("PRAGMA temp_store = MEMORY").await?;
                Ok(())
            })
        })
        .connect_with(connect_options)
        .await?;

    tracing::info!("SQLite configured: WAL mode, 32MB cache, 64MB mmap (per connection)");

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

    // Configure scanner video extensions from config
    if !config.scanner.video_extensions.is_empty() {
        scanner::set_video_extensions(config.scanner.video_extensions.clone());
        tracing::debug!(
            "Scanner configured with {} video extensions",
            config.scanner.video_extensions.len()
        );
    }

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

    // Initialize background task manager with graceful shutdown support
    let mut bg_tasks = BackgroundTasks::new();
    let shutdown_token = bg_tasks.token();

    // Spawn background task for library auto-creation and scanning
    // (This is a one-time task, doesn't need cancellation)
    if !config.libraries.is_empty() {
        let bg_pool = pool.clone();
        let bg_config = config.clone();
        bg_tasks.spawn("library-init", async move {
            // Small delay to let the server fully start
            tokio::time::sleep(Duration::from_millis(500)).await;

            tracing::info!(
                "Background: Checking {} configured libraries...",
                bg_config.libraries.len()
            );

            for lib in &bg_config.libraries {
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
                            "Skipping library '{}': invalid type '{}'",
                            lib.name,
                            lib.library_type
                        );
                        continue;
                    }

                    // Use async check to avoid blocking
                    if !tokio::fs::try_exists(&lib.path).await.unwrap_or(false) {
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
                        Some(bg_config.fetch_episode_metadata),
                    )
                    .await
                    {
                        Ok(result) => {
                            tracing::info!(
                                "Library '{}' scan complete: {} series, {} episodes, {} movies",
                                lib.name,
                                result.series_added,
                                result.episodes_added,
                                result.movies_added
                            );
                        }
                        Err(e) => {
                            tracing::error!("Failed to scan library '{}': {}", lib.name, e);
                        }
                    }
                }
            }

            tracing::info!("Background: Library initialization complete");
            tracing::info!("Background: Rebuilding full-text search index...");
            if let Err(e) = db::rebuild_fts_index(&bg_pool).await {
                tracing::error!("Failed to rebuild FTS index: {}", e);
            }
        });
    } else {
        let fts_pool = pool.clone();
        bg_tasks.spawn("fts-rebuild", async move {
            tokio::time::sleep(Duration::from_millis(500)).await;
            tracing::info!("Background: Rebuilding full-text search index...");
            if let Err(e) = db::rebuild_fts_index(&fts_pool).await {
                tracing::error!("Failed to rebuild FTS index: {}", e);
            }
        });
    }

    // Spawn periodic scanner task with cancellation support
    if config.scanner.enabled {
        let scanner_pool = pool.clone();
        let scanner_config = config.clone();
        let cancel = shutdown_token.clone();
        bg_tasks.spawn("periodic-scanner", async move {
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
                    Ok(result) if result.files_added > 0 || result.files_removed > 0 => {
                        tracing::info!(
                            "Startup quick scan: {} added, {} removed",
                            result.files_added,
                            result.files_removed
                        );
                    }
                    Err(e) => tracing::error!("Startup quick scan failed: {}", e),
                    _ => {}
                }
            }

            let check_interval = Duration::from_secs(60);
            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!("Scanner task received shutdown signal");
                        break;
                    }
                    _ = tokio::time::sleep(check_interval) => {
                        let now = std::time::Instant::now();

                        if scanner_config.scanner.quick_scan_interval_minutes > 0
                            && now.duration_since(last_quick_scan) >= quick_interval
                        {
                            if let Ok(result) = scanner::quick_scan_all_libraries(
                                &scanner_pool,
                                scanner_config.paths.cache_dir.clone(),
                            ).await {
                                if result.files_added > 0 || result.files_removed > 0 {
                                    tracing::info!(
                                        "Quick scan: {} added, {} removed",
                                        result.files_added, result.files_removed
                                    );
                                }
                            }
                            last_quick_scan = now;
                        }

                        if scanner_config.scanner.full_scan_interval_hours > 0
                            && now.duration_since(last_full_scan) >= full_interval
                        {
                            if let Err(e) = scanner::refresh_all_libraries(&scanner_pool).await {
                                tracing::error!("Full scan failed: {}", e);
                            }
                            last_full_scan = now;
                            last_quick_scan = now;
                        }
                    }
                }
            }
        });
    }

    // Spawn background image downloader task with cancellation
    {
        let image_pool = pool.clone();
        let image_config = config.clone();
        let cancel = shutdown_token.clone();
        bg_tasks.spawn("image-downloader", async move {
            tokio::time::sleep(Duration::from_secs(10)).await;

            let image_cache_dir = image_config.paths.cache_dir.join("images");
            // Disable anime_db for image downloader - it only needs to download from URLs,
            // not search for metadata. This saves ~60MB of RAM.
            let metadata_service = services::metadata::MetadataService::from_env(
                image_cache_dir.clone(),
                Some(false), // Don't load anime-offline-database for image downloads
            );

            tracing::info!("Background image downloader started");

            loop {
                if cancel.is_cancelled() {
                    tracing::debug!("Image downloader received shutdown signal");
                    break;
                }

                match db::get_pending_images(&image_pool, image_batch_size).await {
                    Ok(pending) if !pending.is_empty() => {
                        for image in pending {
                            if cancel.is_cancelled() { break; }

                            if let Ok(path) = metadata_service
                                .download_image_to_cache(&image.url, &image.item_id, &image.image_type)
                                .await
                            {
                                let image_id = uuid::Uuid::new_v4().to_string();
                                let _ = sqlx::query(
                                    "INSERT OR REPLACE INTO images (id, item_id, image_type, path) VALUES (?, ?, ?, ?)",
                                )
                                .bind(&image_id)
                                .bind(&image.item_id)
                                .bind(&image.image_type)
                                .bind(path.to_str().unwrap_or_default())
                                .execute(&image_pool)
                                .await;
                                let _ = db::mark_image_downloaded(&image_pool, image.id).await;
                            } else {
                                let _ = db::mark_image_failed(&image_pool, image.id).await;
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                    _ => {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        });
    }

    // Spawn background thumbnail generator with cancellation
    {
        let thumb_pool = pool.clone();
        let thumb_config = config.clone();
        let cancel = shutdown_token.clone();
        bg_tasks.spawn("thumbnail-generator", async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let image_cache_dir = thumb_config.paths.cache_dir.join("images");
            tracing::info!("Background thumbnail generator started");

            loop {
                if cancel.is_cancelled() {
                    tracing::debug!("Thumbnail generator received shutdown signal");
                    break;
                }

                match db::get_pending_thumbnails(&thumb_pool, thumbnail_batch_size).await {
                    Ok(pending) if !pending.is_empty() => {
                        for thumb in pending {
                            if cancel.is_cancelled() { break; }

                            let video_path = std::path::Path::new(&thumb.video_path);
                            let timestamp = services::mediainfo::extract_media_info_async(video_path)
                                .await
                                .ok()
                                .and_then(|i| i.duration_seconds)
                                .map(services::mediainfo::calculate_thumbnail_timestamp)
                                .unwrap_or(30.0);

                            let item_dir = image_cache_dir.join(&thumb.item_id);
                            let output_path = item_dir.join("Primary.jpg");

                            if let Ok(()) = services::mediainfo::extract_thumbnail_async(
                                video_path, &output_path, timestamp, Some(480),
                            ).await {
                                let image_id = uuid::Uuid::new_v4().to_string();
                                let _ = sqlx::query(
                                    "INSERT OR REPLACE INTO images (id, item_id, image_type, path) VALUES (?, ?, ?, ?)",
                                )
                                .bind(&image_id)
                                .bind(&thumb.item_id)
                                .bind("Primary")
                                .bind(output_path.to_str().unwrap_or_default())
                                .execute(&thumb_pool)
                                .await;
                                let _ = db::mark_thumbnail_complete(&thumb_pool, thumb.id).await;
                            } else {
                                let _ = db::mark_thumbnail_failed(&thumb_pool, thumb.id).await;
                            }
                            tokio::time::sleep(Duration::from_millis(200)).await;
                        }
                    }
                    _ => {
                        tokio::time::sleep(Duration::from_secs(10)).await;
                    }
                }
            }
        });
    }

    // Spawn session cleanup task with cancellation
    {
        let session_pool = pool.clone();
        let cancel = shutdown_token.clone();
        bg_tasks.spawn("session-cleanup", async move {
            tokio::time::sleep(Duration::from_secs(30)).await;
            tracing::info!("Session cleanup task started");

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!("Session cleanup received shutdown signal");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(300)) => {
                        if let Ok(removed) = services::auth::cleanup_expired_sessions(&session_pool).await {
                            if removed > 0 {
                                tracing::info!("Cleaned up {} expired sessions", removed);
                            }
                        }
                        if let Ok(removed) = api::sessions::cleanup_stale_sessions(&session_pool, 3600).await {
                            if removed > 0 {
                                tracing::info!("Cleaned up {} stale active sessions", removed);
                            }
                        }
                    }
                }
            }
        });
    }

    // Spawn missing thumbnail checker task (configurable interval)
    if config.scanner.missing_thumbnail_check_minutes > 0 {
        let thumb_check_pool = pool.clone();
        let thumb_check_config = config.clone();
        let cancel = shutdown_token.clone();
        let interval_secs = config.scanner.missing_thumbnail_check_minutes * 60;

        bg_tasks.spawn("missing-thumbnail-checker", async move {
            // Wait 2 minutes before first check (let initial scan complete)
            tokio::time::sleep(Duration::from_secs(120)).await;
            tracing::info!(
                "Missing thumbnail checker started (interval: {} minutes)",
                thumb_check_config.scanner.missing_thumbnail_check_minutes
            );

            loop {
                tokio::select! {
                    _ = cancel.cancelled() => {
                        tracing::debug!("Missing thumbnail checker received shutdown signal");
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(interval_secs)) => {
                        // Queue any items missing thumbnails
                        match db::queue_missing_thumbnails(&thumb_check_pool).await {
                            Ok(count) if count > 0 => {
                                tracing::info!("Queued {} missing thumbnails for generation", count);
                            }
                            Ok(_) => {
                                tracing::debug!("No missing thumbnails found");
                            }
                            Err(e) => {
                                tracing::warn!("Failed to check for missing thumbnails: {}", e);
                            }
                        }

                        // Also reset any failed thumbnails for retry (if enabled)
                        if thumb_check_config.scanner.retry_failed_thumbnails {
                            match db::reset_failed_thumbnails(&thumb_check_pool).await {
                                Ok(count) if count > 0 => {
                                    tracing::info!("Reset {} failed thumbnails for retry", count);
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::warn!("Failed to reset failed thumbnails: {}", e);
                                }
                            }
                        }
                    }
                }
            }
        });
    } else {
        tracing::info!("Missing thumbnail checker disabled (interval set to 0)");
    }

    // Root handler
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

    // Create shutdown signal listener
    let shutdown_signal = async {
        let ctrl_c = async {
            tokio::signal::ctrl_c()
                .await
                .expect("Failed to install Ctrl+C handler");
        };

        #[cfg(unix)]
        let terminate = async {
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("Failed to install SIGTERM handler")
                .recv()
                .await;
        };

        #[cfg(not(unix))]
        let terminate = std::future::pending::<()>();

        tokio::select! {
            _ = ctrl_c => tracing::info!("Received Ctrl+C, shutting down..."),
            _ = terminate => tracing::info!("Received SIGTERM, shutting down..."),
        }
    };

    // Start server with graceful shutdown
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal)
        .await?;

    // After server stops, gracefully shutdown background tasks
    bg_tasks.shutdown().await;

    tracing::info!("Server shutdown complete");
    Ok(())
}
