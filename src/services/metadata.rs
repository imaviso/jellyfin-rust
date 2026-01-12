use anyhow::Result;
use std::path::PathBuf;

use super::anidb::{AniDBClient, AniDBMetadata};
use super::anilist::{AniListClient, AnimeMetadata, CastMember};
use super::anime_db::AnimeOfflineDatabase;
use super::jikan::{JikanClient, JikanMetadata};
use super::tmdb::{MediaMetadata, TmdbCastMember, TmdbClient};

#[derive(Debug, Clone, Default)]
pub struct UnifiedMetadata {
    pub anilist_id: Option<String>,
    pub anidb_id: Option<String>,
    pub mal_id: Option<String>,
    pub kitsu_id: Option<String>,
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub name: Option<String>,
    pub name_original: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub episode_count: Option<i32>,
    pub runtime_minutes: Option<i32>,
    pub genres: Option<Vec<String>>,
    pub studio: Option<String>,
    pub cast: Vec<CastMember>,
    pub provider: MetadataProvider,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum MetadataProvider {
    #[default]
    None,
    AniList,
    AniDB,
    Jikan,
    Tmdb,
}

impl std::fmt::Display for MetadataProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetadataProvider::None => write!(f, "None"),
            MetadataProvider::AniList => write!(f, "AniList"),
            MetadataProvider::AniDB => write!(f, "AniDB"),
            MetadataProvider::Jikan => write!(f, "Jikan/MAL"),
            MetadataProvider::Tmdb => write!(f, "TMDB"),
        }
    }
}

/// Episode-level metadata
#[derive(Debug, Clone, Default)]
pub struct EpisodeMetadata {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub runtime_minutes: Option<i32>,
    pub still_url: Option<String>,
}

pub struct MetadataService {
    anilist: AniListClient,
    anidb: AniDBClient,
    jikan: JikanClient,
    anime_db: AnimeOfflineDatabase,
    tmdb: Option<TmdbClient>,
    image_cache_dir: PathBuf,
}

impl MetadataService {
    pub fn new(image_cache_dir: PathBuf, anime_db_enabled: Option<bool>) -> Self {
        let tmdb = TmdbClient::from_env(image_cache_dir.clone());
        let cache_dir = image_cache_dir
            .parent()
            .unwrap_or(&image_cache_dir)
            .to_path_buf();

        Self {
            anilist: AniListClient::new(image_cache_dir.clone()),
            anidb: AniDBClient::new(image_cache_dir.clone()),
            jikan: JikanClient::new(),
            anime_db: AnimeOfflineDatabase::new(cache_dir, anime_db_enabled),
            tmdb,
            image_cache_dir,
        }
    }

    /// Create from environment, returns None if no providers are available
    /// Note: AniList is always available (no API key needed)
    /// anime_db_enabled: pass Some(true/false) to override, or None to use env var
    pub fn from_env(image_cache_dir: PathBuf, anime_db_enabled: Option<bool>) -> Self {
        Self::new(image_cache_dir, anime_db_enabled)
    }

    pub fn is_available(&self) -> bool {
        true
    }

    pub fn has_tmdb(&self) -> bool {
        self.tmdb.is_some()
    }

    pub fn has_anime_db(&self) -> bool {
        self.anime_db.is_enabled()
    }

    /// Preload the anime offline database (downloads if needed)
    /// Call this before scanning to ensure the database is ready
    pub async fn preload_anime_db(&self) -> Result<()> {
        self.anime_db.preload().await
    }

    /// Unload the anime offline database from memory to free RAM
    /// Call this after scanning is complete - will be reloaded on next search if needed
    pub async fn unload_anime_db(&self) {
        self.anime_db.unload().await
    }

