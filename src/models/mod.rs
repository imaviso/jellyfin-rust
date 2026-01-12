use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing)]
    pub password_hash: String,
    pub is_admin: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Session {
    pub token: String,
    pub user_id: String,
    pub device_id: String,
    pub device_name: String,
    pub client: String,
    pub created_at: String,
    pub last_activity: String,
    pub expires_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Library {
    pub id: String,
    pub name: String,
    pub path: String,
    pub library_type: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LibraryType {
    Movies,
    TvShows,
    Music,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct MediaItem {
    pub id: String,
    pub library_id: String,
    pub parent_id: Option<String>,
    pub item_type: String,
    pub name: String,
    pub path: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub runtime_ticks: Option<i64>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub anilist_id: Option<String>,
    pub mal_id: Option<String>,
    pub anidb_id: Option<String>,
    pub kitsu_id: Option<String>,
    pub sort_name: Option<String>,
    pub index_number: Option<i32>,
    pub parent_index_number: Option<i32>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ItemType {
    Movie,
    Series,
    Season,
    Episode,
    MusicAlbum,
    Audio,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Image {
    pub id: String,
    pub item_id: String,
    pub image_type: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImageType {
    Primary,
    Backdrop,
    Banner,
    Thumb,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct PlaybackProgress {
    pub user_id: String,
    pub item_id: String,
    pub position_ticks: i64,
    pub played: bool,
    pub play_count: i32,
    pub last_played: Option<String>,
}
