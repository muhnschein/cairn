//! Bounded, global cluster cache.
//!
//! One decompressed cluster can back many entries, so a client that walks a
//! cluster's blobs pays for one decode. The budget is global, not per archive,
//! so memory does not scale with the number of open archives.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Cache key: which archive, which cluster.
pub type Key = (usize, u32);

/// Hit/miss counters reported by `/v1/status`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    pub bytes: usize,
    pub entries: usize,
}

#[derive(Debug)]
struct Slot {
    body: Arc<Vec<u8>>,
    offset_size: usize,
    last: u64,
}

#[derive(Debug, Default)]
struct Inner {
    slots: HashMap<Key, Slot>,
    bytes: usize,
    tick: u64,
    hits: u64,
    misses: u64,
    evictions: u64,
}

/// LRU cluster cache with a byte budget.
#[derive(Debug)]
pub struct ClusterCache {
    inner: Mutex<Inner>,
    budget: usize,
}

impl ClusterCache {
    /// A cache holding at most `budget` bytes of decompressed cluster bodies.
    pub fn new(budget: usize) -> ClusterCache {
        ClusterCache { inner: Mutex::new(Inner::default()), budget }
    }

    /// Look up a cluster body, counting the hit or miss.
    pub fn get(&self, key: Key) -> Option<(Arc<Vec<u8>>, usize)> {
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        match inner.slots.get_mut(&key) {
            Some(slot) => {
                slot.last = tick;
                let found = (Arc::clone(&slot.body), slot.offset_size);
                inner.hits += 1;
                Some(found)
            }
            None => {
                inner.misses += 1;
                None
            }
        }
    }

    /// Insert a decoded cluster body, evicting least-recently-used slots.
    ///
    /// A body larger than the whole budget is returned uncached rather than
    /// emptying the cache for one entry.
    pub fn insert(&self, key: Key, body: Vec<u8>, offset_size: usize) -> Arc<Vec<u8>> {
        let body = Arc::new(body);
        if body.len() > self.budget {
            return body;
        }
        let mut inner = self.lock();
        inner.tick += 1;
        let tick = inner.tick;
        if let Some(old) = inner.slots.remove(&key) {
            inner.bytes -= old.body.len();
        }
        inner.bytes += body.len();
        inner.slots.insert(key, Slot { body: Arc::clone(&body), offset_size, last: tick });
        while inner.bytes > self.budget {
            // The slot count is bounded by budget / cluster size, so this scan is short.
            let Some(&victim) = inner
                .slots
                .iter()
                .min_by_key(|(_, s)| s.last)
                .map(|(k, _)| k)
            else {
                break;
            };
            if victim == key {
                break;
            }
            if let Some(old) = inner.slots.remove(&victim) {
                inner.bytes -= old.body.len();
                inner.evictions += 1;
            }
        }
        body
    }

    /// Drop everything cached for one archive.
    pub fn forget_archive(&self, archive: usize) {
        let mut inner = self.lock();
        let keys: Vec<Key> = inner.slots.keys().copied().filter(|(a, _)| *a == archive).collect();
        for k in keys {
            if let Some(old) = inner.slots.remove(&k) {
                inner.bytes -= old.body.len();
            }
        }
    }

    /// Current counters.
    pub fn stats(&self) -> Stats {
        let inner = self.lock();
        Stats {
            hits: inner.hits,
            misses: inner.misses,
            evictions: inner.evictions,
            bytes: inner.bytes,
            entries: inner.slots.len(),
        }
    }

    /// Configured budget in bytes.
    pub fn budget(&self) -> usize {
        self.budget
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic in a cache method would poison the lock; there is no state to
        // repair, so the poisoned guard is taken as-is.
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evicts_least_recently_used() {
        let c = ClusterCache::new(300);
        c.insert((0, 0), vec![0; 100], 4);
        c.insert((0, 1), vec![0; 100], 4);
        c.insert((0, 2), vec![0; 100], 4);
        assert!(c.get((0, 0)).is_some()); // 0 is now the most recent
        c.insert((0, 3), vec![0; 100], 4);
        assert!(c.get((0, 1)).is_none(), "oldest slot should have gone");
        assert!(c.get((0, 0)).is_some());
        assert!(c.stats().bytes <= 300);
    }

    #[test]
    fn oversized_body_is_not_cached() {
        let c = ClusterCache::new(100);
        let body = c.insert((0, 0), vec![0; 500], 4);
        assert_eq!(body.len(), 500);
        assert_eq!(c.stats().entries, 0);
    }

    #[test]
    fn forget_archive_frees_bytes() {
        let c = ClusterCache::new(1000);
        c.insert((0, 0), vec![0; 100], 4);
        c.insert((1, 0), vec![0; 100], 4);
        c.forget_archive(0);
        assert_eq!(c.stats().entries, 1);
        assert_eq!(c.stats().bytes, 100);
    }
}
