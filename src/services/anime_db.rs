// Anime Offline Database service
// Uses the manami-project/anime-offline-database for title → ID mapping
// This allows us to search by title locally and get cross-referenced IDs
// for AniList, AniDB, MAL, Kitsu, etc.
//
// Database URL: https://github.com/manami-project/anime-offline-database/releases/latest/download/anime-offline-database-minified.json

use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::PathBuf;
use tokio::fs;
use tokio::sync::RwLock;

const DATABASE_URL: &str = "https://github.com/manami-project/anime-offline-database/releases/latest/download/anime-offline-database-minified.json";
const DATABASE_FILENAME: &str = "anime-offline-database.json";
// Re-download if older than 7 days
const MAX_AGE_SECS: u64 = 7 * 24 * 60 * 60;

#[derive(Debug, Clone, Deserialize)]
pub struct AnimeEntry {
    pub sources: Vec<String>,
    pub title: String,
    #[serde(rename = "type")]
    pub anime_type: String,
    pub episodes: i32,
    pub status: String,
    #[serde(rename = "animeSeason")]
    pub anime_season: Option<AnimeSeason>,
    pub picture: Option<String>,
    pub thumbnail: Option<String>,
    pub synonyms: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AnimeSeason {
    pub season: Option<String>,
    pub year: Option<i32>,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderIds {
    pub anilist_id: Option<i64>,
    pub anidb_id: Option<i64>,
    pub mal_id: Option<i64>,
    pub kitsu_id: Option<i64>,
}

impl AnimeEntry {
    pub fn provider_ids(&self) -> ProviderIds {
        let mut ids = ProviderIds::default();

        for source in &self.sources {
            if let Some(id) = extract_id_from_url(source, "anilist.co/anime/") {
                ids.anilist_id = Some(id);
            } else if let Some(id) = extract_id_from_url(source, "anidb.net/anime/") {
                ids.anidb_id = Some(id);
            } else if let Some(id) = extract_id_from_url(source, "myanimelist.net/anime/") {
                ids.mal_id = Some(id);
            } else if let Some(id) = extract_id_from_url(source, "kitsu.app/anime/") {
                ids.kitsu_id = Some(id);
            }
        }

        ids
    }

    pub fn year(&self) -> Option<i32> {
        self.anime_season.as_ref().and_then(|s| s.year)
    }
}

fn extract_id_from_url(url: &str, pattern: &str) -> Option<i64> {
    if let Some(pos) = url.find(pattern) {
        let after = &url[pos + pattern.len()..];
        // Take digits until we hit a non-digit
        let id_str: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        id_str.parse().ok()
    } else {
        None
    }
}

#[derive(Debug, Deserialize)]
struct DatabaseRoot {
    data: Vec<AnimeEntry>,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub entry: AnimeEntry,
    pub score: f64,
}

pub struct AnimeOfflineDatabase {
    cache_dir: PathBuf,
    enabled: bool,
    /// The loaded database (lazy loaded)
    database: RwLock<Option<Vec<AnimeEntry>>>,
    /// Title index for fast lookups (lowercase title -> indices)
    title_index: RwLock<HashMap<String, Vec<usize>>>,
}

impl AnimeOfflineDatabase {
    pub fn new(cache_dir: PathBuf, enabled: Option<bool>) -> Self {
        let enabled = enabled.unwrap_or_else(|| {
            std::env::var("ENABLE_ANIME_DB")
                .map(|v| v.eq_ignore_ascii_case("true") || v == "1")
                .unwrap_or(false)
        });

        if enabled {
            tracing::info!("Anime offline database enabled, cache dir: {:?}", cache_dir);
        }

        Self {
            cache_dir,
            enabled,
            database: RwLock::new(None),
            title_index: RwLock::new(HashMap::new()),
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Unload the database from memory to free RAM
    /// Can be called after scanning is complete - will be reloaded on next search
    pub async fn unload(&self) {
        let mut db = self.database.write().await;
        if db.is_some() {
            *db = None;
            self.title_index.write().await.clear();
            tracing::info!("Anime offline database unloaded from memory");
        }
    }

    /// Preload the database (download if needed) - call this before scanning
    pub async fn preload(&self) -> Result<()> {
        if !self.enabled {
            tracing::debug!("Anime offline database is disabled, skipping preload");
            return Ok(());
        }

        tracing::info!("Preloading anime offline database...");
        self.ensure_loaded().await
    }

    async fn ensure_loaded(&self) -> Result<()> {
        if !self.enabled {
            anyhow::bail!(
                "Anime offline database is disabled. Set ENABLE_ANIME_DB=true to enable."
            );
        }

        {
            let db = self.database.read().await;
            if db.is_some() {
                return Ok(());
            }
        }

        let mut db = self.database.write().await;
        if db.is_some() {
            return Ok(());
        }

        let entries = self.load_or_download().await?;

        let mut index = HashMap::new();
        for (i, entry) in entries.iter().enumerate() {
            let title_lower = entry.title.to_lowercase();
            index.entry(title_lower).or_insert_with(Vec::new).push(i);

            for syn in &entry.synonyms {
                let syn_lower = syn.to_lowercase();
                index.entry(syn_lower).or_insert_with(Vec::new).push(i);
            }
        }

        *self.title_index.write().await = index;
        *db = Some(entries);

        tracing::info!("Anime offline database loaded and indexed");
        Ok(())
    }

    async fn load_or_download(&self) -> Result<Vec<AnimeEntry>> {
        fs::create_dir_all(&self.cache_dir).await?;
        let db_path = self.cache_dir.join(DATABASE_FILENAME);

        // Use async check to avoid blocking
        let db_exists = fs::try_exists(&db_path).await.unwrap_or(false);
        let should_download = if db_exists {
            match fs::metadata(&db_path).await {
                Ok(meta) => {
                    if let Ok(modified) = meta.modified() {
                        let age = modified.elapsed().unwrap_or_default();
                        age.as_secs() > MAX_AGE_SECS
                    } else {
                        true
                    }
                }
                Err(_) => true,
            }
        } else {
            true
        };

        if should_download {
            tracing::info!("Downloading anime offline database...");
            match self.download_database(&db_path).await {
                Ok(entries) => return Ok(entries),
                Err(e) => {
                    tracing::warn!("Failed to download database: {}", e);
                    // Re-check existence asynchronously
                    if fs::try_exists(&db_path).await.unwrap_or(false) {
                        tracing::info!("Using cached database instead");
                    } else {
                        return Err(e);
                    }
                }
            }
        }

        tracing::info!("Loading anime offline database from cache...");
        let content = fs::read_to_string(&db_path).await?;
        let root: DatabaseRoot =
            serde_json::from_str(&content).context("Failed to parse anime offline database")?;

        tracing::info!("Loaded {} anime entries from cache", root.data.len());
        Ok(root.data)
    }

    async fn download_database(&self, save_path: &PathBuf) -> Result<Vec<AnimeEntry>> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(120))
            .build()?;

        let response = client
            .get(DATABASE_URL)
            .send()
            .await
            .context("Failed to download anime offline database")?;

        if !response.status().is_success() {
            anyhow::bail!("Download failed with status: {}", response.status());
        }

        let content = response.text().await?;

        fs::write(save_path, &content).await?;

        let root: DatabaseRoot =
            serde_json::from_str(&content).context("Failed to parse anime offline database")?;

        tracing::info!("Downloaded {} anime entries", root.data.len());
        Ok(root.data)
    }

    pub async fn search(&self, query: &str, year: Option<i32>) -> Result<Vec<SearchResult>> {
        self.ensure_loaded().await?;

        let query_owned = query.to_string();

        let db = self.database.read().await;
        let entries = db.as_ref().unwrap().clone();
        let index = self.title_index.read().await.clone();
        drop(db);

        let results = tokio::task::spawn_blocking(move || {
            search_entries_sync(&query_owned, year, &entries, &index)
        })
        .await
        .context("Search task panicked")?;

        Ok(results)
    }

    pub async fn find_by_anilist_id(&self, anilist_id: i64) -> Result<Option<AnimeEntry>> {
        self.ensure_loaded().await?;

        let db = self.database.read().await;
        let entries = db.as_ref().unwrap();

        let pattern = format!("anilist.co/anime/{}", anilist_id);
        for entry in entries {
            if entry.sources.iter().any(|s| s.contains(&pattern)) {
                return Ok(Some(entry.clone()));
            }
        }

        Ok(None)
    }

    pub async fn find_by_anidb_id(&self, anidb_id: i64) -> Result<Option<AnimeEntry>> {
        self.ensure_loaded().await?;

        let db = self.database.read().await;
        let entries = db.as_ref().unwrap();

        let pattern = format!("anidb.net/anime/{}", anidb_id);
        for entry in entries {
            if entry.sources.iter().any(|s| s.contains(&pattern)) {
                return Ok(Some(entry.clone()));
            }
        }

        Ok(None)
    }

    pub async fn find_by_mal_id(&self, mal_id: i64) -> Result<Option<AnimeEntry>> {
        self.ensure_loaded().await?;

        let db = self.database.read().await;
        let entries = db.as_ref().unwrap();

        let pattern = format!("myanimelist.net/anime/{}", mal_id);
        for entry in entries {
            if entry.sources.iter().any(|s| s.contains(&pattern)) {
                return Ok(Some(entry.clone()));
            }
        }

        Ok(None)
    }
}

/// Synchronous search implementation for spawn_blocking
/// This runs the CPU-intensive fuzzy search without blocking the async runtime
fn search_entries_sync(
    query: &str,
    year: Option<i32>,
    entries: &[AnimeEntry],
    index: &HashMap<String, Vec<usize>>,
) -> Vec<SearchResult> {
    let query_lower = query.to_lowercase();
    let query_words: Vec<&str> = query_lower.split_whitespace().collect();

    let mut results: Vec<SearchResult> = Vec::new();
    let mut seen_indices: std::collections::HashSet<usize> = std::collections::HashSet::new();

    const MIN_SCORE_THRESHOLD: f64 = 60.0;

    if let Some(indices) = index.get(&query_lower) {
        for &idx in indices {
            if idx < entries.len() {
                seen_indices.insert(idx);
                let entry = &entries[idx];
                let score = calculate_match_score(&query_lower, &query_words, entry, year);
                if score >= MIN_SCORE_THRESHOLD {
                    results.push(SearchResult {
                        entry: entry.clone(),
                        score,
                    });
                }
            }
        }
    }

    for (key, indices) in index.iter() {
        if key.contains(&query_lower) || query_lower.contains(key.as_str()) {
            for &idx in indices {
                if idx < entries.len() && !seen_indices.contains(&idx) {
                    seen_indices.insert(idx);
                    let entry = &entries[idx];
                    let score = calculate_match_score(&query_lower, &query_words, entry, year);
                    if score >= MIN_SCORE_THRESHOLD {
                        results.push(SearchResult {
                            entry: entry.clone(),
                            score,
                        });
                    }
                }
            }
        }
    }

    if results.len() < 5 {
        for (idx, entry) in entries.iter().enumerate() {
            if seen_indices.contains(&idx) {
                continue;
            }
            let score = calculate_match_score(&query_lower, &query_words, entry, year);
            if score >= MIN_SCORE_THRESHOLD {
                results.push(SearchResult {
                    entry: entry.clone(),
                    score,
                });
            }
        }
    }

    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    results.truncate(10);
    results
}

fn calculate_match_score(
    query_lower: &str,
    query_words: &[&str],
    entry: &AnimeEntry,
    year: Option<i32>,
) -> f64 {
    let mut best_score: f64 = 0.0;
    let mut matched_title: Option<&str> = None;

    let title_lower = entry.title.to_lowercase();
    let title_score = string_similarity(query_lower, &title_lower);
    if title_score > best_score {
        best_score = title_score;
        matched_title = Some(&entry.title);
    }

    for syn in &entry.synonyms {
        let syn_lower = syn.to_lowercase();
        let syn_score = string_similarity(query_lower, &syn_lower);
        if syn_score > best_score {
            best_score = syn_score;
            matched_title = Some(syn);
        }
    }

    if let Some(title) = matched_title {
        let title_lower = title.to_lowercase();
        let title_words: Vec<&str> = title_lower.split_whitespace().collect();
        let word_matches = query_words
            .iter()
            .filter(|qw| {
                title_words
                    .iter()
                    .any(|tw| tw.contains(*qw) || qw.contains(tw))
            })
            .count();

        if word_matches == query_words.len() && !query_words.is_empty() {
            best_score += 20.0;
        }
    }

    if let Some(y) = year {
        if entry.year() == Some(y) {
            best_score += 15.0;
        } else if let Some(entry_year) = entry.year() {
            let diff = (entry_year - y).abs();
            if diff <= 1 {
                best_score += 10.0;
            } else if diff <= 2 {
                best_score += 5.0;
            }
        }
    } else {
        if let Some(_entry_year) = entry.year() {
            best_score += 2.0;
        }
    }

    if query_lower.len() <= 3 && matched_title.map(|t| t.len()).unwrap_or(0) > 10 {
        if best_score < 95.0 {
            best_score *= 0.3;
        }
    }

    best_score
}

fn string_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 100.0;
    }

    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    if a == b {
        return 100.0;
    }

