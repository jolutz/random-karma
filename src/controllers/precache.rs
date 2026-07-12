use crate::cache::CACHE_STORE;
use crate::chart::{add_failed_target_marker, add_similarity_data};
use crate::controllers::calculation::cache_key;
use crate::utils::{base_target_step, spread_indices};
use futures::future::{AbortHandle, Abortable};
use futures::{Sink, SinkExt, Stream, StreamExt};
use random_karma::worker_agent::{KarmaArgs, KarmaResult, KarmaTask, RequestMetadata};
use random_karma::{get_target_range_for_subset, Car};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use yew::UseStateHandle;
use yew_agent::Spawnable;

const WORKER_COUNT: usize = 4;
const UPDATE_BATCH_SIZE: usize = 8;

pub struct PrecacheConfig {
    pub cars: Vec<Car>,
    pub lap_count: usize,
    pub player_count: usize,
    pub timeout_secs: f64,
    pub tolerance_percent: f64,
}

#[derive(Clone)]
pub struct PrecacheExecutionContext {
    pub cache_version: UseStateHandle<usize>,
    pub error_count: UseStateHandle<usize>,
    pub failed_targets: UseStateHandle<Rc<Vec<u32>>>,
    pub dataset_generation: Rc<Cell<u64>>,
    pub expected_dataset_generation: u64,
    pub precache_generation: Rc<Cell<u64>>,
    pub expected_precache_generation: u64,
}

pub struct PrecacheJob {
    pub config: PrecacheConfig,
    pub context: PrecacheExecutionContext,
    pub request_ids: Rc<Cell<u64>>,
    pub abort_handles: Rc<RefCell<Vec<AbortHandle>>>,
}

fn next_request_id(request_ids: &Cell<u64>) -> u64 {
    let next = request_ids.get().wrapping_add(1);
    request_ids.set(next);
    next
}

fn is_current(context: &PrecacheExecutionContext) -> bool {
    context.dataset_generation.get() == context.expected_dataset_generation
        && context.precache_generation.get() == context.expected_precache_generation
}

fn update_cache_version(cache_version: &UseStateHandle<usize>) {
    cache_version.set(cache_version.wrapping_add(1));
}

fn flush_updates(
    context: &PrecacheExecutionContext,
    completed_since_update: &mut usize,
    failed: &mut Vec<u32>,
) {
    if *completed_since_update == 0 {
        return;
    }
    update_cache_version(&context.cache_version);
    if !failed.is_empty() {
        context.error_count.set(*context.error_count + failed.len());
        let mut all_failed = (*context.failed_targets).to_vec();
        all_failed.append(failed);
        context.failed_targets.set(Rc::new(all_failed));
    }
    *completed_since_update = 0;
}

async fn process_target(
    bridge: &mut (impl Stream<Item = KarmaResult> + Sink<KarmaArgs> + Unpin),
    args: KarmaArgs,
    context: &PrecacheExecutionContext,
) -> Result<(), ()> {
    let metadata = args.metadata.clone();
    if !is_current(context) {
        return Err(());
    }
    bridge.send(args).await.map_err(|_| ())?;
    let response = bridge.next().await.ok_or(())?;
    if !is_current(context) {
        return Err(());
    }

    match response {
        Ok(success) if success.metadata == metadata => {
            add_similarity_data(
                success.calculated_target,
                success.similarity * 100.0,
                metadata.lap_count as u32,
                metadata.player_count as u32,
            );
            CACHE_STORE.with(|cache| {
                cache.borrow_mut().insert(
                    cache_key(&metadata),
                    (success.sets, success.similarity, success.calculated_target),
                );
            });
            Ok(())
        }
        Err(failure) if failure.metadata == metadata => {
            add_failed_target_marker(
                metadata.target,
                metadata.lap_count as u32,
                metadata.player_count as u32,
            );
            Err(())
        }
        _ => Err(()),
    }
}

pub fn run(job: PrecacheJob) {
    let PrecacheJob {
        config,
        context,
        request_ids,
        abort_handles,
    } = job;
    let PrecacheConfig {
        cars,
        lap_count,
        player_count,
        timeout_secs,
        tolerance_percent,
    } = config;
    let (min, max) = get_target_range_for_subset(&cars, lap_count);
    let step = base_target_step(min, max);
    let order = Rc::new(spread_indices(crate::config::SLIDER_MAX_INDEX + 1));

    for worker_idx in 0..WORKER_COUNT {
        let cars = cars.clone();
        let context = context.clone();
        let request_ids = request_ids.clone();
        let order = order.clone();
        let (abort_handle, abort_registration) = AbortHandle::new_pair();
        abort_handles.borrow_mut().push(abort_handle);

        wasm_bindgen_futures::spawn_local(async move {
            let worker = async move {
                let mut bridge =
                    <KarmaTask as Spawnable>::spawner().spawn(crate::config::WORKER_SCRIPT);
                let mut completed_since_update = 0usize;
                let mut failed = Vec::new();

                for pos in (worker_idx..order.len()).step_by(WORKER_COUNT) {
                    if !is_current(&context) {
                        return;
                    }
                    let target = (min + step * order[pos] as u32).min(max);
                    let metadata = RequestMetadata {
                        request_id: next_request_id(&request_ids),
                        dataset_generation: context.expected_dataset_generation,
                        target,
                        lap_count,
                        player_count,
                        timeout_ms: timeout_secs * 1000.0,
                        tolerance_percent,
                    };
                    if CACHE_STORE.with(|cache| cache.borrow().contains_key(&cache_key(&metadata)))
                    {
                        continue;
                    }
                    let args = KarmaArgs {
                        cars: cars.clone(),
                        metadata,
                    };
                    if process_target(&mut bridge, args, &context).await.is_err() {
                        failed.push(target);
                    }
                    completed_since_update += 1;
                    if completed_since_update >= UPDATE_BATCH_SIZE {
                        flush_updates(&context, &mut completed_since_update, &mut failed);
                    }
                }
                flush_updates(&context, &mut completed_since_update, &mut failed);
            };
            let _ = Abortable::new(worker, abort_registration).await;
        });
    }
}
