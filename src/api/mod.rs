use axum::Router;
use std::sync::Arc;

use crate::AppState;

mod branding;
mod collections;
mod display_preferences;
mod favorites;
pub mod filters;
mod home;
mod images;
mod items;
mod library;
mod localization;
mod movies;
mod persons;
mod playback;
mod playbackinfo;
mod playlists;
pub mod segments;
pub mod sessions;
mod shows;
mod stubs;
mod subtitles;
mod system;
mod tasks;
mod users;
mod videos;
mod views;

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .nest("/System", system::routes())
        .nest("/Branding", branding::routes())
        .nest("/Users", users::routes())
        .nest("/Library/VirtualFolders", library::routes())
        .nest("/Items", items::routes())
        .nest("/Items", images::routes()) // Image routes under /Items/:id/Images
        .nest("/Items", playbackinfo::routes()) // PlaybackInfo under /Items/:id/PlaybackInfo
        .nest("/Items", subtitles::search_routes()) // Subtitle search under /Items/:id/RemoteSearch/Subtitles
        .nest("/Search", items::search_routes()) // Search hints
        .nest("/Videos", videos::routes())
        .nest("/Videos", subtitles::routes()) // Subtitle routes under /Videos/:id/:id/Subtitles
        .nest("/Sessions", sessions::routes()) // Active session management
        .nest("/Sessions", playback::routes()) // Playback reporting (Playing, Progress, Stopped)
        .nest("/Shows", shows::routes()) // Shows endpoints (Seasons, Episodes)
        .nest("/Shows/NextUp", home::next_up_routes()) // NextUp endpoint
        .nest("/Movies", movies::routes()) // Movie recommendations
        .nest("/UserViews", views::routes()) // User library views
        .nest("/UserItems/Resume", home::resume_routes()) // Resume watching
        .nest("/QuickConnect", stubs::quick_connect_routes()) // QuickConnect stub
        .nest("/DisplayPreferences", display_preferences::routes()) // Display prefs
        .nest("/ScheduledTasks", tasks::routes()) // Scheduled tasks
        .nest("/Collections", collections::routes()) // Collections API
        .nest("/Playlists", playlists::routes()) // Playlists API
        .nest("/Persons", persons::routes()) // Cast/actors API
        .nest("/Localization", localization::routes()) // Cultures/languages API
        .nest("/MediaSegments", segments::routes()) // Media segments (intro/outro skip)
        // Jellyfin clients also query /Users/{userId}/Items
        .route(
            "/Users/:userId/Items",
            axum::routing::get(items::get_user_items),
        )
        .route(
            "/Users/:userId/Items/:itemId",
            axum::routing::get(items::get_user_item),
        )
        // User latest items for home screen
        .nest("/Users/:userId/Items/Latest", home::user_latest_routes())
        // User images
        .nest("/Users/:userId/Images", users::user_image_routes())
        // User played items (mark as played/unplayed)
        .nest("/Users/:userId/PlayedItems", playback::user_played_routes())
        // User favorites
        .nest("/UserFavoriteItems", favorites::routes())
        // Genres and Studios endpoints
        .nest("/Genres", filters::routes())
        .nest("/Studios", filters::studio_routes())
}
