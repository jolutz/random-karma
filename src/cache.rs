//! Thread-local cache for calculation results.
//!
//! Entries are scoped to the loaded dataset generation and every solver setting
//! that can affect a result. The cache is deliberately bounded so exploring
//! many combinations cannot grow browser memory without limit.

use random_karma::SolverStrategy;
use std::cell::RefCell;
use std::collections::{hash_map::Entry, HashMap, VecDeque};

/// Maximum number of full calculation results retained in memory.
///
/// A result can contain many subsets, so keeping this modest is preferable to
/// retaining every pre-cache run for the entire browser session.
pub const MAX_CACHE_ENTRIES: usize = 256;

/// Identity of a cached solver request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CacheKey {
    pub dataset_generation: u64,
    pub target_ms: u32,
    pub lap_count: usize,
    pub player_count: usize,
    pub tolerance_percent_bits: u64,
    pub timeout_ms_bits: u64,
    pub strategy: SolverStrategy,
}

impl CacheKey {
    pub fn new(
        dataset_generation: u64,
        target_ms: u32,
        lap_count: usize,
        player_count: usize,
        tolerance_percent: f64,
        timeout_ms: f64,
        strategy: SolverStrategy,
    ) -> Self {
        Self {
            dataset_generation,
            target_ms,
            lap_count,
            player_count,
            tolerance_percent_bits: tolerance_percent.to_bits(),
            timeout_ms_bits: timeout_ms.to_bits(),
            strategy,
        }
    }
}

/// Cache value: (subsets, similarity, calculated_target).
pub type CacheValue = (Vec<Vec<usize>>, f64, u32);

/// Lookup abstraction that keeps legacy cache-counting code source-compatible.
pub trait CacheLookup {
    fn matches(&self, key: &CacheKey) -> bool;
}

impl CacheLookup for CacheKey {
    fn matches(&self, key: &CacheKey) -> bool {
        self == key
    }
}

impl CacheLookup for (u32, usize, usize) {
    fn matches(&self, key: &CacheKey) -> bool {
        (key.target_ms, key.lap_count, key.player_count) == *self
    }
}

/// A bounded insertion-ordered cache.
///
/// FIFO eviction is intentional: pre-cache jobs visit targets in a spread-out
/// order, and this avoids the mutation/borrowing overhead of a full LRU while
/// providing a hard memory bound.
pub struct CacheStore {
    entries: HashMap<CacheKey, CacheValue>,
    insertion_order: VecDeque<CacheKey>,
}

impl CacheStore {
    pub fn new() -> Self {
        Self {
            entries: HashMap::with_capacity(MAX_CACHE_ENTRIES),
            insertion_order: VecDeque::with_capacity(MAX_CACHE_ENTRIES),
        }
    }

    pub fn contains_key<K: CacheLookup + ?Sized>(&self, key: &K) -> bool {
        self.entries.keys().any(|cache_key| key.matches(cache_key))
    }

    pub fn get<K: CacheLookup + ?Sized>(&self, key: &K) -> Option<&CacheValue> {
        self.entries
            .iter()
            .find_map(|(cache_key, value)| key.matches(cache_key).then_some(value))
    }

    pub fn insert(&mut self, key: CacheKey, value: CacheValue) -> Option<CacheValue> {
        match self.entries.entry(key) {
            Entry::Occupied(mut entry) => Some(entry.insert(value)),
            Entry::Vacant(entry) => {
                let key = entry.into_key();
                while self.entries.len() >= MAX_CACHE_ENTRIES {
                    let Some(oldest_key) = self.insertion_order.pop_front() else {
                        break;
                    };
                    self.entries.remove(&oldest_key);
                }

                self.insertion_order.push_back(key.clone());
                self.entries.insert(key, value);
                None
            }
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.insertion_order.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&CacheKey, &CacheValue)> {
        self.entries.iter()
    }
}

thread_local! {
    /// Global cache that survives component lifetimes.
    /// Thread-local to avoid synchronization overhead in WASM.
    pub static CACHE_STORE: RefCell<CacheStore> = RefCell::new(CacheStore::new());
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(target_ms: u32) -> CacheKey {
        CacheKey::new(0, target_ms, 1, 1, 0.5, 1_000.0, SolverStrategy::Bounded)
    }

    fn value(target_ms: u32) -> CacheValue {
        (vec![vec![target_ms as usize]], 0.0, target_ms)
    }

    #[test]
    fn evicts_the_oldest_entry_at_capacity() {
        let mut cache = CacheStore::new();
        for target in 0..MAX_CACHE_ENTRIES as u32 {
            cache.insert(key(target), value(target));
        }

        cache.insert(
            key(MAX_CACHE_ENTRIES as u32),
            value(MAX_CACHE_ENTRIES as u32),
        );

        assert_eq!(cache.len(), MAX_CACHE_ENTRIES);
        assert!(cache.get(&key(0)).is_none());
        assert!(cache.get(&key(MAX_CACHE_ENTRIES as u32)).is_some());
    }

    #[test]
    fn strategies_use_distinct_entries() {
        let mut cache = CacheStore::new();
        let bounded = key(1);
        let mut legacy = bounded.clone();
        legacy.strategy = SolverStrategy::Legacy;

        cache.insert(bounded.clone(), value(1));
        cache.insert(legacy.clone(), value(2));

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.get(&bounded).unwrap().2, 1);
        assert_eq!(cache.get(&legacy).unwrap().2, 2);
    }

    #[test]
    fn replacing_an_entry_does_not_evict_another_entry() {
        let mut cache = CacheStore::new();
        cache.insert(key(1), value(1));
        cache.insert(key(1), value(2));

        assert_eq!(cache.len(), 1);
        assert_eq!(cache.get(&key(1)).expect("entry exists").2, 2);
    }
}
