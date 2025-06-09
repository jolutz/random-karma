//! Thread-local cache for storing calculation results.
//!
//! The cache persists across component re-renders, storing results keyed by
//! (target_ms, lap_count, player_count) tuples. This avoids expensive
//! recalculations when users adjust parameters back and forth.
//!
//! # Cache Key Structure
//! - `target`: Target time in milliseconds
//! - `lap_count`: Number of laps/cars per subset
//! - `player_count`: Number of players/subsets
//!
//! # Cache Value Structure
//! - `Vec<Vec<usize>>`: All generated subsets (car indices)
//! - `f64`: Jaccard similarity score (0.0-1.0)
//! - `u32`: Actual target used in calculation

use std::cell::RefCell;
use std::collections::HashMap;

/// Cache key: (target_ms, lap_count, player_count)
pub type CacheKey = (u32, usize, usize);

/// Cache value: (subsets, similarity, calculated_target)
pub type CacheValue = (Vec<Vec<usize>>, f64, u32);

thread_local! {
    /// Global cache that survives component lifetimes.
    /// Thread-local to avoid synchronization overhead in WASM.
    pub static CACHE_STORE: RefCell<HashMap<CacheKey, CacheValue>> =
        RefCell::new(HashMap::with_capacity(1000)); // Pre-allocate for performance
}
