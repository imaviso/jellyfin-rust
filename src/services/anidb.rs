// AniDB metadata provider service
// API Documentation: https://wiki.anidb.net/HTTP_API_Definition
// Note: AniDB has strict rate limiting (1 request per 2 seconds)
// They also have a UDP API but HTTP is simpler for our needs

use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio::fs;
use tokio::sync::Mutex;

const ANIDB_API_BASE: &str = "http://api.anidb.net:9001/httpapi";
const ANIDB_IMAGE_BASE: &str = "https://cdn.anidb.net/images/main";
// AniDB requires a client identifier
const ANIDB_CLIENT: &str = "jellyfinrust";
const ANIDB_CLIENT_VER: i32 = 1;
// Rate limit: max 1 request per 2 seconds
const RATE_LIMIT_MS: u64 = 2000;

/// AniDB API client
pub struct AniDBClient {
    client: Client,
    image_cache_dir: PathBuf,
    last_request: Mutex<Option<Instant>>,
}

/// AniDB anime data from XML response
#[derive(Debug, Clone, Default)]
pub struct AniDBAnime {
    pub aid: i64,
    pub title: String,
    pub title_romaji: Option<String>,
    pub title_kanji: Option<String>,
    pub description: Option<String>,
    pub picture: Option<String>,
    pub start_date: Option<String>,
    pub end_date: Option<String>,
    pub episode_count: Option<i32>,
    pub rating: Option<f64>,
    pub rating_count: Option<i32>,
}

/// AniDB episode data
#[derive(Debug, Clone, Default)]
pub struct AniDBEpisode {
    pub eid: i64,
    pub epno: String,
    pub title: String,
    pub title_romaji: Option<String>,
    pub air_date: Option<String>,
    pub length: Option<i32>,
    pub rating: Option<f64>,
}

/// Metadata result compatible with our system
#[derive(Debug, Clone, Default)]
pub struct AniDBMetadata {
    pub anidb_id: Option<String>,
    pub name: Option<String>,
    pub name_romaji: Option<String>,
    pub name_native: Option<String>,
    pub overview: Option<String>,
    pub year: Option<i32>,
    pub premiere_date: Option<String>,
    pub community_rating: Option<f64>,
    pub poster_url: Option<String>,
    pub episode_count: Option<i32>,
    pub episodes: Vec<AniDBEpisode>,
}

