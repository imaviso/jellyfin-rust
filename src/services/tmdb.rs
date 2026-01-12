// TMDB metadata provider service
// API Documentation: https://developer.themoviedb.org/reference/intro/getting-started

use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::fs;

const TMDB_API_BASE: &str = "https://api.themoviedb.org/3";
const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p";

/// TMDB API client
pub struct TmdbClient {
    client: Client,
    api_key: String,
    image_cache_dir: PathBuf,
}

/// Search result for TV shows
#[derive(Debug, Deserialize)]
pub struct TvSearchResults {
    pub results: Vec<TvSearchResult>,
    pub total_results: i32,
}

#[derive(Debug, Deserialize)]
pub struct TvSearchResult {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub first_air_date: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
}

/// Search result for movies
#[derive(Debug, Deserialize)]
pub struct MovieSearchResults {
    pub results: Vec<MovieSearchResult>,
    pub total_results: i32,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MovieSearchResult {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub release_date: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
}

/// Detailed TV show info
#[derive(Debug, Deserialize)]
pub struct TvDetails {
    pub id: i64,
    pub name: String,
    pub original_name: Option<String>,
    pub overview: Option<String>,
    pub first_air_date: Option<String>,
    pub last_air_date: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub number_of_seasons: Option<i32>,
    pub number_of_episodes: Option<i32>,
    pub status: Option<String>,
    pub genres: Option<Vec<Genre>>,
    pub external_ids: Option<ExternalIds>,
    pub credits: Option<Credits>,
}

/// Detailed movie info
#[derive(Debug, Deserialize)]
pub struct MovieDetails {
    pub id: i64,
    pub title: String,
    pub original_title: Option<String>,
    pub overview: Option<String>,
    pub release_date: Option<String>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub vote_average: Option<f64>,
    pub runtime: Option<i32>,
    pub status: Option<String>,
    pub genres: Option<Vec<Genre>>,
    pub imdb_id: Option<String>,
    pub credits: Option<Credits>,
}

/// Season details
#[derive(Debug, Deserialize)]
pub struct SeasonDetails {
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub season_number: i32,
    pub air_date: Option<String>,
    pub episodes: Option<Vec<EpisodeInfo>>,
}

/// Episode info from season details
#[derive(Debug, Deserialize)]
pub struct EpisodeInfo {
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub episode_number: i32,
    pub season_number: i32,
    pub air_date: Option<String>,
    pub still_path: Option<String>,
    pub vote_average: Option<f64>,
    pub runtime: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ExternalIds {
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
}

/// Credits response (cast and crew)
#[derive(Debug, Deserialize)]
pub struct Credits {
    pub cast: Option<Vec<CastMember>>,
    pub crew: Option<Vec<CrewMember>>,
}

/// Cast member from TMDB credits
#[derive(Debug, Clone, Deserialize)]
pub struct CastMember {
    pub id: i64,
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
    pub order: Option<i32>,
}

/// Crew member from TMDB credits
#[derive(Debug, Clone, Deserialize)]
pub struct CrewMember {
    pub id: i64,
    pub name: String,
    pub job: Option<String>,
    pub department: Option<String>,
    pub profile_path: Option<String>,
}

/// Metadata result that can be applied to a media item
#[derive(Debug, Clone, Default)]
pub struct MediaMetadata {
    pub tmdb_id: Option<String>,
    pub imdb_id: Option<String>,
    pub name: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub runtime_minutes: Option<i32>,
    pub genres: Option<Vec<String>>,
    pub cast: Vec<TmdbCastMember>,
}

/// Cast member info for unified metadata
#[derive(Debug, Clone, Default)]
pub struct TmdbCastMember {
    pub person_id: String,
    pub person_name: String,
    pub person_image_url: Option<String>,
    pub character_name: Option<String>,
    pub role: String,
}

/// Image sizes for different purposes
#[derive(Debug, Clone, Copy)]
pub enum ImageSize {
    /// w185 - small poster
    PosterSmall,
    /// w342 - medium poster
    PosterMedium,
    /// w500 - large poster
    PosterLarge,
    /// original - full size poster
    PosterOriginal,
    /// w780 - backdrop
    Backdrop,
    /// w1280 - large backdrop
    BackdropLarge,
    /// original - full size backdrop
    BackdropOriginal,
}

impl ImageSize {
    fn as_str(&self) -> &'static str {
        match self {
            ImageSize::PosterSmall => "w185",
            ImageSize::PosterMedium => "w342",
            ImageSize::PosterLarge => "w500",
            ImageSize::PosterOriginal => "original",
            ImageSize::Backdrop => "w780",
            ImageSize::BackdropLarge => "w1280",
            ImageSize::BackdropOriginal => "original",
        }
    }
}

impl TmdbClient {
    /// Create a new TMDB client
    pub fn new(api_key: String, image_cache_dir: PathBuf) -> Self {
        Self {
            client: Client::new(),
            api_key,
            image_cache_dir,
        }
    }

