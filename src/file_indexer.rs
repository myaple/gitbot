use anyhow::Result;
use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use std::collections::{hash_map::DefaultHasher, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

use crate::gitlab::GitlabApiClient;
use crate::models::GitlabProject;
use crate::repo_context::GitlabFile;

/// The size of n-grams to use for indexing
const NGRAM_SIZE: usize = 3;

/// Maximum number of files to index per project
const MAX_FILES_TO_INDEX: usize = 1000;

/// Maximum file size to index (in bytes)
const MAX_FILE_SIZE: usize = 100_000;

/// File extensions to index
const INDEXABLE_EXTENSIONS: [&str; 22] = [
    "rs", "py", "js", "ts", "java", "c", "cpp", "h", "hpp", "go", "rb", "php", "cs", "scala", "kt",
    "swift", "sh", "jsx", "tsx", "vue", "svelte", "md",
];

/// Represents an index of file content using n-grams
#[derive(Debug, Clone)]
pub struct FileContentIndex {
    /// Maps n-grams to file IDs that contain them
    ngram_to_files: Arc<DashMap<String, HashSet<u32>>>,
    /// Maps file paths to their file IDs
    path_to_file_id: Arc<DashMap<String, u32>>,
    /// Maps file IDs to their file paths
    file_id_to_path: Arc<DashMap<u32, String>>,
    /// Maps file IDs to their last indexed content hash
    file_hashes: Arc<DashMap<u32, u64>>,
    /// Next available file ID
    next_file_id: Arc<AtomicU32>,
    /// When the index was last updated
    last_updated: Arc<RwLock<Instant>>,
}

impl FileContentIndex {
    /// Create a new empty file content index
    pub fn new(_project_id: i64) -> Self {
        Self {
            ngram_to_files: Arc::new(DashMap::new()),
            path_to_file_id: Arc::new(DashMap::new()),
            file_id_to_path: Arc::new(DashMap::new()),
            file_hashes: Arc::new(DashMap::new()),
            next_file_id: Arc::new(AtomicU32::new(1)),
            last_updated: Arc::new(RwLock::new(Instant::now())),
        }
    }

    /// Check if a file should be indexed based on its extension
    pub fn should_index_file(file_path: &str) -> bool {
        let extension = file_path.split('.').next_back().unwrap_or("");
        INDEXABLE_EXTENSIONS.contains(&extension)
    }

    /// Generate n-grams from text
    pub fn generate_ngrams(text: &str) -> HashSet<String> {
        let normalized_text = text.to_lowercase();
        let chars: Vec<char> = normalized_text.chars().collect();
        let mut ngrams = HashSet::new();

        if chars.len() < NGRAM_SIZE {
            // If text is shorter than n-gram size, just add the whole text
            ngrams.insert(normalized_text);
            return ngrams;
        }

        for i in 0..=chars.len() - NGRAM_SIZE {
            let ngram: String = chars[i..i + NGRAM_SIZE].iter().collect();
            ngrams.insert(ngram);
        }

        ngrams
    }

    /// Calculate a simple hash of file content for change detection
    pub fn calculate_content_hash(content: &str) -> u64 {
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Add a file to the index
    pub fn add_file(&self, file_path: &str, content: &str) {
        if !Self::should_index_file(file_path) {
            return;
        }

        let content_hash = Self::calculate_content_hash(content);

        // Get or create file ID for this path
        let file_id = *self
            .path_to_file_id
            .entry(file_path.to_string())
            .or_insert_with(|| self.next_file_id.fetch_add(1, Ordering::Relaxed));

        // Ensure reverse mapping exists
        self.file_id_to_path
            .entry(file_id)
            .or_insert_with(|| file_path.to_string());

        // Check if we've already indexed this file with the same content
        if let Some(existing_hash) = self.file_hashes.get(&file_id) {
            if *existing_hash == content_hash {
                // Content hasn't changed, no need to reindex
                return;
            }
        }

        // Generate n-grams from the file content
        let ngrams = Self::generate_ngrams(content);

        // Add each n-gram to the index
        for ngram in ngrams {
            self.ngram_to_files
                .entry(ngram)
                .or_default()
                .insert(file_id);
        }

        // Update the file hash
        self.file_hashes.insert(file_id, content_hash);
    }

    /// Remove a file from the index
    /// This method is primarily used for testing and cleanup operations.
    #[cfg(test)]
    pub fn remove_file(&self, file_path: &str) {
        // Find file ID
        if let Some((_, file_id)) = self.path_to_file_id.remove(file_path) {
            // Remove from reverse mapping
            self.file_id_to_path.remove(&file_id);

            // Remove file from file_hashes
            self.file_hashes.remove(&file_id);

            // Remove file from all n-gram entries
            for mut entry in self.ngram_to_files.iter_mut() {
                entry.value_mut().remove(&file_id);
            }

            // Clean up empty n-gram entries
            self.ngram_to_files.retain(|_, files| !files.is_empty());
        }
    }

    /// Search for files containing all the given keywords
    pub fn search(&self, keywords: &[String]) -> Vec<String> {
        if keywords.is_empty() {
            return Vec::new();
        }

        // Generate n-grams for each keyword
        let keyword_ngrams: Vec<HashSet<String>> = keywords
            .iter()
            .map(|keyword| Self::generate_ngrams(keyword))
            .collect();

        // For each keyword, find file IDs that contain any of its n-grams
        let mut keyword_matches: Vec<HashSet<u32>> = Vec::new();

        for (i, ngrams) in keyword_ngrams.iter().enumerate() {
            let mut file_ids_for_keyword = HashSet::new();
            let keyword = &keywords[i].to_lowercase();

            // Special handling for short keywords (less than NGRAM_SIZE)
            if keyword.len() < NGRAM_SIZE {
                // For short keywords, we need to check if any file contains this keyword
                // by looking at all n-grams that might contain it
                for item in self.ngram_to_files.iter() {
                    let ngram = item.key();
                    if ngram.contains(keyword) {
                        file_ids_for_keyword.extend(item.value().iter().cloned());
                    }
                }
            } else {
                // Normal case: look for exact n-gram matches
                for ngram in ngrams {
                    if let Some(files) = self.ngram_to_files.get(ngram) {
                        file_ids_for_keyword.extend(files.iter().cloned());
                    }
                }
            }

            if !file_ids_for_keyword.is_empty() {
                keyword_matches.push(file_ids_for_keyword);
            }
        }

        // If no matches found for any keyword, return empty result
        if keyword_matches.is_empty() {
            return Vec::new();
        }

        // Find file IDs that match all keywords (intersection of all sets)
        let mut result_ids = keyword_matches[0].clone();
        for file_ids in &keyword_matches[1..] {
            result_ids = result_ids.intersection(file_ids).cloned().collect();
        }

        // Convert IDs to file paths
        result_ids
            .into_iter()
            .filter_map(|id| self.file_id_to_path.get(&id).map(|p| p.value().clone()))
            .collect()
    }

    /// Get the time since the index was last updated
    pub async fn time_since_update(&self) -> Duration {
        let last_updated = *self.last_updated.read().await;
        last_updated.elapsed()
    }

    /// Update the last updated timestamp
    pub async fn mark_updated(&self) {
        let mut last_updated = self.last_updated.write().await;
        *last_updated = Instant::now();
    }
}

/// Manages file content indexes for multiple projects
pub struct FileIndexManager {
    /// Maps project IDs to their file content indexes
    indexes: Arc<DashMap<i64, FileContentIndex>>,
    /// GitLab API client for fetching file content
    gitlab_client: Arc<GitlabApiClient>,
    /// Refresh interval in seconds
    refresh_interval: u64,
}

impl FileIndexManager {
    /// Create a new file index manager
    pub fn new(gitlab_client: Arc<GitlabApiClient>, refresh_interval: u64) -> Self {
        Self {
            indexes: Arc::new(DashMap::new()),
            gitlab_client,
            refresh_interval,
        }
    }

    /// Get or create an index for a project
    pub fn get_or_create_index(&self, project_id: i64) -> FileContentIndex {
        self.indexes
            .entry(project_id)
            .or_insert_with(|| FileContentIndex::new(project_id))
            .clone()
    }

    /// Build or refresh the index for a project
    pub async fn build_index(&self, project: &GitlabProject) -> Result<()> {
        let project_id = project.id;
        info!("Building index for project {}", project.path_with_namespace);

        // Get or create the index
        let index = self.get_or_create_index(project_id);

        // Get all files in the repository
        let files = match self.gitlab_client.get_repository_tree(project_id).await {
            Ok(files) => files,
            Err(e) => {
                error!("Failed to get repository tree: {}", e);
                return Err(e.into());
            }
        };

        // Filter files to only include those we want to index
        let files_to_index: Vec<String> = files
            .into_iter()
            .filter(|path| FileContentIndex::should_index_file(path))
            .take(MAX_FILES_TO_INDEX)
            .collect();

        debug!(
            "Found {} indexable files in project {}",
            files_to_index.len(),
            project.path_with_namespace
        );

        // Process files in parallel with a limit on concurrency
        let (mut indexed_count, mut error_count) = (0, 0);

        // Create a stream of files with limited concurrency
        let results = stream::iter(files_to_index)
            .map(|file_path| {
                let client = self.gitlab_client.clone();
                let index = index.clone();

                async move {
                    match client.get_file_content(project_id, &file_path, None).await {
                        Ok(file) => {
                            if file.size <= MAX_FILE_SIZE {
                                if let Some(content) = file.content {
                                    index.add_file(&file_path, &content);
                                    return Ok(file_path);
                                }
                            }
                            Err(format!("File too large or no content: {file_path}"))
                        }
                        Err(e) => Err(format!("Failed to get content: {e}")),
                    }
                }
            })
            .buffer_unordered(10) // Process up to 10 files concurrently
            .collect::<Vec<_>>()
            .await;

        // Process results
        for result in results {
            match result {
                Ok(_) => {
                    indexed_count += 1;
                    if indexed_count % 100 == 0 {
                        debug!("Indexed {} files so far", indexed_count);
                    }
                }
                Err(e) => {
                    error_count += 1;
                    debug!("Error indexing file: {}", e);
                }
            }
        }

        // Update the last updated timestamp
        index.mark_updated().await;

        info!(
            "Finished indexing for project {}: {} files indexed, {} errors",
            project.path_with_namespace, indexed_count, error_count
        );

        Ok(())
    }

    /// Start a background task to periodically refresh indexes
    pub fn start_refresh_task(self: Arc<Self>, projects: Vec<GitlabProject>) {
        tokio::spawn(async move {
            let mut interval = interval(Duration::from_secs(self.refresh_interval));

            loop {
                interval.tick().await;
                info!("Starting scheduled index refresh");

                for project in &projects {
                    if let Err(e) = self.build_index(project).await {
                        warn!(
                            "Failed to refresh index for {}: {}",
                            project.path_with_namespace, e
                        );
                    }
                }
            }
        });
    }

    /// Search for files containing all the given keywords in a project
    pub async fn search_files(
        &self,
        project_id: i64,
        keywords: &[String],
    ) -> Result<Vec<GitlabFile>> {
        let index = self.get_or_create_index(project_id);

        // Check if index needs to be built
        if index.time_since_update().await > Duration::from_secs(self.refresh_interval * 2) {
            warn!(
                "Index for project {} is stale, results may be incomplete",
                project_id
            );
        }

        // Search the index
        let matching_files = index.search(keywords);

        if matching_files.is_empty() {
            return Ok(Vec::new());
        }

        // Fetch content for matching files and calculate relevance scores
        let mut files_with_content = Vec::new();

        // Limit the number of files to fetch and run requests concurrently
        let files_to_fetch: Vec<_> = matching_files.iter().take(5).collect();

        let fetch_futures = files_to_fetch.iter().map(|file_path| {
            let client = self.gitlab_client.clone();
            async move {
                client.get_file_content(project_id, file_path, None).await
            }
        });

        let results = futures::future::join_all(fetch_futures).await;

        for (result, file_path) in results.into_iter().zip(files_to_fetch) {
            match result {
                Ok(mut file) => {
                    // Calculate relevance score based on content
                    let content_score = if let Some(content) = &file.content {
                        calculate_content_keyword_frequency(content, keywords)
                    } else {
                        0
                    };

                    // Add base score for being found by the index
                    let total_score = content_score + 10;
                    file.relevance_score = Some(total_score);

                    files_with_content.push(file);
                }
                Err(e) => warn!("Failed to get content for file {}: {}", file_path, e),
            }
        }

        // Sort by relevance score (highest first)
        files_with_content.sort_by(|a, b| {
            b.relevance_score
                .unwrap_or(0)
                .cmp(&a.relevance_score.unwrap_or(0))
        });

        Ok(files_with_content)
    }
}

/// Calculate keyword frequency in content for relevance scoring
fn calculate_content_keyword_frequency(content: &str, keywords: &[String]) -> usize {
    if keywords.is_empty() || content.is_empty() {
        return 0;
    }

    let content_lower = content.to_lowercase();
    let mut total_hits = 0;

    // Count occurrences of each keyword
    for keyword in keywords {
        let keyword_lower = keyword.to_lowercase();
        let hits = content_lower.matches(&keyword_lower).count();
        total_hits += hits;
    }

    total_hits
}