impl AniDBClient {
    /// Create a new AniDB client
    pub fn new(image_cache_dir: PathBuf) -> Self {
        Self {
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| Client::new()),
            image_cache_dir,
            last_request: Mutex::new(None),
        }
    }

    /// Enforce rate limiting (1 request per 2 seconds)
    async fn rate_limit(&self) {
        let mut last = self.last_request.lock().await;
        if let Some(last_time) = *last {
            let elapsed = last_time.elapsed();
            if elapsed < Duration::from_millis(RATE_LIMIT_MS) {
                let wait = Duration::from_millis(RATE_LIMIT_MS) - elapsed;
                tracing::debug!("AniDB rate limit: waiting {:?}", wait);
                tokio::time::sleep(wait).await;
            }
        }
        *last = Some(Instant::now());
    }

    /// Get anime details by AniDB ID
    /// Note: AniDB doesn't have a search API via HTTP, only by ID
    /// You typically need to use their title dump or UDP API for search
    pub async fn get_anime_by_id(&self, aid: i64) -> Result<Option<AniDBMetadata>> {
        self.rate_limit().await;

        let url = format!(
            "{}?request=anime&client={}&clientver={}&protover=1&aid={}",
            ANIDB_API_BASE, ANIDB_CLIENT, ANIDB_CLIENT_VER, aid
        );

        tracing::debug!("Fetching AniDB anime: {}", aid);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to fetch from AniDB")?;

        if !response.status().is_success() {
            tracing::warn!("AniDB request failed with status: {}", response.status());
            return Ok(None);
        }

        let xml = response.text().await?;

        // Check for error responses
        if xml.contains("<error>") {
            tracing::warn!("AniDB returned error: {}", xml);
            return Ok(None);
        }

        // Parse the XML response
        self.parse_anime_xml(&xml, aid)
    }

    /// Parse AniDB XML response
    fn parse_anime_xml(&self, xml: &str, aid: i64) -> Result<Option<AniDBMetadata>> {
        // Simple XML parsing - AniDB returns relatively simple XML
        // In production, you'd want to use quick-xml or roxmltree

        let mut metadata = AniDBMetadata {
            anidb_id: Some(aid.to_string()),
            ..Default::default()
        };

        // Extract title (main title)
        if let Some(title) = extract_xml_value(xml, "title", Some("type=\"main\"")) {
            metadata.name = Some(title);
        } else if let Some(title) = extract_xml_value(xml, "title", Some("type=\"official\"")) {
            metadata.name = Some(title);
        }

        // Extract romaji title
        if let Some(title) =
            extract_xml_value(xml, "title", Some("type=\"official\" xml:lang=\"x-jat\""))
        {
            metadata.name_romaji = Some(title);
        }

        // Extract kanji title
        if let Some(title) =
            extract_xml_value(xml, "title", Some("type=\"official\" xml:lang=\"ja\""))
        {
            metadata.name_native = Some(title);
        }

        // Extract description
        if let Some(desc) = extract_xml_content(xml, "description") {
            metadata.overview = Some(desc);
        }

        // Extract picture
        if let Some(pic) = extract_xml_content(xml, "picture") {
            metadata.poster_url = Some(format!("{}/{}", ANIDB_IMAGE_BASE, pic));
        }

        // Extract start date
        if let Some(date) = extract_xml_content(xml, "startdate") {
            metadata.premiere_date = Some(date.clone());
            // Extract year from date
            if let Some(year_str) = date.split('-').next() {
                metadata.year = year_str.parse().ok();
            }
        }

        // Extract episode count
        if let Some(count) = extract_xml_content(xml, "episodecount") {
            metadata.episode_count = count.parse().ok();
        }

        // Extract rating
        if let Some(rating_str) = extract_xml_content(xml, "rating") {
            if let Ok(rating) = rating_str.parse::<f64>() {
                metadata.community_rating = Some(rating);
            }
        }

        // Extract episodes
        metadata.episodes = self.parse_episodes_xml(xml);

        if metadata.name.is_some() {
            Ok(Some(metadata))
        } else {
            Ok(None)
        }
    }

    /// Parse episodes from XML
    fn parse_episodes_xml(&self, xml: &str) -> Vec<AniDBEpisode> {
        let mut episodes = Vec::new();

        // Find the episodes section
        if let Some(eps_start) = xml.find("<episodes>") {
            if let Some(eps_end) = xml[eps_start..].find("</episodes>") {
                let eps_xml = &xml[eps_start..eps_start + eps_end];

                // Parse individual episodes
                let mut pos = 0;
                while let Some(ep_start) = eps_xml[pos..].find("<episode ") {
                    let ep_start = pos + ep_start;
                    if let Some(ep_end) = eps_xml[ep_start..].find("</episode>") {
                        let ep_xml = &eps_xml[ep_start..ep_start + ep_end + 10];

                        let mut episode = AniDBEpisode::default();

                        // Extract episode ID
                        if let Some(eid) = extract_xml_attr(ep_xml, "episode", "id") {
                            episode.eid = eid.parse().unwrap_or(0);
                        }

                        // Extract episode number
                        if let Some(epno) = extract_xml_content(ep_xml, "epno") {
                            episode.epno = epno;
                        }

                        // Extract title
                        if let Some(title) =
                            extract_xml_value(ep_xml, "title", Some("xml:lang=\"en\""))
                        {
                            episode.title = title;
                        } else if let Some(title) = extract_xml_content(ep_xml, "title") {
                            episode.title = title;
                        }

                        // Extract air date
                        if let Some(date) = extract_xml_content(ep_xml, "airdate") {
                            episode.air_date = Some(date);
                        }

                        // Extract length (minutes)
                        if let Some(len) = extract_xml_content(ep_xml, "length") {
                            episode.length = len.parse().ok();
                        }

                        if !episode.epno.is_empty() {
                            episodes.push(episode);
                        }

                        pos = ep_start + ep_end + 10;
                    } else {
                        break;
                    }
                }
            }
        }

        episodes
    }

    /// Download and cache an image
    pub async fn download_image(
        &self,
        url: &str,
        item_id: &str,
        image_type: &str,
    ) -> Result<PathBuf> {
        let item_cache_dir = self.image_cache_dir.join(item_id);
        fs::create_dir_all(&item_cache_dir).await?;

        let ext = url.rsplit('.').next().unwrap_or("jpg");
        let local_filename = format!("{}.{}", image_type, ext);
        let local_path = item_cache_dir.join(&local_filename);

        // Skip if already cached (use async check to avoid blocking)
        if fs::try_exists(&local_path).await.unwrap_or(false) {
            return Ok(local_path);
        }

        // No rate limiting for image downloads (different server)
        let response = self
            .client
            .get(url)
            .send()
            .await
            .context("Failed to download image from AniDB")?;

        if !response.status().is_success() {
            anyhow::bail!("Image download failed: {}", response.status());
        }

        let bytes = response.bytes().await?;
        fs::write(&local_path, &bytes).await?;

        tracing::info!("Downloaded AniDB image to {:?}", local_path);
        Ok(local_path)
    }
}