    /// Create client from environment variable
    pub fn from_env(image_cache_dir: PathBuf) -> Option<Self> {
        std::env::var("TMDB_API_KEY")
            .ok()
            .map(|key| Self::new(key, image_cache_dir))
    }

    /// Search for TV shows by name
    pub async fn search_tv(&self, query: &str, year: Option<i32>) -> Result<Vec<TvSearchResult>> {
        let mut url = format!(
            "{}/search/tv?api_key={}&query={}&include_adult=false",
            TMDB_API_BASE,
            self.api_key,
            urlencoding::encode(query)
        );

        if let Some(y) = year {
            url.push_str(&format!("&first_air_date_year={}", y));
        }

        let response: TvSearchResults = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to search TMDB for TV shows")?
            .json()
            .await
            .context("Failed to parse TMDB TV search response")?;

        Ok(response.results)
    }

    /// Search for movies by name
    pub async fn search_movie(
        &self,
        query: &str,
        year: Option<i32>,
    ) -> Result<Vec<MovieSearchResult>> {
        let mut url = format!(
            "{}/search/movie?api_key={}&query={}&include_adult=false",
            TMDB_API_BASE,
            self.api_key,
            urlencoding::encode(query)
        );

        if let Some(y) = year {
            url.push_str(&format!("&year={}", y));
        }

        let response: MovieSearchResults = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to search TMDB for movies")?
            .json()
            .await
            .context("Failed to parse TMDB movie search response")?;

        Ok(response.results)
    }

    /// Get detailed TV show info
    pub async fn get_tv_details(&self, tmdb_id: i64) -> Result<TvDetails> {
        let url = format!(
            "{}/tv/{}?api_key={}&append_to_response=external_ids,credits",
            TMDB_API_BASE, tmdb_id, self.api_key
        );

        let response: TvDetails = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get TMDB TV details")?
            .json()
            .await
            .context("Failed to parse TMDB TV details response")?;

        Ok(response)
    }

    /// Get detailed movie info
    pub async fn get_movie_details(&self, tmdb_id: i64) -> Result<MovieDetails> {
        let url = format!(
            "{}/movie/{}?api_key={}&append_to_response=credits",
            TMDB_API_BASE, tmdb_id, self.api_key
        );

        let response: MovieDetails = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get TMDB movie details")?
            .json()
            .await
            .context("Failed to parse TMDB movie details response")?;

        Ok(response)
    }

    /// Get season details including episode list
    pub async fn get_season_details(
        &self,
        tv_id: i64,
        season_number: i32,
    ) -> Result<SeasonDetails> {
        let url = format!(
            "{}/tv/{}/season/{}?api_key={}",
            TMDB_API_BASE, tv_id, season_number, self.api_key
        );

        let response: SeasonDetails = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to get TMDB season details")?
            .json()
            .await
            .context("Failed to parse TMDB season details response")?;

        Ok(response)
    }

