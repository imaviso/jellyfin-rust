use anyhow::{Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::fs;

const ANILIST_API_URL: &str = "https://graphql.anilist.co";

/// AniList API client
pub struct AniListClient {
    client: Client,
    image_cache_dir: PathBuf,
}

/// GraphQL request wrapper
#[derive(Debug, Serialize)]
struct GraphQLRequest {
    query: String,
    variables: serde_json::Value,
}

/// Search response wrapper
#[derive(Debug, Deserialize)]
struct SearchResponse {
    data: Option<SearchData>,
}

#[derive(Debug, Deserialize)]
struct SearchData {
    #[serde(rename = "Page")]
    page: Option<PageData>,
    #[serde(rename = "Media")]
    media: Option<MediaData>,
}

#[derive(Debug, Deserialize)]
struct PageData {
    media: Option<Vec<MediaData>>,
}

/// AniList media (anime) data
#[derive(Debug, Clone, Deserialize)]
pub struct MediaData {
    pub id: i64,
    pub title: Option<TitleData>,
    pub description: Option<String>,
    #[serde(rename = "startDate")]
    pub start_date: Option<FuzzyDate>,
    #[serde(rename = "endDate")]
    pub end_date: Option<FuzzyDate>,
    #[serde(rename = "coverImage")]
    pub cover_image: Option<CoverImage>,
    #[serde(rename = "bannerImage")]
    pub banner_image: Option<String>,
    #[serde(rename = "averageScore")]
    pub average_score: Option<i32>,
    pub episodes: Option<i32>,
    pub duration: Option<i32>,
    pub genres: Option<Vec<String>>,
    pub studios: Option<StudioConnection>,
    pub characters: Option<CharacterConnection>,
    #[serde(rename = "idMal")]
    pub id_mal: Option<i64>,
    pub format: Option<String>,
    pub status: Option<String>,
    #[serde(rename = "seasonYear")]
    pub season_year: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TitleData {
    pub romaji: Option<String>,
    pub english: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FuzzyDate {
    pub year: Option<i32>,
    pub month: Option<i32>,
    pub day: Option<i32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CoverImage {
    #[serde(rename = "extraLarge")]
    pub extra_large: Option<String>,
    pub large: Option<String>,
    pub medium: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StudioConnection {
    pub nodes: Option<Vec<Studio>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Studio {
    pub name: String,
    #[serde(rename = "isAnimationStudio")]
    pub is_animation_studio: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CharacterConnection {
    pub edges: Option<Vec<CharacterEdge>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CharacterEdge {
    pub node: Option<Character>,
    pub role: Option<String>,
    #[serde(rename = "voiceActors")]
    pub voice_actors: Option<Vec<Staff>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Character {
    pub id: i64,
    pub name: Option<CharacterName>,
    pub image: Option<CharacterImage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CharacterName {
    pub full: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CharacterImage {
    pub large: Option<String>,
    pub medium: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Staff {
    pub id: i64,
    pub name: Option<StaffName>,
    pub image: Option<StaffImage>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StaffName {
    pub full: Option<String>,
    pub native: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StaffImage {
    pub large: Option<String>,
    pub medium: Option<String>,
}

/// Metadata result compatible with our system
#[derive(Debug, Clone, Default)]
pub struct AnimeMetadata {
    pub anilist_id: Option<String>,
    pub mal_id: Option<String>,
    pub name: Option<String>,
    pub name_romaji: Option<String>,
    pub name_native: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub poster_url: Option<String>,
    pub backdrop_url: Option<String>,
    pub episode_count: Option<i32>,
    pub episode_duration_minutes: Option<i32>,
    pub genres: Option<Vec<String>>,
    pub studio: Option<String>,
    pub cast: Vec<CastMember>,
}

/// A cast member (voice actor + character)
#[derive(Debug, Clone)]
pub struct CastMember {
    pub person_id: String,
    pub person_name: String,
    pub person_image_url: Option<String>,
    pub character_name: Option<String>,
    pub role: String,
}

impl AniListClient {
    /// Create a new AniList client (no API key needed!)
    pub fn new(image_cache_dir: PathBuf) -> Self {
        Self {
            client: Client::new(),
            image_cache_dir,
        }
    }

    /// Search for anime by title
    pub async fn search_anime(&self, query: &str, year: Option<i32>) -> Result<Vec<MediaData>> {
        let graphql_query = r#"
            query ($search: String, $year: Int) {
                Page(page: 1, perPage: 10) {
                    media(search: $search, seasonYear: $year, type: ANIME, sort: SEARCH_MATCH) {
                        id
                        idMal
                        title {
                            romaji
                            english
                            native
                        }
                        description(asHtml: false)
                        startDate {
                            year
                            month
                            day
                        }
                        endDate {
                            year
                            month
                            day
                        }
                        coverImage {
                            extraLarge
                            large
                            medium
                        }
                        bannerImage
                        averageScore
                        episodes
                        duration
                        genres
                        studios(isMain: true) {
                            nodes {
                                name
                                isAnimationStudio
                            }
                        }
                        format
                        status
                        seasonYear
                    }
                }
            }
        "#;

        let variables = serde_json::json!({
            "search": query,
            "year": year
        });

        let request = GraphQLRequest {
            query: graphql_query.to_string(),
            variables,
        };

        let response: SearchResponse = self
            .client
            .post(ANILIST_API_URL)
            .json(&request)
            .send()
            .await
            .context("Failed to search AniList")?
            .json()
            .await
            .context("Failed to parse AniList search response")?;

        Ok(response
            .data
            .and_then(|d| d.page)
            .and_then(|p| p.media)
            .unwrap_or_default())
    }

    /// Get detailed anime info by AniList ID
    pub async fn get_anime_details(&self, anilist_id: i64) -> Result<Option<MediaData>> {
        let graphql_query = r#"
            query ($id: Int) {
                Media(id: $id, type: ANIME) {
                    id
                    idMal
                    title {
                        romaji
                        english
                        native
                    }
                    description(asHtml: false)
                    startDate {
                        year
                        month
                        day
                    }
                    endDate {
                        year
                        month
                        day
                    }
                    coverImage {
                        extraLarge
                        large
                        medium
                    }
                    bannerImage
                    averageScore
                    episodes
                    duration
                    genres
                    studios(isMain: true) {
                        nodes {
                            name
                            isAnimationStudio
                        }
                    }
                    characters(sort: ROLE, perPage: 25) {
                        edges {
                            node {
                                id
                                name {
                                    full
                                    native
                                }
                                image {
                                    large
                                    medium
                                }
                            }
                            role
                            voiceActors(language: JAPANESE) {
                                id
                                name {
                                    full
                                    native
                                }
                                image {
                                    large
                                    medium
                                }
                                language
                            }
                        }
                    }
                    format
                    status
                    seasonYear
                }
            }
        "#;

        let variables = serde_json::json!({
            "id": anilist_id
        });

        let request = GraphQLRequest {
            query: graphql_query.to_string(),
            variables,
        };

        let response: SearchResponse = self
            .client
            .post(ANILIST_API_URL)
            .json(&request)
            .send()
            .await
            .context("Failed to get AniList details")?
            .json()
            .await
            .context("Failed to parse AniList details response")?;

        Ok(response.data.and_then(|d| d.media))
    }

    /// Search and get metadata for an anime series
    pub async fn get_anime_metadata(
        &self,
        name: &str,
        year: Option<i32>,
    ) -> Result<Option<AnimeMetadata>> {
        let results = self.search_anime(name, year).await?;

        if results.is_empty() {
            return Ok(None);
        }

        // Clean up query for matching
        let query_lower = name.to_lowercase();
        // Remove year suffix if present (e.g., "Show Name (2024)" -> "show name")
        let query_clean = query_lower
            .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
            .trim()
            .to_string();

        // Try to find a matching result
        for media in &results {
            if self.is_title_match(&query_clean, media, year) {
                return Ok(Some(self.media_to_metadata(media)));
            }
        }

        // If we have a year filter and the first result matches the year, trust AniList's search
        // This handles cases where English folder names don't match Romaji titles
        if let Some(search_year) = year {
            let first = &results[0];
            let result_year = first
                .season_year
                .or_else(|| first.start_date.as_ref().and_then(|d| d.year));

            if result_year == Some(search_year) {
                tracing::info!(
                    "AniList: No title match for '{}' but year {} matches, trusting first result",
                    name,
                    search_year
                );
                return Ok(Some(self.media_to_metadata(first)));
            }
        }

        tracing::debug!(
            "AniList search returned results for '{}' but none matched well enough",
            name
        );
        Ok(None)
    }

    /// Check if a media result matches our search query
    fn is_title_match(&self, query_clean: &str, media: &MediaData, year: Option<i32>) -> bool {
        let Some(ref title) = media.title else {
            return false;
        };

        // Check all title variants (English, Romaji, Native)
        let titles_to_check = [
            title.english.as_deref(),
            title.romaji.as_deref(),
            title.native.as_deref(),
        ];

        for title_opt in titles_to_check.iter().flatten() {
            let title_lower = title_opt.to_lowercase();
            let title_clean = title_lower
                .trim_end_matches(|c: char| c == ')' || c.is_ascii_digit() || c == '(' || c == ' ')
                .trim();

            // 1. Exact match
            if title_clean == query_clean {
                return true;
            }

            // 2. Substring containment (with length ratio check)
            if title_clean.contains(query_clean) || query_clean.contains(title_clean) {
                let shorter = query_clean.len().min(title_clean.len());
                let longer = query_clean.len().max(title_clean.len());
                if shorter as f64 / longer as f64 > 0.4 {
                    return true;
                }
            }

            // 3. Word-based matching
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
        }

        false
    }

    /// Convert AniList MediaData to our AnimeMetadata format
    fn media_to_metadata(&self, media: &MediaData) -> AnimeMetadata {
        let title = media.title.as_ref();

        // Prefer English title, fall back to Romaji
        let name = title
            .and_then(|t| t.english.clone())
            .or_else(|| title.and_then(|t| t.romaji.clone()));

        let premiere_date = media.start_date.as_ref().map(|d| {
            format!(
                "{:04}-{:02}-{:02}",
                d.year.unwrap_or(0),
                d.month.unwrap_or(1),
                d.day.unwrap_or(1)
            )
        });

        // AniList scores are 0-100, convert to 0-10 scale
        let rating = media.average_score.map(|s| s as f64 / 10.0);

        // Get poster URL (prefer extra large)
        let poster_url = media
            .cover_image
            .as_ref()
            .and_then(|c| c.extra_large.clone().or_else(|| c.large.clone()));

        // Get main animation studio
        let studio = media
            .studios
            .as_ref()
            .and_then(|s| s.nodes.as_ref())
            .and_then(|nodes| {
                nodes
                    .iter()
                    .find(|s| s.is_animation_studio)
                    .or_else(|| nodes.first())
                    .map(|s| s.name.clone())
            });

        // Clean up description (remove HTML if any slipped through)
        let overview = media.description.as_ref().map(|d| {
            // Basic HTML tag removal
            let re = regex::Regex::new(r"<[^>]+>").unwrap();
            re.replace_all(d, "").trim().to_string()
        });

        // Extract cast from character edges (voice actors)
        let cast = self.extract_cast(media);

        AnimeMetadata {
            anilist_id: Some(media.id.to_string()),
            mal_id: media.id_mal.map(|id| id.to_string()),
            name,
            name_romaji: title.and_then(|t| t.romaji.clone()),
            name_native: title.and_then(|t| t.native.clone()),
            overview,
            year: media
                .season_year
                .or_else(|| media.start_date.as_ref().and_then(|d| d.year)),
            premiere_date,
            community_rating: rating,
            poster_url,
            backdrop_url: media.banner_image.clone(),
            episode_count: media.episodes,
            episode_duration_minutes: media.duration,
            genres: media.genres.clone(),
            studio,
            cast,
        }
    }

    /// Extract voice actors from character data
    fn extract_cast(&self, media: &MediaData) -> Vec<CastMember> {
        let mut cast = Vec::new();

        if let Some(ref characters) = media.characters {
            if let Some(ref edges) = characters.edges {
                for edge in edges {
                    // Get character name
                    let character_name = edge
                        .node
                        .as_ref()
                        .and_then(|c| c.name.as_ref())
                        .and_then(|n| n.full.clone());

                    // Get voice actors (prefer Japanese)
                    if let Some(ref voice_actors) = edge.voice_actors {
                        for va in voice_actors {
                            // Prefer Japanese voice actors
                            if va.language.as_deref() != Some("Japanese") {
                                continue;
                            }

                            let person_name = va
                                .name
                                .as_ref()
                                .and_then(|n| n.full.clone())
                                .unwrap_or_default();

                            if person_name.is_empty() {
                                continue;
                            }

                            let person_image_url = va
                                .image
                                .as_ref()
                                .and_then(|i| i.large.clone().or_else(|| i.medium.clone()));

                            cast.push(CastMember {
                                person_id: format!("anilist-staff-{}", va.id),
                                person_name,
                                person_image_url,
                                character_name: character_name.clone(),
                                role: "Voice Actor".to_string(),
                            });
                        }
                    }
                }
            }
        }

        cast
    }

    /// Get anime metadata by AniList ID (direct lookup, no search needed)
    pub async fn get_anime_by_id(&self, anilist_id: i64) -> Result<Option<AnimeMetadata>> {
        match self.get_anime_details(anilist_id).await? {
            Some(media) => Ok(Some(self.media_to_metadata(&media))),
            None => Ok(None),
        }
    }

    /// Download and cache an image, returns the local path
    pub async fn download_image(
        &self,
        url: &str,
        item_id: &str,
        image_type: &str,
    ) -> Result<PathBuf> {
        // Create cache directory structure
        let item_cache_dir = self.image_cache_dir.join(item_id);
        fs::create_dir_all(&item_cache_dir).await?;

        // Determine file extension from URL
        let ext = url
            .rsplit('.')
            .next()
            .filter(|e| e.len() <= 4)
            .unwrap_or("jpg");

        let local_filename = format!("{}.{}", image_type, ext);
        let local_path = item_cache_dir.join(&local_filename);

        // Skip if already cached
        if local_path.exists() {
            tracing::debug!("Image already cached: {:?}", local_path);
            return Ok(local_path);
        }

        // Download
        tracing::debug!("Downloading image: {}", url);

        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to download image from AniList")?;

        if !response.status().is_success() {
            anyhow::bail!("Image download failed with status: {}", response.status());
        }

        let bytes = response.bytes().await?;
        fs::write(&local_path, &bytes).await?;

        tracing::info!("Downloaded image to {:?}", local_path);
        Ok(local_path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_media_to_metadata() {
        let client = AniListClient::new(PathBuf::from("cache/images"));

        let media = MediaData {
            id: 12345,
            title: Some(TitleData {
                romaji: Some("Beck".to_string()),
                english: Some("Beck: Mongolian Chop Squad".to_string()),
                native: Some("ベック".to_string()),
            }),
            description: Some("A story about music".to_string()),
            start_date: Some(FuzzyDate {
                year: Some(2004),
                month: Some(10),
                day: Some(6),
            }),
            end_date: None,
            cover_image: Some(CoverImage {
                extra_large: Some("https://example.com/poster.jpg".to_string()),
                large: None,
                medium: None,
            }),
            banner_image: None,
            average_score: Some(82),
            episodes: Some(26),
            duration: Some(25),
            genres: Some(vec!["Music".to_string(), "Slice of Life".to_string()]),
            studios: None,
            characters: None,
            id_mal: Some(57),
            format: Some("TV".to_string()),
            status: Some("FINISHED".to_string()),
            season_year: Some(2004),
        };

        let metadata = client.media_to_metadata(&media);

        assert_eq!(
            metadata.name,
            Some("Beck: Mongolian Chop Squad".to_string())
        );
        assert_eq!(metadata.anilist_id, Some("12345".to_string()));
        assert_eq!(metadata.mal_id, Some("57".to_string()));
        assert_eq!(metadata.year, Some(2004));
        assert_eq!(metadata.community_rating, Some(8.2));
    }
}