    if b.starts_with(a) {
        let ratio = a.len() as f64 / b.len() as f64;
        return 90.0 + ratio * 10.0;
    }
    if a.starts_with(b) {
        let ratio = b.len() as f64 / a.len() as f64;
        return 75.0 + ratio * 15.0;
    }

    if b.contains(a) {
        let ratio = a.len() as f64 / b.len() as f64;
        if ratio > 0.5 {
            return 80.0 + ratio * 15.0;
        } else if ratio > 0.3 {
            return 60.0 + ratio * 20.0;
        } else {
            return 30.0 + ratio * 30.0;
        }
    }

    if a.contains(b) {
        let ratio = b.len() as f64 / a.len() as f64;
        if ratio > 0.5 {
            return 70.0 + ratio * 15.0;
        } else {
            return 40.0 + ratio * 20.0;
        }
    }

    let a_words: Vec<&str> = a.split_whitespace().collect();
    let b_words: Vec<&str> = b.split_whitespace().collect();

    if !a_words.is_empty() && !b_words.is_empty() {
        let matching_words = a_words
            .iter()
            .filter(|aw| {
                b_words.iter().any(|bw| {
                    *aw == bw
                        || (aw.len() >= 3 && bw.contains(*aw))
                        || (bw.len() >= 3 && aw.contains(bw))
                })
            })
            .count();

        let word_ratio = matching_words as f64 / a_words.len().max(1) as f64;
        if word_ratio > 0.8 {
            return 70.0 + word_ratio * 20.0;
        } else if word_ratio > 0.5 {
            return 50.0 + word_ratio * 30.0;
        }
    }

    if a.len() < 50 && b.len() < 50 {
        let distance = simple_edit_distance(a, b);
        let max_len = a.len().max(b.len()) as f64;
        let similarity = 1.0 - (distance as f64 / max_len);
        return similarity * 50.0;
    }

    0.0
}

fn simple_edit_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_id_from_url() {
        assert_eq!(
            extract_id_from_url("https://anilist.co/anime/1535", "anilist.co/anime/"),
            Some(1535)
        );
        assert_eq!(
            extract_id_from_url("https://anidb.net/anime/4563", "anidb.net/anime/"),
            Some(4563)
        );
        assert_eq!(
            extract_id_from_url(
                "https://myanimelist.net/anime/1535",
                "myanimelist.net/anime/"
            ),
            Some(1535)
        );
        assert_eq!(
            extract_id_from_url("https://kitsu.app/anime/1376", "kitsu.app/anime/"),
            Some(1376)
        );
    }

    #[test]
    fn test_string_similarity() {
        assert_eq!(string_similarity("beck", "beck"), 100.0);
        assert!(string_similarity("beck", "beck mongolian chop squad") > 70.0);
        assert!(string_similarity("death note", "death note") == 100.0);
    }
}
