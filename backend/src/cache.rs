use std::{hash::Hash, num::NonZeroUsize, sync::Mutex};

use lru::LruCache;

/// A thread-safe least-recently-used cache.
///
/// `capacity` is the maximum number of entries. Inserting a new entry
/// at capacity evicts the least recently used entry.
///
/// Values are cloned by [`Self::get`], which keeps the cache lock out of caller
/// code and avoids holding it across asynchronous work.
pub(crate) struct BoundedCache<K, V> {
    inner: Mutex<LruCache<K, V>>,
}

impl<K, V> BoundedCache<K, V>
where
    K: Eq + Hash,
    V: Clone,
{
    /// Creates an empty cache that holds at most `capacity` entries.
    ///
    /// # Panics
    ///
    /// Panics if `capacity` is zero.
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(LruCache::new(
                NonZeroUsize::new(capacity).expect("cache capacity is positive"),
            )),
        }
    }

    /// Returns a clone of the value and marks the entry as most recently used.
    ///
    /// Returns `None` without modifying the cache when `key` is absent.
    pub(crate) fn get(&self, key: &K) -> Option<V> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(key)
            .cloned()
    }

    /// Returns the existing value or atomically inserts a newly created value.
    pub(crate) fn get_or_insert_with(&self, key: K, create: impl FnOnce() -> V) -> V {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(value) = inner.get(&key) {
            return value.clone();
        }
        let value = create();
        inner.put(key, value.clone());
        value
    }

    /// Inserts an entry and marks it as most recently used.
    ///
    /// If `key` already exists, its value is replaced without changing the
    /// number of entries or evicting another entry. If `key` is new and the
    /// cache is at capacity, the least recently used entry is evicted first.
    pub(crate) fn insert(&self, key: K, value: V) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .put(key, value);
    }

    /// Removes `key` when present.
    ///
    /// This is a no-op when `key` does not exist.
    pub(crate) fn remove(&self, key: &K) {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop(key);
    }
}

#[cfg(test)]
mod tests {
    use super::BoundedCache;

    #[test]
    fn evicts_the_least_recently_used_entry() {
        let cache = BoundedCache::new(2);
        cache.insert(1, "one");
        cache.insert(2, "two");
        assert_eq!(cache.get(&1), Some("one"));

        cache.insert(3, "three");

        assert_eq!(cache.get(&1), Some("one"));
        assert_eq!(cache.get(&2), None);
        assert_eq!(cache.get(&3), Some("three"));
    }

    #[test]
    fn replaces_and_removes_entries() {
        let cache = BoundedCache::new(2);
        cache.insert(1, "old");
        cache.insert(2, "other");
        cache.insert(1, "new");
        assert_eq!(cache.get(&1), Some("new"));
        assert_eq!(cache.get(&2), Some("other"));

        cache.remove(&1);
        assert_eq!(cache.get(&1), None);
        cache.remove(&3);
        assert_eq!(cache.get(&2), Some("other"));
    }
}
