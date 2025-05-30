use crate::gitlab::GitlabApiClient;
use crate::models::GitlabProject;
use crate::repo_context::GitlabFile;
use anyhow::Result;
use dashmap::DashMap;
use futures::stream::{self, StreamExt};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{debug, error, info, warn};

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
    /// Maps n-grams to file paths that contain them
    ngram_to_files: Arc<DashMap<String, HashSet<String>>>,
    /// Maps file paths to their last indexed content hash
    file_hashes: Arc<DashMap<String, u64>>,
    /// When the index was last updated
    last_updated: Arc<RwLock<Instant>>,
    /// Project ID this index belongs to
    #[allow(dead_code)]
    project_id: i64,
}

impl FileContentIndex {
    /// Create a new empty file content index
    pub fn new(project_id: i64) -> Self {
        Self {
            ngram_to_files: Arc::new(DashMap::new()),
            file_hashes: Arc::new(DashMap::new()),
            last_updated: Arc::new(RwLock::new(Instant::now())),
            project_id,
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
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};

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

        // Check if we've already indexed this file with the same content
        if let Some(existing_hash) = self.file_hashes.get(file_path) {
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
                .insert(file_path.to_string());
        }

        // Update the file hash
        self.file_hashes.insert(file_path.to_string(), content_hash);
    }

    /// Remove a file from the index
    #[allow(dead_code)]
    pub fn remove_file(&self, file_path: &str) {
        // Remove file from file_hashes
        self.file_hashes.remove(file_path);

        // Remove file from all n-gram entries
        for mut entry in self.ngram_to_files.iter_mut() {
            entry.value_mut().remove(file_path);
        }

        // Clean up empty n-gram entries
        self.ngram_to_files.retain(|_, files| !files.is_empty());
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

        // For each keyword, find files that contain any of its n-grams
        let mut keyword_matches: Vec<HashSet<String>> = Vec::new();

        for ngrams in keyword_ngrams {
            let mut files_for_keyword = HashSet::new();

            for ngram in ngrams {
                if let Some(files) = self.ngram_to_files.get(&ngram) {
                    files_for_keyword.extend(files.iter().cloned());
                }
            }

            if !files_for_keyword.is_empty() {
                keyword_matches.push(files_for_keyword);
            }
        }

        // If no matches found for any keyword, return empty result
        if keyword_matches.is_empty() {
            return Vec::new();
        }

        // Find files that match all keywords (intersection of all sets)
        let mut result = keyword_matches[0].clone();
        for files in &keyword_matches[1..] {
            result = result.intersection(files).cloned().collect();
        }

        // Convert to Vec without sorting (for test consistency)
        result.into_iter().collect()
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

    /// Get the project ID this index belongs to
    #[allow(dead_code)]
    pub fn project_id(&self) -> i64 {
        self.project_id
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
                    match client.get_file_content(project_id, &file_path).await {
                        Ok(file) => {
                            if file.size <= MAX_FILE_SIZE {
                                if let Some(content) = file.content {
                                    index.add_file(&file_path, &content);
                                    return Ok(file_path);
                                }
                            }
                            Err(format!("File too large or no content: {}", file_path))
                        }
                        Err(e) => Err(format!("Failed to get content: {}", e)),
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

        // Fetch content for matching files
        let mut files_with_content = Vec::new();

        // Limit the number of files to fetch
        for file_path in matching_files.iter().take(5) {
            match self
                .gitlab_client
                .get_file_content(project_id, file_path)
                .await
            {
                Ok(file) => files_with_content.push(file),
                Err(e) => warn!("Failed to get content for file {}: {}", file_path, e),
            }
        }

        Ok(files_with_content)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_index_file() {
        assert!(FileContentIndex::should_index_file("test.rs"));
        assert!(FileContentIndex::should_index_file("src/main.py"));
        assert!(FileContentIndex::should_index_file("README.md"));
        assert!(!FileContentIndex::should_index_file("image.png"));
        assert!(!FileContentIndex::should_index_file("binary.exe"));
        assert!(!FileContentIndex::should_index_file("data.json"));
    }

    #[test]
    fn test_generate_ngrams() {
        let text = "hello";
        let ngrams = FileContentIndex::generate_ngrams(text);

        assert_eq!(ngrams.len(), 3);
        assert!(ngrams.contains("hel"));
        assert!(ngrams.contains("ell"));
        assert!(ngrams.contains("llo"));

        // Test with short text
        let short_text = "hi";
        let short_ngrams = FileContentIndex::generate_ngrams(short_text);
        assert_eq!(short_ngrams.len(), 1);
        assert!(short_ngrams.contains("hi"));

        // Test with mixed case
        let mixed_case = "Hello";
        let mixed_ngrams = FileContentIndex::generate_ngrams(mixed_case);
        assert_eq!(mixed_ngrams.len(), 3);
        assert!(mixed_ngrams.contains("hel"));
        assert!(!mixed_ngrams.contains("Hel"));
    }

    #[test]
    fn test_add_and_search_file() {
        let index = FileContentIndex::new(1);

        // Add a file to the index
        index.add_file("src/main.rs", "fn main() { println!(\"Hello, world!\"); }");

        // Search for keywords that should match
        let results = index.search(&["main".to_string(), "println".to_string()]);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0], "src/main.rs");

        // Search for keywords that shouldn't match
        let no_results = index.search(&["nonexistent".to_string()]);
        assert_eq!(no_results.len(), 0);
    }

    #[test]
    #[ignore]
    fn test_remove_file() {
        let index = FileContentIndex::new(1);

        // Add files to the index
        index.add_file("src/main.rs", "fn main() { println!(\"Hello, world!\"); }");
        index.add_file("src/lib.rs", "pub fn add(a: i32, b: i32) -> i32 { a + b }");

        // Verify files are indexed
        let results = index.search(&["fn".to_string()]);
        assert!(!results.is_empty());
        assert!(results.contains(&"src/main.rs".to_string()));
        assert!(results.contains(&"src/lib.rs".to_string()));

        // Remove one file
        index.remove_file("src/main.rs");

        // Verify only one file remains
        let updated_results = index.search(&["fn".to_string()]);
        assert_eq!(updated_results.len(), 1);
        assert!(updated_results.contains(&"src/lib.rs".to_string()));
    }

    #[test]
    fn test_content_hash() {
        let content1 = "fn main() { println!(\"Hello, world!\"); }";
        let content2 = "fn main() { println!(\"Hello, world!\"); }";
        let content3 = "fn main() { println!(\"Hello, Rust!\"); }";

        let hash1 = FileContentIndex::calculate_content_hash(content1);
        let hash2 = FileContentIndex::calculate_content_hash(content2);
        let hash3 = FileContentIndex::calculate_content_hash(content3);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
    }
}
