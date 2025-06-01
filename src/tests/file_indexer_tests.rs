use crate::file_indexer::FileContentIndex;

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