    /// Download and cache an image, returns the local path
    pub async fn download_image(
        &self,
        tmdb_path: &str,
        size: ImageSize,
        item_id: &str,
        image_type: &str,
    ) -> Result<PathBuf> {
        // Create cache directory structure: cache/images/{item_id}/
        let item_cache_dir = self.image_cache_dir.join(item_id);
        fs::create_dir_all(&item_cache_dir).await?;

        // Determine file extension from TMDB path
        let ext = Path::new(tmdb_path)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("jpg");

        let local_filename = format!("{}.{}", image_type, ext);
        let local_path = item_cache_dir.join(&local_filename);

        // Skip if already cached (use async check to avoid blocking)
        if fs::try_exists(&local_path).await.unwrap_or(false) {
            tracing::debug!("Image already cached: {:?}", local_path);
            return Ok(local_path);
        }

        // Download from TMDB
        let url = format!("{}/{}{}", TMDB_IMAGE_BASE, size.as_str(), tmdb_path);
        tracing::debug!("Downloading image: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to download image from TMDB")?;

        if !response.status().is_success() {
            anyhow::bail!(
                "TMDB image download failed with status: {}",
                response.status()
            );
        }

        let bytes = response.bytes().await?;
        fs::write(&local_path, &bytes).await?;

        tracing::info!("Downloaded image to {:?}", local_path);
        Ok(local_path)
    }