    /// Get metadata for an anime series
    /// Priority: anime-offline-database -> AniList -> AniDB -> TMDB
    pub async fn get_anime_metadata(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Option<UnifiedMetadata>> {
        tracing::debug!("Searching for anime metadata: {} ({:?})", name, year);

        const MIN_CONFIDENCE_SCORE: f64 = 60.0;
        const MAX_YEAR_DIFF: i32 = 5;

        if self.anime_db.is_enabled() {
            match self.anime_db.search(name, year).await {
                Ok(results) if !results.is_empty() => {
                    let best_match = &results[0];

                    if best_match.score < MIN_CONFIDENCE_SCORE {
                        tracing::debug!(
                            "anime-offline-database match score {} too low for '{}', trying other providers",
                            best_match.score, name
                        );
                    } else {
                        let year_ok =
                            match (year, best_match.entry.year()) {
                                (Some(query_year), Some(entry_year)) => {
                                    let diff = (query_year - entry_year).abs();
                                    if diff > MAX_YEAR_DIFF {
                                        tracing::warn!(
                                        "Year mismatch for '{}': query={}, match={} ({}), skipping",
                                        name, query_year, entry_year, best_match.entry.title
                                    );
                                        false
                                    } else {
                                        true
                                    }
                                }
                                _ => true,
                            };

                        if year_ok {
                            let provider_ids = best_match.entry.provider_ids();

                            tracing::debug!(
                                "Found in anime-offline-database: {} (score={:.1}, AniList={:?}, MAL={:?}, AniDB={:?})",
                                best_match.entry.title,
                                best_match.score,
                                provider_ids.anilist_id,
                                provider_ids.mal_id,
                                provider_ids.anidb_id
                            );

                            if let Some(anilist_id) = provider_ids.anilist_id {
                                if let Ok(Some(meta)) =
                                    self.anilist.get_anime_by_id(anilist_id).await
                                {
                                    tracing::info!(
                                        "Found anime on AniList (via local DB): {} -> {}",
                                        name,
                                        meta.name.as_deref().unwrap_or("Unknown")
                                    );
                                    let mut unified = self.anilist_to_unified(meta);
                                    if unified.anidb_id.is_none() {
                                        unified.anidb_id =
                                            provider_ids.anidb_id.map(|id| id.to_string());
                                    }
                                    if unified.kitsu_id.is_none() {
                                        unified.kitsu_id =
                                            provider_ids.kitsu_id.map(|id| id.to_string());
                                    }
                                    if unified.mal_id.is_none() {
                                        unified.mal_id =
                                            provider_ids.mal_id.map(|id| id.to_string());
                                    }
                                    return Ok(Some(unified));
                                }
                            }

                            if let Some(anidb_id) = provider_ids.anidb_id {
                                if let Ok(Some(meta)) = self.anidb.get_anime_by_id(anidb_id).await {
                                    tracing::info!(
                                        "Found anime on AniDB (via local DB): {} -> {}",
                                        name,
                                        meta.name.as_deref().unwrap_or("Unknown")
                                    );
                                    let mut unified = self.anidb_to_unified(meta);
                                    unified.anilist_id =
                                        provider_ids.anilist_id.map(|id| id.to_string());
                                    unified.mal_id = provider_ids.mal_id.map(|id| id.to_string());
                                    return Ok(Some(unified));
                                }
                            }

                            // Try Jikan (MAL) if we have a MAL ID
                            if let Some(mal_id) = provider_ids.mal_id {
                                if let Ok(Some(meta)) = self.jikan.get_anime_by_id(mal_id).await {
                                    tracing::info!(
                                        "Found anime on Jikan/MAL (via local DB): {} -> {}",
                                        name,
                                        meta.name.as_deref().unwrap_or("Unknown")
                                    );
                                    let mut unified = self.jikan_to_unified(meta);
                                    unified.anilist_id =
                                        provider_ids.anilist_id.map(|id| id.to_string());
                                    unified.anidb_id =
                                        provider_ids.anidb_id.map(|id| id.to_string());
                                    unified.kitsu_id =
                                        provider_ids.kitsu_id.map(|id| id.to_string());
                                    return Ok(Some(unified));
                                }
                            }
                        }
                    }
                }
                Ok(_) => {
                    tracing::debug!("No anime-offline-database match for: {}", name);
                }
                Err(e) => {
                    tracing::debug!("Anime offline database search failed for {}: {}", name, e);
                }
            }
        }

        match self.anilist.get_anime_metadata(name, year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found anime on AniList: {} -> {}",
                    name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );

                let mut unified = self.anilist_to_unified(meta.clone());
                if self.anime_db.is_enabled() {
                    if let Some(ref anilist_id_str) = meta.anilist_id {
                        if let Ok(anilist_id) = anilist_id_str.parse::<i64>() {
                            if let Ok(Some(entry)) =
                                self.anime_db.find_by_anilist_id(anilist_id).await
                            {
                                let provider_ids = entry.provider_ids();
                                if unified.anidb_id.is_none() {
                                    unified.anidb_id =
                                        provider_ids.anidb_id.map(|id| id.to_string());
                                }
                                if unified.kitsu_id.is_none() {
                                    unified.kitsu_id =
                                        provider_ids.kitsu_id.map(|id| id.to_string());
                                }
                            }
                        }
                    }
                }

                return Ok(Some(unified));
            }
            Ok(None) => {
                tracing::debug!("No AniList match for: {}", name);
            }
            Err(e) => {
                tracing::warn!("AniList search failed for {}: {}", name, e);
            }
        }

