use crate::cache::{CacheKey, CacheValue, CACHE_STORE};
use crate::chart::{add_failed_target_marker, add_similarity_data};
use futures::future::{AbortRegistration, Abortable};
use futures::{SinkExt, StreamExt};
use random_karma::worker_agent::{KarmaArgs, KarmaResult, KarmaTask, RequestMetadata};
use yew_agent::Spawnable;

pub fn cache_key(metadata: &RequestMetadata) -> CacheKey {
    CacheKey::new(
        metadata.dataset_generation,
        metadata.target,
        metadata.lap_count,
        metadata.player_count,
        metadata.tolerance_percent,
        metadata.timeout_ms,
        metadata.strategy,
    )
}

pub fn cached_result(metadata: &RequestMetadata) -> Option<CacheValue> {
    CACHE_STORE.with(|cache| cache.borrow().get(&cache_key(metadata)).cloned())
}

/// UI-neutral result of applying a correlated worker response.
pub enum CalculationOutcome {
    Success(CacheValue),
    Failure(String),
}

/// Runs one calculation on an exclusively owned worker bridge.
///
/// Aborting drops the bridge, terminating the corresponding browser worker.
pub async fn run_worker(
    args: KarmaArgs,
    abort_registration: AbortRegistration,
) -> Option<KarmaResult> {
    let task = async {
        let mut bridge = <KarmaTask as Spawnable>::spawner().spawn(crate::config::WORKER_SCRIPT);
        bridge.send(args).await.ok()?;
        bridge.next().await
    };
    Abortable::new(task, abort_registration)
        .await
        .ok()
        .flatten()
}

/// Updates chart and cache side effects, returning only the state needed by Yew.
pub fn apply_result(response: KarmaResult) -> CalculationOutcome {
    match response {
        Ok(success) => {
            add_similarity_data(
                success.calculated_target,
                success.similarity * 100.0,
                success.metadata.lap_count as u32,
                success.metadata.player_count as u32,
            );
            let value = (success.sets, success.similarity, success.calculated_target);
            CACHE_STORE.with(|cache| {
                cache
                    .borrow_mut()
                    .insert(cache_key(&success.metadata), value.clone());
            });
            CalculationOutcome::Success(value)
        }
        Err(failure) => {
            add_failed_target_marker(
                failure.metadata.target,
                failure.metadata.lap_count as u32,
                failure.metadata.player_count as u32,
            );
            CalculationOutcome::Failure(failure.error)
        }
    }
}