/// Helper to extract XML content between tags
fn extract_xml_content(xml: &str, tag: &str) -> Option<String> {
    let start_tag = format!("<{}", tag);
    let end_tag = format!("</{}>", tag);

    if let Some(start) = xml.find(&start_tag) {
        // Find the end of the opening tag
        if let Some(tag_end) = xml[start..].find('>') {
            let content_start = start + tag_end + 1;
            if let Some(end) = xml[content_start..].find(&end_tag) {
                let content = &xml[content_start..content_start + end];
                return Some(html_decode(content.trim()));
            }
        }
    }
    None
}

/// Helper to extract XML attribute value
fn extract_xml_attr(xml: &str, tag: &str, attr: &str) -> Option<String> {
    let start_tag = format!("<{}", tag);

    if let Some(start) = xml.find(&start_tag) {
        if let Some(tag_end) = xml[start..].find('>') {
            let tag_content = &xml[start..start + tag_end];
            let attr_pattern = format!("{}=\"", attr);
            if let Some(attr_start) = tag_content.find(&attr_pattern) {
                let value_start = attr_start + attr_pattern.len();
                if let Some(value_end) = tag_content[value_start..].find('"') {
                    return Some(tag_content[value_start..value_start + value_end].to_string());
                }
            }
        }
    }
    None
}

/// Helper to extract XML value with specific attributes
fn extract_xml_value(xml: &str, tag: &str, attrs: Option<&str>) -> Option<String> {
    let pattern = if let Some(a) = attrs {
        format!("<{} {}", tag, a)
    } else {
        format!("<{}>", tag)
    };

    if let Some(start) = xml.find(&pattern) {
        if let Some(tag_end) = xml[start..].find('>') {
            let content_start = start + tag_end + 1;
            let end_tag = format!("</{}>", tag);
            if let Some(end) = xml[content_start..].find(&end_tag) {
                let content = &xml[content_start..content_start + end];
                return Some(html_decode(content.trim()));
            }
        }
    }
    None
}

/// Basic HTML entity decoding
fn html_decode(s: &str) -> String {
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&apos;", "'")
        .replace("<br />", "\n")
        .replace("<br/>", "\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_xml_content() {
        let xml = "<anime><title>Test Show</title><rating>8.5</rating></anime>";
        assert_eq!(
            extract_xml_content(xml, "title"),
            Some("Test Show".to_string())
        );
        assert_eq!(extract_xml_content(xml, "rating"), Some("8.5".to_string()));
    }

    #[test]
    fn test_html_decode() {
        assert_eq!(html_decode("Tom &amp; Jerry"), "Tom & Jerry");
        assert_eq!(html_decode("a &lt; b"), "a < b");
    }
}
