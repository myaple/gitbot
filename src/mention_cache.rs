use std::collections::HashSet;
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Clone)]
pub struct MentionCache {
    processed_ids: Arc<Mutex<HashSet<i64>>>,
}

impl MentionCache {
    pub fn new() -> Self {
        MentionCache {
            processed_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub async fn check(&self, mention_id: i64) -> bool {
        let set = self.processed_ids.lock().await;
        set.contains(&mention_id)
    }

    pub async fn add(&self, mention_id: i64) {
        let mut set = self.processed_ids.lock().await;
        set.insert(mention_id);
    }
}
