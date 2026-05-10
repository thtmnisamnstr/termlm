use chrono::{DateTime, Duration, Utc};
use std::collections::{BTreeMap, VecDeque};

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub body: String,
    pub size_bytes: usize,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct WebCache {
    entries: BTreeMap<String, CacheEntry>,
    order: VecDeque<String>,
    max_bytes: usize,
    total_bytes: usize,
    ttl_secs: i64,
}

impl WebCache {
    pub fn new(max_bytes: usize, ttl_secs: u64) -> Self {
        Self {
            entries: BTreeMap::new(),
            order: VecDeque::new(),
            max_bytes,
            total_bytes: 0,
            ttl_secs: ttl_secs as i64,
        }
    }

    pub fn get(&mut self, key: &str) -> Option<String> {
        let now = Utc::now();
        if let Some(entry) = self.entries.get(key).cloned() {
            if now - entry.created_at > Duration::seconds(self.ttl_secs) {
                self.remove(key);
                return None;
            }
            self.touch(key);
            return Some(entry.body);
        }
        None
    }

    pub fn insert(&mut self, key: String, body: String) {
        let size = body.len();
        self.order.retain(|k| k != &key);
        if let Some(old) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.size_bytes);
        }

        self.entries.insert(
            key.clone(),
            CacheEntry {
                body,
                size_bytes: size,
                created_at: Utc::now(),
            },
        );
        self.order.push_back(key);
        self.total_bytes += size;

        while self.total_bytes > self.max_bytes {
            if let Some(k) = self.order.pop_front() {
                self.remove(&k);
            } else {
                break;
            }
        }
    }

    fn touch(&mut self, key: &str) {
        self.order.retain(|k| k != key);
        self.order.push_back(key.to_string());
    }

    fn remove(&mut self, key: &str) {
        self.order.retain(|k| k != key);
        if let Some(old) = self.entries.remove(key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.size_bytes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_refreshes_recency() {
        let mut cache = WebCache::new(64, 60);
        cache.insert("a".to_string(), "1111".to_string());
        cache.insert("b".to_string(), "2222".to_string());
        let _ = cache.get("a");
        cache.insert(
            "c".to_string(),
            "333333333333333333333333333333333333333333333333333333333333".to_string(),
        );
        assert!(cache.get("a").is_some());
    }
}
