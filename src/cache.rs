//! Thread-local cache for calculation results.
//!
//! Entries are deliberately scoped to the loaded dataset generation and every
//! solver setting that can affect a result. Timeout is included as an exact
//! `f64::to_bits` value: although it is primarily a search budget, changing it
//! can change a randomized search result, so results from different budgets are
//! never reused.

use std::cell::RefCell;
use std::collections::HashMap;

/// Identity of a cached solver request.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct CacheKey {
    pub dataset_generation: u64,
    pub target_ms: u32,
    pub lap_count: usize,
    pub player_count: usize,
    pub tolerance_percent_bits: u64,
    pub timeout_ms_bits: u64,
}

impl CacheKey {
    pub fn new(
        dataset_generation: u64,
        target_ms: u32,
        lap_count: usize,
        player_count: usize,
        tolerance_percent: f64,
        timeout_ms: f64,
    ) -> Self {
        Self {
            dataset_generation,
            target_ms,
            lap_count,
            player_count,
            tolerance_percent_bits: tolerance_percent.to_bits(),
            timeout_ms_bits: timeout_ms.to_bits(),
        }
    }
}

/// Cache value: (subsets, similarity, calculated_target).
pub type CacheValue = (Vec<Vec<usize>>, f64, u32);

/// Lookup abstraction that keeps legacy cache-counting code source-compatible.
/// New code must use `CacheKey`; legacy lookups intentionally only answer
/// whether any dataset/settings-specific entry matches the old three fields.
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

pub struct CacheStore(HashMap<CacheKey, CacheValue>);

impl CacheStore {
    pub fn new() -> Self {
        Self(HashMap::with_capacity(1000))
    }

    pub fn contains_key<K: CacheLookup + ?Sized>(&self, key: &K) -> bool {
        self.0.keys().any(|cache_key| key.matches(cache_key))
    }

    pub fn get<K: CacheLookup + ?Sized>(&self, key: &K) -> Option<&CacheValue> {
        self.0
            .iter()
            .find_map(|(cache_key, value)| key.matches(cache_key).then_some(value))
    }

    pub fn insert(&mut self, key: CacheKey, value: CacheValue) -> Option<CacheValue> {
        self.0.insert(key, value)
    }

    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&CacheKey, &CacheValue)> {
        self.0.iter()
    }
}

thread_local! {
    /// Global cache that survives component lifetimes.
    /// Thread-local to avoid synchronization overhead in WASM.
    pub static CACHE_STORE: RefCell<CacheStore> = RefCell::new(CacheStore::new());
}