    /// Search and get metadata for a TV series
    pub async fn get_series_metadata(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Option<MediaMetadata>> {
        let results = self.search_tv(name, year).await?;

        // Validate that results actually match our query
        let query_lower = name.to_lowercase();
        let query_clean = query_lower
            .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
            .trim();

        let best_match = results.into_iter().find(|result| {
            let title_lower = result.name.to_lowercase();
            let title_clean = title_lower
                .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
                .trim();

            // Check original name too
            let orig_lower = result.original_name.as_deref().unwrap_or("").to_lowercase();
            let orig_clean = orig_lower
                .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
                .trim();

            // Exact or substring match
            if title_clean == query_clean || orig_clean == query_clean {
                return true;
            }
            if title_clean.contains(query_clean) || query_clean.contains(title_clean) {
                let shorter = query_clean.len().min(title_clean.len());
                let longer = query_clean.len().max(title_clean.len());
                if shorter > 0 && shorter as f64 / longer as f64 > 0.4 {
                    return true;
                }
            }
            if !orig_clean.is_empty()
                && (orig_clean.contains(query_clean) || query_clean.contains(orig_clean))
            {
                let shorter = query_clean.len().min(orig_clean.len());
                let longer = query_clean.len().max(orig_clean.len());
                if shorter > 0 && shorter as f64 / longer as f64 > 0.4 {
                    return true;
                }
            }

            // Word overlap check
            let query_words: std::collections::HashSet<&str> =
                query_clean.split_whitespace().collect();
            let title_words: std::collections::HashSet<&str> =
                title_clean.split_whitespace().collect();
            let common_words = query_words.intersection(&title_words).count();

            if !query_words.is_empty() && !title_words.is_empty() {
                let match_ratio =
                    common_words as f64 / query_words.len().min(title_words.len()) as f64;
                if match_ratio >= 0.6 || (common_words >= 2 && match_ratio >= 0.4) {
                    return true;
                }
            }

            false
        });

        if let Some(result) = best_match {
            // Get detailed info for more data
            let details = self.get_tv_details(result.id).await?;

            let year = details
                .first_air_date
                .as_ref()
                .and_then(|d| d.split('-').next())
                .and_then(|y| y.parse().ok());

            // Extract cast (limit to top 20 to keep it manageable)
            let cast = Self::extract_cast(&details.credits, 20);

            Ok(Some(MediaMetadata {
                tmdb_id: Some(details.id.to_string()),
                imdb_id: details.external_ids.and_then(|e| e.imdb_id),
                name: Some(details.name),
                overview: details.overview,
                year,
                premiere_date: details.first_air_date,
                community_rating: details.vote_average,
                poster_path: details.poster_path,
                backdrop_path: details.backdrop_path,
                runtime_minutes: None,
                genres: details
                    .genres
                    .map(|g| g.into_iter().map(|genre| genre.name).collect()),
                cast,
            }))
        } else {
            tracing::debug!(
                "TMDB search returned results for '{}' but none matched well enough",
                name
            );
            Ok(None)
        }
    }

    /// Search and get metadata for a movie
    pub async fn get_movie_metadata(
        &self,
        title: &str,
        year: Option<i32>,
    ) -> Result<Option<MediaMetadata>> {
        let results = self.search_movie(title, year).await?;

        // Validate that results actually match our query
        let query_lower = title.to_lowercase();
        let query_clean = query_lower
            .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
            .trim();

        let title_matches = |result: &MovieSearchResult| -> bool {
            let title_lower = result.title.to_lowercase();
            let title_clean = title_lower
                .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
                .trim();

            let orig_lower = result
                .original_title
                .as_deref()
                .unwrap_or("")
                .to_lowercase();
            let orig_clean = orig_lower
                .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
                .trim();

            // Exact or substring match
            if title_clean == query_clean || orig_clean == query_clean {
                return true;
            }
            if title_clean.contains(query_clean) || query_clean.contains(title_clean) {
                let shorter = query_clean.len().min(title_clean.len());
                let longer = query_clean.len().max(title_clean.len());
                if shorter > 0 && shorter as f64 / longer as f64 > 0.4 {
                    return true;
                }
            }
            if !orig_clean.is_empty()
                && (orig_clean.contains(query_clean) || query_clean.contains(orig_clean))
            {
                let shorter = query_clean.len().min(orig_clean.len());
                let longer = query_clean.len().max(orig_clean.len());
                if shorter > 0 && shorter as f64 / longer as f64 > 0.4 {
                    return true;
                }
            }

            // Word overlap check
            let query_words: std::collections::HashSet<&str> =
                query_clean.split_whitespace().collect();
            let title_words: std::collections::HashSet<&str> =
                title_clean.split_whitespace().collect();
            let common_words = query_words.intersection(&title_words).count();

            if !query_words.is_empty() && !title_words.is_empty() {
                let match_ratio =
                    common_words as f64 / query_words.len().min(title_words.len()) as f64;
                if match_ratio >= 0.6 || (common_words >= 2 && match_ratio >= 0.4) {
                    return true;
                }
            }

            false
        };

        // Prefer exact year match if provided, but still validate title
        let best_match = if let Some(target_year) = year {
            results
                .iter()
                .find(|r| {
                    title_matches(r)
                        && r.release_date
                            .as_ref()
                            .and_then(|d| d.split('-').next())
                            .and_then(|y| y.parse::<i32>().ok())
                            == Some(target_year)
                })
                .cloned()
                .or_else(|| results.into_iter().find(|r| title_matches(r)))
        } else {
            results.into_iter().find(|r| title_matches(r))
        };

        if let Some(result) = best_match {
            // Get detailed info
            let details = self.get_movie_details(result.id).await?;

            let year = details
                .release_date
                .as_ref()
                .and_then(|d| d.split('-').next())
                .and_then(|y| y.parse().ok());

            // Extract cast (limit to top 20)
            let cast = Self::extract_cast(&details.credits, 20);

            Ok(Some(MediaMetadata {
                tmdb_id: Some(details.id.to_string()),
                imdb_id: details.imdb_id,
                name: Some(details.title),
                overview: details.overview,
                year,
                premiere_date: details.release_date,
                community_rating: details.vote_average,
                poster_path: details.poster_path,
                backdrop_path: details.backdrop_path,
                runtime_minutes: details.runtime,
                genres: details
                    .genres
                    .map(|g| g.into_iter().map(|genre| genre.name).collect()),
                cast,
            }))
        } else {
            tracing::debug!(
                "TMDB movie search returned results for '{}' but none matched well enough",
                title
            );
            Ok(None)
        }
    }

    /// Get episode metadata from season info
    pub async fn get_episode_metadata(
        &self,
        tv_id: i64,
        season_number: i32,
        episode_number: i32,
    ) -> Result<Option<MediaMetadata>> {
        let season = self.get_season_details(tv_id, season_number).await?;

        if let Some(episodes) = season.episodes {
            if let Some(episode) = episodes.iter().find(|e| e.episode_number == episode_number) {
                return Ok(Some(MediaMetadata {
                    tmdb_id: Some(episode.id.to_string()),
                    imdb_id: None,
                    name: Some(episode.name.clone()),
                    overview: episode.overview.clone(),
                    year: None,
                    premiere_date: episode.air_date.clone(),
                    community_rating: episode.vote_average,
                    poster_path: episode.still_path.clone(), // Episode stills go to poster
                    backdrop_path: None,
                    runtime_minutes: episode.runtime,
                    genres: None,     // Episodes don't have genres
                    cast: Vec::new(), // Episodes don't have cast data here
                }));
            }
        }

        Ok(None)
    }

    /// Download and cache images for an item, returns (poster_path, backdrop_path)
    pub async fn cache_item_images(
        &self,
        item_id: &str,
        poster_path: Option<&str>,
        backdrop_path: Option<&str>,
    ) -> Result<(Option<PathBuf>, Option<PathBuf>)> {
        let mut cached_poster = None;
        let mut cached_backdrop = None;

        if let Some(poster) = poster_path {
            match self
                .download_image(poster, ImageSize::PosterLarge, item_id, "Primary")
                .await
            {
                Ok(path) => cached_poster = Some(path),
                Err(e) => tracing::warn!("Failed to download poster: {}", e),
            }
        }

        if let Some(backdrop) = backdrop_path {
            match self
                .download_image(backdrop, ImageSize::BackdropLarge, item_id, "Backdrop")
                .await
            {
                Ok(path) => cached_backdrop = Some(path),
                Err(e) => tracing::warn!("Failed to download backdrop: {}", e),
            }
        }

        Ok((cached_poster, cached_backdrop))
    }

    /// Extract cast members from TMDB credits response
    fn extract_cast(credits: &Option<Credits>, limit: usize) -> Vec<TmdbCastMember> {
        let Some(credits) = credits else {
            return Vec::new();
        };

        let mut result = Vec::new();

        // Add cast (actors)
        if let Some(cast) = &credits.cast {
            for member in cast.iter().take(limit) {
                result.push(TmdbCastMember {
                    person_id: format!("tmdb-person-{}", member.id),
                    person_name: member.name.clone(),
                    person_image_url: member
                        .profile_path
                        .as_ref()
                        .map(|p| format!("https://image.tmdb.org/t/p/w185{}", p)),
                    character_name: member.character.clone(),
                    role: "Actor".to_string(),
                });
            }
        }

        // Add key crew members (director, writer) if there's room
        if result.len() < limit {
            if let Some(crew) = &credits.crew {
                let remaining = limit - result.len();
                for member in crew
                    .iter()
                    .filter(|c| {
                        matches!(
                            c.job.as_deref(),
                            Some("Director") | Some("Writer") | Some("Screenplay")
                        )
                    })
                    .take(remaining)
                {
                    result.push(TmdbCastMember {
                        person_id: format!("tmdb-person-{}", member.id),
                        person_name: member.name.clone(),
                        person_image_url: member
                            .profile_path
                            .as_ref()
                            .map(|p| format!("https://image.tmdb.org/t/p/w185{}", p)),
                        character_name: None,
                        role: member.job.clone().unwrap_or_else(|| "Crew".to_string()),
                    });
                }
            }
        }

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_image_size_str() {
        assert_eq!(ImageSize::PosterLarge.as_str(), "w500");
        assert_eq!(ImageSize::BackdropLarge.as_str(), "w1280");
    }
}