        // Try Jikan (MAL) as fallback
        match self.jikan.search_anime_best_match(name, year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found anime on Jikan/MAL: {} -> {}",
                    name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );
                return Ok(Some(self.jikan_to_unified(meta)));
            }
            Ok(None) => {
                tracing::debug!("No Jikan/MAL match for: {}", name);
            }
            Err(e) => {
                tracing::warn!("Jikan search failed for {}: {}", name, e);
            }
        }

        if let Some(ref tmdb) = self.tmdb {
            match tmdb.get_series_metadata(name, year).await {
                Ok(Some(meta)) => {
                    tracing::info!(
                        "Found anime on TMDB: {} -> {}",
                        name,
                        meta.name.as_deref().unwrap_or("Unknown")
                    );
                    return Ok(Some(self.tmdb_series_to_unified(meta)));
                }
                Ok(None) => {
                    tracing::debug!("No TMDB match for: {}", name);
                }
                Err(e) => {
                    tracing::warn!("TMDB search failed for {}: {}", name, e);
                }
            }
        }

        Ok(None)
    }

    /// Get metadata for a TV series (non-anime)
    /// When anime_db is enabled: anime-offline-database -> AniList -> TMDB
    /// When anime_db is disabled: TMDB -> AniList
    pub async fn get_series_metadata(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Option<UnifiedMetadata>> {
        tracing::debug!("Searching for series metadata: {} ({:?})", name, year);

        const MIN_CONFIDENCE_SCORE: f64 = 60.0;
        const MAX_YEAR_DIFF: i32 = 5;

        if self.anime_db.is_enabled() {
            match self.anime_db.search(name, year).await {
                Ok(results) if !results.is_empty() => {
                    let best_match = &results[0];

                    if best_match.score < MIN_CONFIDENCE_SCORE {
                        tracing::debug!(
                            "anime-offline-database match score {} too low for '{}', trying other providers",
                            best_match.score, name
                        );
                    } else {
                        let year_ok =
                            match (year, best_match.entry.year()) {
                                (Some(query_year), Some(entry_year)) => {
                                    let diff = (query_year - entry_year).abs();
                                    if diff > MAX_YEAR_DIFF {
                                        tracing::warn!(
                                        "Year mismatch for '{}': query={}, match={} ({}), skipping",
                                        name, query_year, entry_year, best_match.entry.title
                                    );
                                        false
                                    } else {
                                        true
                                    }
                                }
                                _ => true,
                            };

                        if year_ok {
                            let provider_ids = best_match.entry.provider_ids();

                            tracing::debug!(
                                "Found in anime-offline-database: {} (score={:.1}, AniList={:?}, MAL={:?}, AniDB={:?})",
                                best_match.entry.title,
                                best_match.score,
                                provider_ids.anilist_id,
                                provider_ids.mal_id,
                                provider_ids.anidb_id
                            );

                            if let Some(anilist_id) = provider_ids.anilist_id {
                                if let Ok(Some(meta)) =
                                    self.anilist.get_anime_by_id(anilist_id).await
                                {
                                    tracing::info!(
                                        "Found series on AniList (via anime-offline-database): {} -> {}",
                                        name,
                                        meta.name.as_deref().unwrap_or("Unknown")
                                    );
                                    let mut unified = self.anilist_to_unified(meta);
                                    if unified.anidb_id.is_none() {
                                        unified.anidb_id =
                                            provider_ids.anidb_id.map(|id| id.to_string());
                                    }
                                    if unified.kitsu_id.is_none() {
                                        unified.kitsu_id =
                                            provider_ids.kitsu_id.map(|id| id.to_string());
                                    }
                                    if unified.mal_id.is_none() {
                                        unified.mal_id =
                                            provider_ids.mal_id.map(|id| id.to_string());
                                    }
                                    return Ok(Some(unified));
                                }
                            }

                            if let Some(anidb_id) = provider_ids.anidb_id {
                                if let Ok(Some(meta)) = self.anidb.get_anime_by_id(anidb_id).await {
                                    tracing::info!(
                                        "Found series on AniDB (via anime-offline-database): {} -> {}",
                                        name,
                                        meta.name.as_deref().unwrap_or("Unknown")
                                    );
                                    let mut unified = self.anidb_to_unified(meta);
                                    unified.anilist_id =
                                        provider_ids.anilist_id.map(|id| id.to_string());
                                    unified.mal_id = provider_ids.mal_id.map(|id| id.to_string());
                                    return Ok(Some(unified));
                                }
                            }

                            // Try Jikan (MAL) if we have a MAL ID
                            if let Some(mal_id) = provider_ids.mal_id {
                                if let Ok(Some(meta)) = self.jikan.get_anime_by_id(mal_id).await {
                                    tracing::info!(
                                        "Found series on Jikan/MAL (via local DB): {} -> {}",
                                        name,
                                        meta.name.as_deref().unwrap_or("Unknown")
                                    );
                                    let mut unified = self.jikan_to_unified(meta);
                                    unified.anilist_id =
                                        provider_ids.anilist_id.map(|id| id.to_string());
                                    unified.anidb_id =
                                        provider_ids.anidb_id.map(|id| id.to_string());
                                    unified.kitsu_id =
                                        provider_ids.kitsu_id.map(|id| id.to_string());
                                    return Ok(Some(unified));
                                }
                            }
                        }
                    }
                }
                Ok(_) => {
                    tracing::debug!("No anime-offline-database match for: {}", name);
                }
                Err(e) => {
                    tracing::debug!("Anime offline database search failed for {}: {}", name, e);
                }
            }
        }

        if let Some(ref tmdb) = self.tmdb {
            match tmdb.get_series_metadata(name, year).await {
                Ok(Some(meta)) => {
                    tracing::info!(
                        "Found series on TMDB: {} -> {}",
                        name,
                        meta.name.as_deref().unwrap_or("Unknown")
                    );
                    return Ok(Some(self.tmdb_series_to_unified(meta)));
                }
                Ok(None) => {
                    tracing::debug!("No TMDB match for: {}", name);
                }
                Err(e) => {
                    tracing::warn!("TMDB search failed for {}: {}", name, e);
                }
            }
        }

        match self.anilist.get_anime_metadata(name, year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found series on AniList: {} -> {}",
                    name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );

                let mut unified = self.anilist_to_unified(meta.clone());
                if self.anime_db.is_enabled() {
                    if let Some(ref anilist_id_str) = meta.anilist_id {
                        if let Ok(anilist_id) = anilist_id_str.parse::<i64>() {
                            if let Ok(Some(entry)) =
                                self.anime_db.find_by_anilist_id(anilist_id).await
                            {
                                let provider_ids = entry.provider_ids();
                                if unified.anidb_id.is_none() {
                                    unified.anidb_id =
                                        provider_ids.anidb_id.map(|id| id.to_string());
                                }
                                if unified.kitsu_id.is_none() {
                                    unified.kitsu_id =
                                        provider_ids.kitsu_id.map(|id| id.to_string());
                                }
                                tracing::debug!(
                                    "Cross-referenced IDs for {}: AniDB={:?}, Kitsu={:?}",
                                    name,
                                    provider_ids.anidb_id,
                                    provider_ids.kitsu_id
                                );
                            }
                        }
                    }
                }

                return Ok(Some(unified));
            }
            Ok(None) => {
                tracing::debug!("No AniList match for: {}", name);
            }
            Err(e) => {
                tracing::warn!("AniList search failed for {}: {}", name, e);
            }
        }

        // Try Jikan (MAL) as final fallback for anime
        match self.jikan.search_anime_best_match(name, year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found series on Jikan/MAL: {} -> {}",
                    name,
                    meta.name.as_deref().unwrap_or("Unknown")
                );
                return Ok(Some(self.jikan_to_unified(meta)));
            }
            Ok(None) => {
                tracing::debug!("No Jikan/MAL match for: {}", name);
            }
            Err(e) => {
                tracing::warn!("Jikan search failed for {}: {}", name, e);
            }
        }

        Ok(None)
    }

    /// Get metadata for a movie
    /// Uses TMDB first, then Jikan/AniList for anime movies
    pub async fn get_movie_metadata(
        &self,
        title: &str,
        year: Option<i32>,
    ) -> Result<Option<UnifiedMetadata>> {
        tracing::debug!("Searching for movie metadata: {} ({:?})", title, year);

        if let Some(ref tmdb) = self.tmdb {
            match tmdb.get_movie_metadata(title, year).await {
                Ok(Some(meta)) => {
                    tracing::info!(
                        "Found movie on TMDB: {} -> {}",
                        title,
                        meta.name.as_deref().unwrap_or("Unknown")
                    );
                    return Ok(Some(self.tmdb_movie_to_unified(meta)));
                }
                Ok(None) => {
                    tracing::debug!("No TMDB match for movie: {}", title);
                }
                Err(e) => {
                    tracing::warn!("TMDB search failed for movie {}: {}", title, e);
                }
            }
        }

        // Try Jikan for anime movies
        match self.jikan.search_anime_best_match(title, year).await {
            Ok(Some(meta)) => {
                tracing::info!(
                    "Found movie on Jikan/MAL: {} -> {}",
                    title,
                    meta.name.as_deref().unwrap_or("Unknown")
                );
                return Ok(Some(self.jikan_to_unified(meta)));
            }
            Ok(None) => {
                tracing::debug!("No Jikan/MAL match for movie: {}", title);
            }
            Err(e) => {
                tracing::debug!("Jikan search failed for movie {}: {}", title, e);
            }
        }

        Ok(None)
    }

    pub fn is_likely_anime(name: &str) -> bool {
        let name_lower = name.to_lowercase();

        let anime_indicators = [
            name.starts_with('['),
            name_lower.contains("-san"),
            name_lower.contains("-kun"),
            name_lower.contains("-chan"),
            name_lower.contains("-sama"),
            name_lower.contains("-sensei"),
            name_lower.contains("-senpai"),
            name_lower.contains("-dono"),
            name_lower.contains("shounen"),
            name_lower.contains("shonen"),
            name_lower.contains("shoujo"),
            name_lower.contains("shojo"),
            name_lower.contains("seinen"),
            name_lower.contains("josei"),
            name_lower.contains("isekai"),
            name_lower.contains("mahou"),
            name_lower.contains("mecha"),
            name_lower.contains("ecchi"),
            name_lower.contains("harem"),
            name_lower.contains("chibi"),
            name_lower.contains(" no "),
            name_lower.contains("-tachi"),
            name_lower.contains("monogatari"),
            name_lower.contains("densetsu"),
            name_lower.contains("bouken"),
            name_lower.contains("[dual-audio]"),
            name_lower.contains("dual-audio"),
            name_lower.contains("[multi-audio]"),
            name_lower.contains("multi-audio"),
            name_lower.contains("x265"),
            name_lower.contains("10-bit"),
            name_lower.contains("10bit"),
            name_lower.contains("hevc"),
            name_lower.contains("flac"),
            name_lower.contains("[bd]"),
            name_lower.contains("[bdrip]"),
            name_lower.contains("[subsplease]"),
            name_lower.contains("[erai-raws]"),
            name_lower.contains("[horriblesubs]"),
            name_lower.contains("[commie]"),
            name_lower.contains("[gg]"),
            name_lower.contains("[reaktor]"),
            name_lower.contains("[judas]"),
            name_lower.contains("[doki]"),
            name_lower.contains("nyaa"),
            name_lower.contains(" 2nd season"),
            name_lower.contains(" 3rd season"),
            name_lower.contains(" ova"),
            name_lower.contains(" ona"),
            name_lower.contains("[ova]"),
            name_lower.contains("[ona]"),
            name_lower.contains("reincarnated"),
            name_lower.contains("otherworld"),
            name_lower.contains("another world"),
            name_lower.contains("villainess"),
            name_lower.contains("demon lord"),
            name_lower.contains("demon king"),
            name_lower.contains("hero"),
            name_lower.contains("saint"),
            name_lower.contains("summoned"),
            name_lower.contains("guild"),
            name_lower.contains("adventurer"),
            name_lower.contains("dungeon"),
            name_lower.contains("kingdom"),
            name_lower.contains("noble"),
            name_lower.contains("prince"),
            name_lower.contains("princess"),
            name_lower.contains("fiancé") || name_lower.contains("fiance"),
            name_lower.contains("engagement"),
            name_lower.contains("magic"),
            name_lower.contains("sorcerer"),
            name_lower.contains("witch"),
            name_lower.contains("slime"),
            name_lower.contains("skill"),
            name_lower.contains("level"),
            name_lower.contains("cheat"),
            name_lower.contains("overpowered"),
            name_lower.contains("strongest"),
            name_lower.contains("weakest"),
            name_lower.contains("tossed aside"),
            name_lower.contains("kicked out"),
            name_lower.contains("banished"),
            name_lower.contains("exiled"),
            name_lower.contains("sold to"),
            name_lower.contains("reborn as"),
            name_lower.contains("became a"),
            name_lower.contains("turned into"),
            name_lower.contains("i was"),
            name_lower.contains("my life as"),
            name.chars().any(|c| matches!(c, '\u{3040}'..='\u{309F}')),
            name.chars().any(|c| matches!(c, '\u{30A0}'..='\u{30FF}')),
            name.chars().any(|c| matches!(c, '\u{4E00}'..='\u{9FFF}')),
        ];

        anime_indicators.iter().any(|&x| x)
    }

    /// Smart metadata lookup that auto-detects content type
    pub async fn get_smart_metadata(
        &self,
        name: &str,
        year: Option<i32>,
        is_movie: bool,
    ) -> Result<Option<UnifiedMetadata>> {
        if is_movie {
            self.get_movie_metadata(name, year).await
        } else if Self::is_likely_anime(name) {
            // Try anime providers first
            self.get_anime_metadata(name, year).await
        } else {
            // Try general series providers
            self.get_series_metadata(name, year).await
        }
    }

    pub async fn get_episode_metadata(
        &self,
        series_metadata: Option<&UnifiedMetadata>,
        season_number: i32,
        episode_number: i32,
    ) -> Result<Option<EpisodeMetadata>> {
        if let Some(ref tmdb) = self.tmdb {
            if let Some(series_meta) = series_metadata {
                if let Some(ref tmdb_id_str) = series_meta.tmdb_id {
                    if let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() {
                        match tmdb
                            .get_episode_metadata(tmdb_id, season_number, episode_number)
                            .await
                        {
                            Ok(Some(meta)) => {
                                tracing::debug!(
                                    "Found episode metadata via TMDB: S{:02}E{:02} - {}",
                                    season_number,
                                    episode_number,
                                    meta.name.as_deref().unwrap_or("Unknown")
                                );
                                return Ok(Some(EpisodeMetadata {
                                    name: meta.name,
                                    overview: meta.overview,
                                    premiere_date: meta.premiere_date,
                                    community_rating: meta.community_rating,
                                    runtime_minutes: meta.runtime_minutes,
                                    still_url: meta
                                        .poster_path
                                        .map(|p| format!("https://image.tmdb.org/t/p/w300{}", p)),
                                }));
                            }
                            Ok(None) => {
                                tracing::debug!(
                                    "No TMDB episode data for S{:02}E{:02}",
                                    season_number,
                                    episode_number
                                );
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "TMDB episode lookup failed for S{:02}E{:02}: {}",
                                    season_number,
                                    episode_number,
                                    e
                                );
                            }
                        }
                    }
                }
            }
        }

        Ok(None)
    }

    pub async fn cache_images(
        &self,
        item_id: &str,
        metadata: &UnifiedMetadata,
    ) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
        let mut poster_path = None;
        let mut backdrop_path = None;

        if let Some(ref url) = metadata.poster_url {
            match self.download_image(url, item_id, "Primary").await {
                Ok(path) => poster_path = Some(path),
                Err(e) => tracing::warn!("Failed to download poster: {}", e),
            }
        }

        if let Some(ref url) = metadata.backdrop_url {
            match self.download_image(url, item_id, "Backdrop").await {
                Ok(path) => backdrop_path = Some(path),
                Err(e) => tracing::warn!("Failed to download backdrop: {}", e),
            }
        }

        Ok((poster_path, backdrop_path))
    }

    async fn download_image(&self, url: &str, item_id: &str, image_type: &str) -> Result<PathBuf> {
        self.anilist.download_image(url, item_id, image_type).await
    }

    /// Download an image to the cache (public version for background downloader)
    pub async fn download_image_to_cache(
        &self,
        url: &str,
        item_id: &str,
        image_type: &str,
    ) -> Result<PathBuf> {
        self.anilist.download_image(url, item_id, image_type).await
    }

    fn anilist_to_unified(&self, meta: AnimeMetadata) -> UnifiedMetadata {
        UnifiedMetadata {
            anilist_id: meta.anilist_id,
            mal_id: meta.mal_id,
            anidb_id: None,
            kitsu_id: None,
            tmdb_id: None,
            imdb_id: None,
            name: meta.name,
            name_original: meta.name_native.or(meta.name_romaji),
            overview: meta.overview,
            year: meta.year,
            premiere_date: meta.premiere_date,
            community_rating: meta.community_rating,
            poster_url: meta.poster_url,
            backdrop_url: meta.backdrop_url,
            episode_count: meta.episode_count,
            runtime_minutes: meta.episode_duration_minutes,
            genres: meta.genres,
            studio: meta.studio,
            cast: meta.cast,
            provider: MetadataProvider::AniList,
        }
    }

    fn anidb_to_unified(&self, meta: AniDBMetadata) -> UnifiedMetadata {
        UnifiedMetadata {
            anidb_id: meta.anidb_id,
            anilist_id: None,
            mal_id: None,
            kitsu_id: None,
            tmdb_id: None,
            imdb_id: None,
            name: meta.name,
            name_original: meta.name_native.or(meta.name_romaji),
            overview: meta.overview,
            year: meta.year,
            premiere_date: meta.premiere_date,
            community_rating: meta.community_rating,
            poster_url: meta.poster_url,
            backdrop_url: None,
            episode_count: meta.episode_count,
            runtime_minutes: None,
            genres: None,
            studio: None,
            cast: Vec::new(),
            provider: MetadataProvider::AniDB,
        }
    }

    fn jikan_to_unified(&self, meta: JikanMetadata) -> UnifiedMetadata {
        UnifiedMetadata {
            mal_id: meta.mal_id,
            anilist_id: None,
            anidb_id: None,
            kitsu_id: None,
            tmdb_id: None,
            imdb_id: None,
            name: meta.name,
            name_original: meta.name_japanese.or(meta.name_english),
            overview: meta.overview,
            year: meta.year,
            premiere_date: meta.premiere_date,
            community_rating: meta.community_rating,
            poster_url: meta.poster_url,
            backdrop_url: meta.backdrop_url,
            episode_count: meta.episode_count,
            runtime_minutes: None,
            genres: meta.genres,
            studio: meta.studio,
            cast: Vec::new(),
            provider: MetadataProvider::Jikan,
        }
    }

    fn tmdb_series_to_unified(&self, meta: MediaMetadata) -> UnifiedMetadata {
        UnifiedMetadata {
            anilist_id: None,
            mal_id: None,
            anidb_id: None,
            kitsu_id: None,
            tmdb_id: meta.tmdb_id,
            imdb_id: meta.imdb_id,
            name: meta.name,
            name_original: None,
            overview: meta.overview,
            year: meta.year,
            premiere_date: meta.premiere_date,
            community_rating: meta.community_rating,
            poster_url: meta
                .poster_path
                .map(|p| format!("https://image.tmdb.org/t/p/w500{}", p)),
            backdrop_url: meta
                .backdrop_path
                .map(|p| format!("https://image.tmdb.org/t/p/w1280{}", p)),
            episode_count: None,
            runtime_minutes: meta.runtime_minutes,
            genres: meta.genres,
            studio: None,
            cast: Self::convert_tmdb_cast(meta.cast),
            provider: MetadataProvider::Tmdb,
        }
    }

    fn tmdb_movie_to_unified(&self, meta: MediaMetadata) -> UnifiedMetadata {
        UnifiedMetadata {
            anilist_id: None,
            mal_id: None,
            anidb_id: None,
            kitsu_id: None,
            tmdb_id: meta.tmdb_id,
            imdb_id: meta.imdb_id,
            name: meta.name,
            name_original: None,
            overview: meta.overview,
            year: meta.year,
            premiere_date: meta.premiere_date,
            community_rating: meta.community_rating,
            poster_url: meta
                .poster_path
                .map(|p| format!("https://image.tmdb.org/t/p/w500{}", p)),
            backdrop_url: meta
                .backdrop_path
                .map(|p| format!("https://image.tmdb.org/t/p/w1280{}", p)),
            episode_count: None,
            runtime_minutes: meta.runtime_minutes,
            genres: meta.genres,
            studio: None,
            cast: Self::convert_tmdb_cast(meta.cast),
            provider: MetadataProvider::Tmdb,
        }
    }

    /// Convert TMDB cast members to unified CastMember format
    fn convert_tmdb_cast(tmdb_cast: Vec<TmdbCastMember>) -> Vec<CastMember> {
        tmdb_cast
            .into_iter()
            .map(|c| CastMember {
                person_id: c.person_id,
                person_name: c.person_name,
                person_image_url: c.person_image_url,
                character_name: c.character_name,
                role: c.role,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_likely_anime() {
        assert!(MetadataService::is_likely_anime(
            "[Reaktor] BECK - Mongolian Chop Squad"
        ));
        assert!(MetadataService::is_likely_anime(
            "[SubsPlease] Anime Name - 01"
        ));
        assert!(MetadataService::is_likely_anime("[Erai-raws] Show Name"));

        assert!(MetadataService::is_likely_anime("Naruto-san"));
        assert!(MetadataService::is_likely_anime("Kaguya-sama: Love is War"));
        assert!(MetadataService::is_likely_anime(
            "Uzaki-chan Wants to Hang Out"
        ));

        assert!(MetadataService::is_likely_anime("Some Show [Dual-Audio]"));
        assert!(MetadataService::is_likely_anime("Anime Title x265 10-bit"));
        assert!(MetadataService::is_likely_anime("Show [BD] [1080p] [HEVC]"));

        assert!(MetadataService::is_likely_anime("Some Isekai Adventure"));
        assert!(MetadataService::is_likely_anime("Mahou Shoujo Title"));
        assert!(MetadataService::is_likely_anime("Seinen Drama"));

        assert!(MetadataService::is_likely_anime("Shingeki no Kyojin"));
        assert!(MetadataService::is_likely_anime("Kimetsu no Yaiba"));

        assert!(MetadataService::is_likely_anime("進撃の巨人"));
        assert!(MetadataService::is_likely_anime("アニメ Title"));

        assert!(MetadataService::is_likely_anime("Series Name OVA"));
        assert!(MetadataService::is_likely_anime("Show [ONA]"));

        assert!(!MetadataService::is_likely_anime("Breaking Bad"));
        assert!(!MetadataService::is_likely_anime("Game of Thrones"));
        assert!(!MetadataService::is_likely_anime("The Office"));
        assert!(!MetadataService::is_likely_anime("Friends S01E01"));
        assert!(!MetadataService::is_likely_anime(
            "The Walking Dead Season 1"
        ));

        assert!(!MetadataService::is_likely_anime("San Andreas (2015)"));
        assert!(!MetadataService::is_likely_anime("The Mandalorian"));
    }
}
