use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;

#[derive(Hash, Eq, PartialEq, Clone)]
struct CacheKey {
    spreadsheet_id: String,
    range: String,
    render_option: String,
}

struct CacheEntry {
    data: Value,
    inserted_at: Instant,
}

pub struct SheetCache {
    entries: HashMap<CacheKey, CacheEntry>,
    order: Vec<CacheKey>,
    max_entries: usize,
    ttl: Duration,
    hits: u64,
    misses: u64,
}

impl SheetCache {
    pub fn new(max_entries: usize, ttl_seconds: u64) -> Self {
        Self {
            entries: HashMap::new(),
            order: Vec::new(),
            max_entries,
            ttl: Duration::from_secs(ttl_seconds),
            hits: 0,
            misses: 0,
        }
    }

    pub fn get(
        &mut self,
        spreadsheet_id: &str,
        range: &str,
        render_option: &str,
    ) -> Option<&Value> {
        let key = CacheKey {
            spreadsheet_id: spreadsheet_id.to_string(),
            range: range.to_string(),
            render_option: render_option.to_string(),
        };
        if let Some(entry) = self.entries.get(&key) {
            if entry.inserted_at.elapsed() < self.ttl {
                self.hits += 1;
                self.order.retain(|k| k != &key);
                self.order.push(key.clone());
                return self.entries.get(&key).map(|e| &e.data);
            }
            self.entries.remove(&key);
            self.order.retain(|k| k != &key);
        }
        self.misses += 1;
        None
    }

    pub fn put(
        &mut self,
        spreadsheet_id: &str,
        range: &str,
        render_option: &str,
        data: Value,
    ) {
        let key = CacheKey {
            spreadsheet_id: spreadsheet_id.to_string(),
            range: range.to_string(),
            render_option: render_option.to_string(),
        };
        self.order.retain(|k| k != &key);
        if self.entries.len() >= self.max_entries && !self.entries.contains_key(&key) {
            if let Some(evict_key) = self.order.first().cloned() {
                self.entries.remove(&evict_key);
                self.order.remove(0);
            }
        }
        self.entries.insert(
            key.clone(),
            CacheEntry {
                data,
                inserted_at: Instant::now(),
            },
        );
        self.order.push(key);
    }

    pub fn invalidate(&mut self, spreadsheet_id: &str) {
        self.entries
            .retain(|k, _| k.spreadsheet_id != spreadsheet_id);
        self.order
            .retain(|k| k.spreadsheet_id != spreadsheet_id);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
    }

    pub fn stats(&self) -> (u64, u64, usize) {
        (self.hits, self.misses, self.entries.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn put_and_get() {
        let mut cache = SheetCache::new(10, 300);
        cache.put("abc", "A1:B2", "FORMATTED_VALUE", json!({"values": [["a"]]}));
        assert!(cache.get("abc", "A1:B2", "FORMATTED_VALUE").is_some());
    }

    #[test]
    fn miss_on_unknown() {
        let mut cache = SheetCache::new(10, 300);
        assert!(cache.get("abc", "A1:B2", "FORMATTED_VALUE").is_none());
    }

    #[test]
    fn different_render_option_is_different_key() {
        let mut cache = SheetCache::new(10, 300);
        cache.put("abc", "A1", "FORMATTED_VALUE", json!("formatted"));
        cache.put("abc", "A1", "FORMULA", json!("=SUM(B1)"));
        assert_eq!(cache.get("abc", "A1", "FORMATTED_VALUE").unwrap(), &json!("formatted"));
        assert_eq!(cache.get("abc", "A1", "FORMULA").unwrap(), &json!("=SUM(B1)"));
    }

    #[test]
    fn evicts_lru() {
        let mut cache = SheetCache::new(2, 300);
        cache.put("a", "A1", "V", json!(1));
        cache.put("b", "A1", "V", json!(2));
        cache.put("c", "A1", "V", json!(3));
        assert!(cache.get("a", "A1", "V").is_none());
        assert!(cache.get("b", "A1", "V").is_some());
        assert!(cache.get("c", "A1", "V").is_some());
    }

    #[test]
    fn invalidate_by_spreadsheet() {
        let mut cache = SheetCache::new(10, 300);
        cache.put("abc", "A1", "V", json!(1));
        cache.put("abc", "B1", "V", json!(2));
        cache.put("xyz", "A1", "V", json!(3));
        cache.invalidate("abc");
        assert!(cache.get("abc", "A1", "V").is_none());
        assert!(cache.get("abc", "B1", "V").is_none());
        assert!(cache.get("xyz", "A1", "V").is_some());
    }

    #[test]
    fn ttl_expiry() {
        let mut cache = SheetCache::new(10, 0);
        cache.put("abc", "A1", "V", json!(1));
        std::thread::sleep(Duration::from_millis(10));
        assert!(cache.get("abc", "A1", "V").is_none());
    }

    #[test]
    fn stats_tracking() {
        let mut cache = SheetCache::new(10, 300);
        cache.put("abc", "A1", "V", json!(1));
        cache.get("abc", "A1", "V"); // hit
        cache.get("abc", "B1", "V"); // miss
        let (hits, misses, entries) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
        assert_eq!(entries, 1);
    }
}
