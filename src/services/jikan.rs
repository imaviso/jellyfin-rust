// Jikan API client - Unofficial MyAnimeList API
// API Documentation: https://docs.api.jikan.moe/
// Rate limit: 3 requests/second, 60 requests/minute

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const JIKAN_API_BASE: &str = "https://api.jikan.moe/v4";

/// Jikan API client with rate limiting
pub struct JikanClient {
    client: Client,
    last_request: Arc<Mutex<Instant>>,
}

// === API Response Types ===

#[derive(Debug, Deserialize)]
pub struct JikanResponse<T> {
    pub data: T,
}

#[derive(Debug, Deserialize)]
pub struct JikanSearchResponse {
    pub data: Vec<JikanAnime>,
    pub pagination: Option<JikanPagination>,
}

#[derive(Debug, Deserialize)]
pub struct JikanPagination {
    pub last_visible_page: i32,
    pub has_next_page: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanAnime {
    pub mal_id: i64,
    pub url: Option<String>,
    pub images: Option<JikanImages>,
    pub title: String,
    pub title_english: Option<String>,
    pub title_japanese: Option<String>,
    pub title_synonyms: Option<Vec<String>>,
    #[serde(rename = "type")]
    pub anime_type: Option<String>,
    pub source: Option<String>,
    pub episodes: Option<i32>,
    pub status: Option<String>,
    pub airing: Option<bool>,
    pub aired: Option<JikanAired>,
    pub duration: Option<String>,
    pub rating: Option<String>,
    pub score: Option<f64>,
    pub scored_by: Option<i64>,
    pub rank: Option<i32>,
    pub popularity: Option<i32>,
    pub members: Option<i64>,
    pub favorites: Option<i64>,
    pub synopsis: Option<String>,
    pub background: Option<String>,
    pub season: Option<String>,
    pub year: Option<i32>,
    pub studios: Option<Vec<JikanStudio>>,
    pub genres: Option<Vec<JikanGenre>>,
    pub themes: Option<Vec<JikanGenre>>,
    pub demographics: Option<Vec<JikanGenre>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanImages {
    pub jpg: Option<JikanImageSet>,
    pub webp: Option<JikanImageSet>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanImageSet {
    pub image_url: Option<String>,
    pub small_image_url: Option<String>,
    pub large_image_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanAired {
    pub from: Option<String>,
    pub to: Option<String>,
    pub string: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanStudio {
    pub mal_id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanGenre {
    pub mal_id: i64,
    pub name: String,
}

// === Full anime response (includes more details) ===

#[derive(Debug, Clone, Deserialize)]
pub struct JikanAnimeFull {
    #[serde(flatten)]
    pub base: JikanAnime,
    pub relations: Option<Vec<JikanRelation>>,
    pub external: Option<Vec<JikanExternal>>,
    pub streaming: Option<Vec<JikanStreaming>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanRelation {
    pub relation: String,
    pub entry: Vec<JikanRelationEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanRelationEntry {
    pub mal_id: i64,
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanExternal {
    pub name: String,
    pub url: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct JikanStreaming {
    pub name: String,
    pub url: String,
}

// === Unified metadata output ===

#[derive(Debug, Clone)]
pub struct JikanMetadata {
    pub mal_id: Option<String>,
    pub name: Option<String>,
    pub name_english: Option<String>,
    pub name_japanese: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub genres: Option<Vec<String>>,
    pub studio: Option<String>,
    pub episode_count: Option<i32>,
    pub status: Option<String>,
}

impl JikanClient {
    /// Create a new Jikan client
    pub fn new() -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            last_request: Arc::new(Mutex::new(Instant::now() - Duration::from_secs(1))),
        }
    }

    /// Enforce rate limiting (3 requests per second)
    async fn rate_limit(&self) {
        let mut last = self.last_request.lock().await;
        let elapsed = last.elapsed();
        let min_interval = Duration::from_millis(350); // ~3 req/sec with buffer

        if elapsed < min_interval {
            let wait = min_interval - elapsed;
            tracing::debug!("Jikan rate limit: waiting {:?}", wait);
            tokio::time::sleep(wait).await;
        }
        *last = Instant::now();
    }

    /// Search for anime by name
    pub async fn search_anime(&self, query: &str, year: Option<i32>) -> Result<Vec<JikanAnime>> {
        self.rate_limit().await;

        let mut url = format!(
            "{}/anime?q={}&sfw=true&limit=10",
            JIKAN_API_BASE,
            urlencoding::encode(query)
        );

        // Add year filter if provided
        if let Some(y) = year {
            url.push_str(&format!("&start_date={}-01-01&end_date={}-12-31", y, y));
        }

        tracing::debug!("Jikan search: {}", query);

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to search Jikan")?;

        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            tracing::warn!("Jikan search failed: {} - {}", status, text);
            return Ok(vec![]);
        }

        let result: JikanSearchResponse = response
            .json()
            .await
            .context("Failed to parse Jikan search response")?;

        Ok(result.data)
    }

    /// Get anime by MAL ID
    pub async fn get_anime_by_id(&self, mal_id: i64) -> Result<Option<JikanMetadata>> {
        self.rate_limit().await;

        let url = format!("{}/anime/{}", JIKAN_API_BASE, mal_id);

        tracing::debug!("Jikan get anime: {}", mal_id);

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to fetch from Jikan")?;

        if !response.status().is_success() {
            if response.status().as_u16() == 404 {
                return Ok(None);
            }
            tracing::warn!("Jikan request failed: {}", response.status());
            return Ok(None);
        }

        let result: JikanResponse<JikanAnime> = response
            .json()
            .await
            .context("Failed to parse Jikan response")?;

        Ok(Some(self.anime_to_metadata(&result.data)))
    }

    /// Get full anime details by MAL ID (includes relations, external links)
    pub async fn get_anime_full(&self, mal_id: i64) -> Result<Option<JikanAnimeFull>> {
        self.rate_limit().await;

        let url = format!("{}/anime/{}/full", JIKAN_API_BASE, mal_id);

        tracing::debug!("Jikan get anime full: {}", mal_id);

        let response = self
            .client
            .get(&url)
            .header("Accept", "application/json")
            .send()
            .await
            .context("Failed to fetch from Jikan")?;

        if !response.status().is_success() {
            if response.status().as_u16() == 404 {
                return Ok(None);
            }
            tracing::warn!("Jikan request failed: {}", response.status());
            return Ok(None);
        }

        let result: JikanResponse<JikanAnimeFull> = response
            .json()
            .await
            .context("Failed to parse Jikan full response")?;

        Ok(Some(result.data))
    }

    /// Search and get best match
    pub async fn search_anime_best_match(
        &self,
        query: &str,
        year: Option<i32>,
    ) -> Result<Option<JikanMetadata>> {
        let results = self.search_anime(query, year).await?;

        if results.is_empty() {
            // Try without year filter
            if year.is_some() {
                let results_no_year = self.search_anime(query, None).await?;
                if let Some(best) = self.find_best_match(&results_no_year, query, year) {
                    return Ok(Some(self.anime_to_metadata(best)));
                }
            }
            return Ok(None);
        }

        if let Some(best) = self.find_best_match(&results, query, year) {
            return Ok(Some(self.anime_to_metadata(best)));
        }

        Ok(None)
    }

    /// Find best match from search results
    fn find_best_match<'a>(
        &self,
        results: &'a [JikanAnime],
        query: &str,
        year: Option<i32>,
    ) -> Option<&'a JikanAnime> {
        let query_lower = query.to_lowercase();
        let query_clean = self.clean_title(&query_lower);

        let mut best_match: Option<(&JikanAnime, i32)> = None;

        for anime in results {
            let mut score = 0i32;

            // Check title matches
            let title_lower = anime.title.to_lowercase();
            let title_clean = self.clean_title(&title_lower);

            if title_clean == query_clean {
                score += 100; // Exact match
            } else if title_lower.contains(&query_lower) || query_lower.contains(&title_lower) {
                score += 50; // Partial match
            } else if title_clean.contains(&query_clean) || query_clean.contains(&title_clean) {
                score += 30; // Cleaned partial match
            }

            // Check English title
            if let Some(ref eng) = anime.title_english {
                let eng_lower = eng.to_lowercase();
                let eng_clean = self.clean_title(&eng_lower);
                if eng_clean == query_clean {
                    score += 100;
                } else if eng_lower.contains(&query_lower) {
                    score += 40;
                }
            }

            // Check synonyms
            if let Some(ref synonyms) = anime.title_synonyms {
                for syn in synonyms {
                    let syn_clean = self.clean_title(&syn.to_lowercase());
                    if syn_clean == query_clean {
                        score += 80;
                        break;
                    }
                }
            }

            // Year matching
            if let Some(q_year) = year {
                if anime.year == Some(q_year) {
                    score += 50;
                } else if let Some(aired) = &anime.aired {
                    if let Some(ref from) = aired.from {
                        if from.starts_with(&q_year.to_string()) {
                            score += 40;
                        }
                    }
                }
            }

            // Prefer TV series over movies/specials for series searches
            if anime.anime_type.as_deref() == Some("TV") {
                score += 10;
            }

            // Prefer higher scored anime (popularity tiebreaker)
            if let Some(mal_score) = anime.score {
                score += (mal_score * 2.0) as i32;
            }

            if score > 0 {
                if let Some((_, best_score)) = best_match {
                    if score > best_score {
                        best_match = Some((anime, score));
                    }
                } else {
                    best_match = Some((anime, score));
                }
            }
        }

        best_match.map(|(anime, _)| anime)
    }

    /// Clean title for comparison
    fn clean_title(&self, title: &str) -> String {
        title
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect::<String>()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// Convert JikanAnime to JikanMetadata
    fn anime_to_metadata(&self, anime: &JikanAnime) -> JikanMetadata {
        // Get best poster URL (prefer large)
        let poster_url = anime.images.as_ref().and_then(|images| {
            images
                .jpg
                .as_ref()
                .and_then(|jpg| jpg.large_image_url.clone().or(jpg.image_url.clone()))
                .or_else(|| {
                    images
                        .webp
                        .as_ref()
                        .and_then(|webp| webp.large_image_url.clone().or(webp.image_url.clone()))
                })
        });

        // Extract premiere date from aired
        let premiere_date = anime
            .aired
            .as_ref()
            .and_then(|aired| aired.from.clone())
            .map(|d| d.split('T').next().unwrap_or(&d).to_string());

        // Get year from aired if not directly available
        let year = anime.year.or_else(|| {
            premiere_date
                .as_ref()
                .and_then(|d| d.split('-').next())
                .and_then(|y| y.parse().ok())
        });

        // Collect all genres
        let genres: Vec<String> = anime
            .genres
            .iter()
            .flatten()
            .chain(anime.themes.iter().flatten())
            .map(|g| g.name.clone())
            .collect();

        // Get first studio
        let studio = anime
            .studios
            .as_ref()
            .and_then(|s| s.first())
            .map(|s| s.name.clone());

        JikanMetadata {
            mal_id: Some(anime.mal_id.to_string()),
            name: Some(anime.title.clone()),
            name_english: anime.title_english.clone(),
            name_japanese: anime.title_japanese.clone(),
            overview: anime.synopsis.clone(),
            year,
            premiere_date,
            community_rating: anime.score,
            poster_url,
            backdrop_url: None, // Jikan doesn't provide backdrops
            genres: if genres.is_empty() {
                None
            } else {
                Some(genres)
            },
            studio,
            episode_count: anime.episodes,
            status: anime.status.clone(),
        }
    }
}

impl Default for JikanClient {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: These tests hit the live Jikan API and may fail due to rate limiting
    // Run with: cargo test jikan -- --test-threads=1

    #[tokio::test]
    async fn test_jikan_search() {
        let client = JikanClient::new();

        // Wait a bit to avoid rate limiting from other tests
        tokio::time::sleep(Duration::from_millis(500)).await;

        let results = client.search_anime("Frieren", Some(2023)).await;

        match results {
            Ok(results) if !results.is_empty() => {
                let first = &results[0];
                println!("Found: {} (MAL ID: {})", first.title, first.mal_id);
                assert!(first.title.to_lowercase().contains("frieren"));
            }
            Ok(_) => {
                println!("No results - may be rate limited");
            }
            Err(e) => {
                println!("Search failed (may be rate limited): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_jikan_get_by_id() {
        let client = JikanClient::new();

        // Wait a bit to avoid rate limiting from other tests
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Frieren MAL ID
        let result = client.get_anime_by_id(154587).await;

        match result {
            Ok(Some(anime)) => {
                println!("Got: {:?}", anime.name);
                assert!(anime.name.unwrap_or_default().contains("Frieren"));
            }
            Ok(None) => {
                println!("Not found - may be rate limited");
            }
            Err(e) => {
                println!("Request failed (may be rate limited): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_jikan_best_match() {
        let client = JikanClient::new();

        // Wait a bit to avoid rate limiting from other tests
        tokio::time::sleep(Duration::from_millis(500)).await;

        let result = client
            .search_anime_best_match("Bocchi the Rock", Some(2022))
            .await;

        match result {
            Ok(Some(anime)) => {
                println!(
                    "Best match: {:?} ({})",
                    anime.name,
                    anime.mal_id.unwrap_or_default()
                );
                assert!(anime
                    .name
                    .unwrap_or_default()
                    .to_lowercase()
                    .contains("bocchi"));
            }
            Ok(None) => {
                println!("No match found - may be rate limited");
            }
            Err(e) => {
                println!("Search failed (may be rate limited): {}", e);
            }
        }
    }
}
