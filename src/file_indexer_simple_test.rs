#[cfg(test)]
mod tests {
    use crate::file_indexer::FileContentIndex;

    #[test]
    fn test_basic_indexing() {
        let index = FileContentIndex::new(1);
        
        // Add a file to the index
        index.add_file("test.rs", "fn test() { println!(\"test\"); }");
        
        // Search for a keyword that should be in the file
        let results = index.search(&["test".to_string()]);
        assert!(results.contains(&"test.rs".to_string()));
        
        // Remove the file
        index.remove_file("test.rs");
        
        // Search again, should not find the file
        let results = index.search(&["test".to_string()]);
        assert_eq!(results.len(), 0);
    }
}