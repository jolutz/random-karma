use crate::cache::CACHE_STORE;
use crate::chart::{add_similarity_data, init_similarity_chart};
use random_karma::SolverStrategy;

#[derive(Clone, Copy)]
pub struct ChartCacheFilter {
    pub dataset_generation: u64,
    pub lap_count: usize,
    pub player_count: usize,
    pub timeout_ms: f64,
    pub tolerance_percent: f64,
    pub strategy: SolverStrategy,
}

/// Initializes the chart and replays a sorted, settings-specific cache snapshot.
pub fn initialize_and_replay(min: u32, max: u32, filter: ChartCacheFilter) {
    if max <= min {
        return;
    }

    init_similarity_chart(
        min,
        max,
        filter.lap_count as u32,
        filter.player_count as u32,
    );

    let mut entries: Vec<(u32, f64)> = CACHE_STORE.with(|cache| {
        cache
            .borrow()
            .iter()
            .filter(|(key, _)| {
                key.dataset_generation == filter.dataset_generation
                    && key.lap_count == filter.lap_count
                    && key.player_count == filter.player_count
                    && key.timeout_ms_bits == filter.timeout_ms.to_bits()
                    && key.tolerance_percent_bits == filter.tolerance_percent.to_bits()
                    && key.strategy == filter.strategy
            })
            .map(|(key, (_, similarity, _))| (key.target_ms, *similarity))
            .collect()
    });
    entries.sort_by_key(|(target, _)| *target);

    for (target, similarity) in entries {
        add_similarity_data(
            target,
            similarity * 100.0,
            filter.lap_count as u32,
            filter.player_count as u32,
        );
    }
}
