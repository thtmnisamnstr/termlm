use std::collections::BTreeMap;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct TimedCache<V> {
    map: BTreeMap<String, (Instant, V)>,
    ttl: Duration,
}

impl<V: Clone> TimedCache<V> {
    pub fn new(ttl: Duration) -> Self {
        Self {
            map: BTreeMap::new(),
            ttl,
        }
    }

    pub fn get(&mut self, key: &str) -> Option<V> {
        if let Some((at, value)) = self.map.get(key)
            && at.elapsed() <= self.ttl
        {
            return Some(value.clone());
        }
        self.map.remove(key);
        None
    }

    pub fn insert(&mut self, key: impl Into<String>, value: V) {
        self.map.insert(key.into(), (Instant::now(), value));
    }
}
