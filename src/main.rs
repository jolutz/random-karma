//! Main module for Random Karma application using Yew.
//! Wires UI components, state hooks, and side-effect logic.

use futures::StreamExt;
use gloo_timers::callback::Timeout;
use random_karma::{
    format_ms_to_minsecms, get_target_range_for_subset, read_cars_from_csv_string,
    worker_agent::{KarmaArgs, KarmaResult, KarmaTask, RequestMetadata},
    Car,
};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use web_sys::HtmlInputElement;
use yew::prelude::*;
use yew_agent::reactor::{use_reactor_subscription, ReactorProvider};
use yew_agent::Spawnable;

mod cache;
mod chart;
mod components;
mod config; // Add this line
mod utils;

use cache::{CacheKey, CacheValue, CACHE_STORE};
use chart::{add_failed_target_marker, add_similarity_data, init_similarity_chart};
use components::ResultsWrapper;
use config::*; // This will bring SLIDER_MAX_INDEX and other config constants into scope
use utils::{
    base_target_range, base_target_step, calc_target_from_idx, parse_time_to_ms, spread_indices,
};

// Fixed because `available_parallelism` cannot be evaluated in a const. Workers
// are browser-hosted, so a portable conservative pool is preferable here.
const WORKER_COUNT: usize = 4;

fn cache_key(metadata: &RequestMetadata) -> CacheKey {
    CacheKey::new(
        metadata.dataset_generation,
        metadata.target,
        metadata.lap_count,
        metadata.player_count,
        metadata.tolerance_percent,
        metadata.timeout_ms,
    )
}

fn next_request_id(request_ids: &Rc<Cell<u64>>) -> u64 {
    let next = request_ids.get().wrapping_add(1);
    request_ids.set(next);
    next
}

// ──────────────────────────────────────────────────────────────────────────────
// Helper functions

/// Create a debounced callback that cancels any previous pending call
fn debounce_callback<T: 'static>(
    timer_handle: &UseStateHandle<Option<Timeout>>,
    callback: Callback<T>,
    value: T,
    delay_ms: u32,
) {
    // Cancel any existing timer by replacing it
    timer_handle.set(None);

    // Set new timer
    let timer_handle_clone = timer_handle.clone();
    let handle = Timeout::new(delay_ms, move || {
        callback.emit(value);
        // Clear the handle after execution
        timer_handle_clone.set(None);
    });
    timer_handle.set(Some(handle));
}

/// Helper to update cache version and trigger UI re-render
fn update_cache_version(cache_version: &UseStateHandle<usize>) {
    cache_version.set(cache_version.wrapping_add(1));
}

struct PrecacheConfig {
    cars: Vec<Car>,
    lap_count: usize,
    player_count: usize,
    timeout_secs: f64,
    tolerance_percent: f64,
}

#[derive(Clone)]
struct PrecacheExecutionContext {
    cache_version: UseStateHandle<usize>,
    error_count: UseStateHandle<usize>,
    failed_targets: UseStateHandle<Rc<Vec<u32>>>,
    dataset_generation: Rc<Cell<u64>>,
    expected_dataset_generation: u64,
    precache_generation: Rc<Cell<u64>>,
    expected_precache_generation: u64,
}

struct PrecacheJob {
    config: PrecacheConfig,
    context: PrecacheExecutionContext,
    request_ids: Rc<Cell<u64>>,
}

/// Process a single pre-cache target, ignoring a response after its generation
/// has been superseded. `Rc<Cell<_>>` remains live across Yew renders.
async fn process_precache_target(
    bridge: &mut (impl futures::Stream<Item = KarmaResult> + futures::Sink<KarmaArgs> + Unpin),
    args: KarmaArgs,
    context: &PrecacheExecutionContext,
) -> Result<(), ()> {
    let metadata = args.metadata.clone();
    if context.dataset_generation.get() != context.expected_dataset_generation
        || context.precache_generation.get() != context.expected_precache_generation
    {
        return Err(());
    }

    use futures::SinkExt;
    bridge.send(args).await.map_err(|_| ())?;
    let response = bridge.next().await.ok_or(())?;
    if context.dataset_generation.get() != context.expected_dataset_generation
        || context.precache_generation.get() != context.expected_precache_generation
    {
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
            CACHE_STORE.with(|c| {
                c.borrow_mut().insert(
                    cache_key(&metadata),
                    (success.sets, success.similarity, success.calculated_target),
                );
            });
            update_cache_version(&context.cache_version);
            Ok(())
        }
        Err(failure) if failure.metadata == metadata => {
            add_failed_target_marker(
                metadata.target,
                metadata.lap_count as u32,
                metadata.player_count as u32,
            );
            context.error_count.set(*context.error_count + 1);
            let mut failed = (*context.failed_targets).to_vec();
            failed.push(metadata.target);
            context.failed_targets.set(Rc::new(failed));
            Err(())
        }
        _ => Err(()),
    }
}

/// Run pre-cache for a range of targets.
fn run_precache(job: PrecacheJob) {
    let PrecacheJob {
        config,
        context,
        request_ids,
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
    let order = Rc::new(spread_indices(SLIDER_MAX_INDEX + 1));

    // Spawn task for each worker
    for worker_idx in 0..WORKER_COUNT {
        let cars = cars.clone();
        let context = context.clone();
        let request_ids = request_ids.clone();
        let order = order.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut bridge = <KarmaTask as Spawnable>::spawner().spawn(WORKER_SCRIPT);

            for pos in (worker_idx..order.len()).step_by(WORKER_COUNT) {
                let idx = order[pos];

                // Stop promptly when the dataset or pre-cache job changes.
                if context.dataset_generation.get() != context.expected_dataset_generation
                    || context.precache_generation.get() != context.expected_precache_generation
                {
                    return;
                }

                let target = (min + step * idx as u32).min(max);
                let metadata = RequestMetadata {
                    request_id: next_request_id(&request_ids),
                    dataset_generation: context.expected_dataset_generation,
                    target,
                    lap_count,
                    player_count,
                    timeout_ms: timeout_secs * 1000.0,
                    tolerance_percent,
                };
                let key = cache_key(&metadata);

                if CACHE_STORE.with(|c| c.borrow().contains_key(&key)) {
                    continue;
                }

                let args = KarmaArgs {
                    cars: cars.clone(),
                    metadata,
                };

                let _ = process_precache_target(&mut bridge, args, &context).await;
            }
        });
    }
}

// ──────────────────────────────────────────────────────────────────────────────

/// Primary application component wiring state, effects, and UI elements.
#[function_component(Main)]
fn main_component() -> Html {
    let csv_data = include_str!("cars.csv");
    let cars = use_state(Vec::<Car>::new);
    let target = use_state(|| DEFAULT_TARGET_MS);
    let lap_count = use_state(|| DEFAULT_LAP_COUNT);
    let player_count = use_state(|| DEFAULT_PLAYER_COUNT);
    let timeout_seconds = use_state(|| DEFAULT_TIMEOUT_SEC);
    let tolerance_percent = use_state(|| DEFAULT_TOLERANCE_PCT);

    // Text states for input fields
    let lap_count_text = use_state(|| DEFAULT_LAP_COUNT.to_string());
    let player_count_text = use_state(|| DEFAULT_PLAYER_COUNT.to_string());
    let target_text = use_state(|| format_ms_to_minsecms(DEFAULT_TARGET_MS));
    let timeout_seconds_text = use_state(|| DEFAULT_TIMEOUT_SEC.to_string());
    let tolerance_percent_text = use_state(|| DEFAULT_TOLERANCE_PCT.to_string());

    let results = use_state(|| None::<CacheValue>);
    let is_calculating = use_state(|| false);
    let error_message = use_state(|| None::<String>);
    // Cache version state triggers UI re-render when global cache changes
    let cache_version = use_state(|| 0usize);
    let precache_enabled = use_state(|| true);
    let last_from_cache = use_state(|| false);
    // Debounce timer handle - simplified to use UseStateHandle
    let debounce_timer = use_state(|| None::<Timeout>);
    // Live shared tokens let asynchronous work observe cancellation after a Yew render.
    let dataset_generation = use_state(|| Rc::new(Cell::new(0u64)));
    let precache_generation = use_state(|| Rc::new(Cell::new(0u64)));
    let request_ids = use_state(|| Rc::new(Cell::new(0u64)));
    let active_request = use_state(|| Rc::new(RefCell::new(None::<RequestMetadata>)));
    let debounce_precache = use_state(|| None::<Timeout>);
    // State to track pre-cache errors for the current parameters
    let precache_error_count = use_state(|| 0usize);
    // State to track the specific targets that failed pre-caching
    let precache_failed_targets = use_state(|| Rc::new(Vec::<u32>::new()));
    // Trigger to manually restart pre-cache (incremented to trigger effect)
    let precache_trigger = use_state(|| 0usize);
    // State to control cache settings visibility
    let cache_settings_visible = use_state(|| false);
    // subscription handle (identical to the prime example)
    let karma_sub = use_reactor_subscription::<KarmaTask>();
    let handled_idx = use_mut_ref(|| 0usize); // number of messages already processed

    // Remove worker_count state - use constant instead
    // slider index state (0..SLIDER_MAX_INDEX)
    let slider_idx = use_state(|| 0);
    let clipboard_feedback = use_state(|| None::<String>);
    let copy_feedback = use_state(|| None::<String>);

    // Text input validation states
    let lap_count_error = use_state(|| None::<String>);
    let player_count_error = use_state(|| None::<String>);
    let target_error = use_state(|| None::<String>);
    let timeout_error = use_state(|| None::<String>);
    let tolerance_error = use_state(|| None::<String>);

    // --- OnInput Handlers for Text States ---
    let lap_count_text_oninput = {
        let lap_count_text_setter = lap_count_text.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            lap_count_text_setter.set(input.value());
        })
    };
    let player_count_text_oninput = {
        let player_count_text_setter = player_count_text.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            player_count_text_setter.set(input.value());
        })
    };
    let target_text_oninput = {
        let target_text_setter = target_text.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            target_text_setter.set(input.value());
        })
    };
    let timeout_seconds_text_oninput = {
        let timeout_seconds_text_setter = timeout_seconds_text.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            timeout_seconds_text_setter.set(input.value());
        })
    };
    let tolerance_percent_text_oninput = {
        let tolerance_percent_text_setter = tolerance_percent_text.clone();
        Callback::from(move |e: InputEvent| {
            let input: HtmlInputElement = e.target_unchecked_into();
            tolerance_percent_text_setter.set(input.value());
        })
    };

    // Load cars from CSV on mount
    {
        let cars = cars.clone();
        use_effect_with((), move |_| {
            let loaded = read_cars_from_csv_string(csv_data).unwrap_or_default();
            cars.set(loaded);
        });
    }

    // Combine calculation logic into a single callback that reads current state
    let calculate = {
        let karma_sub = karma_sub.clone();
        let cars_state = cars.clone();
        let target_state = target.clone();
        let lap_count_state = lap_count.clone();
        let player_count_state = player_count.clone();
        let timeout_state = timeout_seconds.clone();
        let tolerance_state = tolerance_percent.clone();
        let last_from_cache = last_from_cache.clone();
        let results = results.clone();
        let error_message = error_message.clone();
        let is_calculating = is_calculating.clone();
        let dataset_generation = dataset_generation.clone();
        let request_ids = request_ids.clone();
        let active_request = active_request.clone();
        Callback::from(move |target_override: Option<u32>| {
            let target_to_use = target_override.unwrap_or(*target_state);
            let lap_count = *lap_count_state;
            let player_count = *player_count_state;
            let timeout_value = *timeout_state;
            let tolerance_value = *tolerance_state;

            is_calculating.set(true);

            let metadata = RequestMetadata {
                request_id: next_request_id(&request_ids),
                dataset_generation: dataset_generation.get(),
                target: target_to_use,
                lap_count,
                player_count,
                timeout_ms: timeout_value * 1000.0,
                tolerance_percent: tolerance_value,
            };
            let key = cache_key(&metadata);

            if let Some(cached) = CACHE_STORE.with(|c| c.borrow().get(&key).cloned()) {
                *active_request.borrow_mut() = None;
                last_from_cache.set(true);
                results.set(Some(cached));
                error_message.set(None);
                is_calculating.set(false);
                return;
            }

            *active_request.borrow_mut() = Some(metadata.clone());
            let args = KarmaArgs {
                cars: (*cars_state).clone(),
                metadata,
            };
            karma_sub.send(args);
            is_calculating.set(true);
        })
    };

    // keep slider_idx and target in sync when range changes
    {
        let slider_idx = slider_idx.clone();
        let target = target.clone();
        let cars = cars.clone();
        let lap_count = lap_count.clone();
        use_effect_with((*lap_count, *player_count), move |_| {
            let (min, max) = base_target_range(&cars, *lap_count);
            let clamped = calc_target_from_idx(min, max, *slider_idx);
            target.set(clamped);
            || ()
        });
    }

    // Automatically clamp target when cars are loaded or lap_count changes
    {
        let target = target.clone();
        let cars_state = (*cars).clone();
        use_effect_with(
            (cars_state.len(), *lap_count),
            move |&(cars_len, subset)| {
                let (min, max) = if cars_len > 0 {
                    get_target_range_for_subset(&cars_state, subset)
                } else {
                    (0, 0)
                };
                let val = *target;
                if val < min {
                    target.set(min);
                } else if val > max {
                    target.set(max);
                }
                || ()
            },
        );
    }

    // A change to every pre-cache input, including enabled state and dataset,
    // invalidates both queued and in-flight work through the live token.
    use_effect_with(
        (
            *lap_count,
            *player_count,
            cars.len(),
            *timeout_seconds,
            *tolerance_percent,
            *precache_enabled,
            *precache_trigger,
            dataset_generation.get(),
        ),
        {
            let cars = cars.clone();
            let precache_error_count = precache_error_count.clone();
            let precache_failed_targets = precache_failed_targets.clone();
            let cache_version = cache_version.clone();
            let precache_generation = precache_generation.clone();
            let dataset_generation = dataset_generation.clone();
            let request_ids = request_ids.clone();
            let debounce_precache = debounce_precache.clone();

            move |&(
                ss,
                nr,
                car_count,
                timeout_secs,
                tolerance_val,
                enabled,
                _trigger,
                dataset_id,
            )|
                  -> Box<dyn FnOnce()> {
                debounce_precache.set(None);
                let generation = precache_generation.get().wrapping_add(1);
                (*precache_generation).set(generation);

                if !enabled || car_count == 0 {
                    return Box::new(|| ());
                }

                precache_error_count.set(0);
                precache_failed_targets.set(Rc::new(Vec::new()));
                let handle = Timeout::new(0, move || {
                    run_precache(PrecacheJob {
                        config: PrecacheConfig {
                            cars: (*cars).clone(),
                            lap_count: ss,
                            player_count: nr,
                            timeout_secs,
                            tolerance_percent: tolerance_val,
                        },
                        context: PrecacheExecutionContext {
                            cache_version,
                            error_count: precache_error_count,
                            failed_targets: precache_failed_targets,
                            dataset_generation: (*dataset_generation).clone(),
                            expected_dataset_generation: dataset_id,
                            precache_generation: (*precache_generation).clone(),
                            expected_precache_generation: generation,
                        },
                        request_ids: (*request_ids).clone(),
                    });
                });
                debounce_precache.set(Some(handle));
                Box::new(|| ())
            }
        },
    );

    // Ensure re-render on cache updates by reading cache_version
    let _ = *cache_version;
    // Ensure re-render on error count updates
    let _ = *precache_error_count;
    // Ensure re-render on failed targets updates
    let _ = precache_failed_targets.len();
    // Prepare a cars handle clone for cache counting
    let cars_for_count = cars.clone();
    // Dynamic cache count for current Subset Size & Runs
    let cached_count = {
        let cars_vec = (*cars_for_count).clone();
        let ss = *lap_count;
        let nr = *player_count;
        let (min, max) = base_target_range(&cars_vec, ss);
        let step = base_target_step(min, max);
        let dataset_id = dataset_generation.get();
        let timeout_ms = *timeout_seconds * 1000.0;
        CACHE_STORE.with(|c| {
            (0..=SLIDER_MAX_INDEX)
                .filter(|idx| {
                    let metadata = RequestMetadata {
                        request_id: 0,
                        dataset_generation: dataset_id,
                        target: (min + step * *idx as u32).min(max),
                        lap_count: ss,
                        player_count: nr,
                        timeout_ms,
                        tolerance_percent: *tolerance_percent,
                    };
                    c.borrow().contains_key(&cache_key(&metadata))
                })
                .count()
        })
    };

    // (re-)initialise the chart on lap_count or player_count changes, and replay cache
    {
        let lap_handle = lap_count.clone();
        let player_handle = player_count.clone();
        let cars = cars.clone();
        let dataset_generation = dataset_generation.clone();
        let timeout_seconds = timeout_seconds.clone();
        let tolerance_percent = tolerance_percent.clone();
        use_effect_with(
            (
                *lap_count,
                *player_count,
                cars.len(),
                dataset_generation.get(),
                *timeout_seconds,
                *tolerance_percent,
                *cache_version,
            ),
            move |_| {
                let lap_count_val = *lap_handle as u32;
                let player_count_val = *player_handle as u32;
                let (min, max) = base_target_range(&cars, *lap_handle as usize);
                if max > min {
                    init_similarity_chart(min, max, lap_count_val, player_count_val);

                    // replay any existing cache entries for this lap_count/player_count
                    let ss = lap_count_val as usize;
                    let nr = player_count_val as usize;
                    CACHE_STORE.with(|c| {
                        let mut entries: Vec<(u32, f64)> = c
                            .borrow()
                            .iter()
                            .filter(|(key, _)| {
                                key.dataset_generation == dataset_generation.get()
                                    && key.lap_count == ss
                                    && key.player_count == nr
                                    && key.timeout_ms_bits == (*timeout_seconds * 1000.0).to_bits()
                                    && key.tolerance_percent_bits == (*tolerance_percent).to_bits()
                            })
                            .map(|(key, (_sets, sim, _calc))| (key.target_ms, *sim))
                            .collect();
                        entries.sort_by_key(|(t, _)| *t);
                        for (t, sim) in entries {
                            add_similarity_data(t, sim * 100.0, lap_count_val, player_count_val);
                        }
                    });
                }
                || ()
            },
        );
    }

    // effect that consumes new worker messages
    {
        let karma_sub_consumer = karma_sub.clone();
        let handled_idx = handled_idx.clone();
        let cache_version = cache_version.clone();
        let last_from_cache = last_from_cache.clone();
        let results = results.clone();
        let error_message = error_message.clone();
        let dataset_generation = dataset_generation.clone();
        let active_request = active_request.clone();
        let is_calculating_cb = is_calculating.clone();
        use_effect_with(karma_sub.len(), move |_| {
            let all = karma_sub_consumer.iter();
            let new_total = all.len();
            for msg in all.skip(*handled_idx.borrow()) {
                match msg.as_ref() {
                    Ok(success)
                        if success.metadata.dataset_generation == dataset_generation.get()
                            && active_request.borrow().as_ref() == Some(&success.metadata) =>
                    {
                        add_similarity_data(
                            success.calculated_target,
                            success.similarity * 100.0,
                            success.metadata.lap_count as u32,
                            success.metadata.player_count as u32,
                        );
                        CACHE_STORE.with(|c| {
                            c.borrow_mut().insert(
                                cache_key(&success.metadata),
                                (
                                    success.sets.clone(),
                                    success.similarity,
                                    success.calculated_target,
                                ),
                            );
                        });
                        update_cache_version(&cache_version);
                        *active_request.borrow_mut() = None;
                        last_from_cache.set(false);
                        results.set(Some((
                            success.sets.clone(),
                            success.similarity,
                            success.calculated_target,
                        )));
                        error_message.set(None);
                        is_calculating_cb.set(false);
                    }
                    Err(failure)
                        if failure.metadata.dataset_generation == dataset_generation.get()
                            && active_request.borrow().as_ref() == Some(&failure.metadata) =>
                    {
                        add_failed_target_marker(
                            failure.metadata.target,
                            failure.metadata.lap_count as u32,
                            failure.metadata.player_count as u32,
                        );
                        *active_request.borrow_mut() = None;
                        results.set(None);
                        error_message.set(Some(failure.error.clone()));
                        is_calculating_cb.set(false);
                    }
                    _ => {}
                }
            }
            *handled_idx.borrow_mut() = new_total;
            || ()
        });
    }

    // Simplified input handlers - no text state management
    let handle_lap_count_input = {
        let lap_count_text_handle = lap_count_text.clone();
        let lap_count_num_handle = lap_count.clone();
        let lap_count_err_handle = lap_count_error.clone();
        let cars_len = cars.len();
        let calculate = calculate.clone();
        let debounce_timer = debounce_timer.clone();

        Callback::from(move |_: ()| {
            let text_val = (*lap_count_text_handle).clone();
            match crate::utils::validate_lap_count(&text_val, cars_len) {
                Ok(valid_num) => {
                    lap_count_err_handle.set(None);
                    lap_count_num_handle.set(valid_num);
                    lap_count_text_handle.set(valid_num.to_string());
                    debounce_callback(&debounce_timer, calculate.clone(), None, DEBOUNCE_MS);
                }
                Err(e) => {
                    lap_count_err_handle.set(Some(e));
                }
            }
        })
    };

    let handle_player_count_input = {
        let player_count_text_handle = player_count_text.clone();
        let player_count_num_handle = player_count.clone();
        let player_count_err_handle = player_count_error.clone();
        let calculate = calculate.clone();
        let debounce_timer = debounce_timer.clone();

        Callback::from(move |_: ()| {
            let text_val = (*player_count_text_handle).clone();
            match crate::utils::validate_player_count(&text_val) {
                Ok(valid_num) => {
                    player_count_err_handle.set(None);
                    player_count_num_handle.set(valid_num);
                    player_count_text_handle.set(valid_num.to_string());
                    debounce_callback(&debounce_timer, calculate.clone(), None, DEBOUNCE_MS);
                }
                Err(e) => {
                    player_count_err_handle.set(Some(e));
                }
            }
        })
    };

    let handle_target_input = {
        let target_text_handle = target_text.clone();
        let target_num_handle = target.clone();
        let target_err_handle = target_error.clone();
        let slider_idx_handle = slider_idx.clone();
        let calculate = calculate.clone();
        let debounce_timer = debounce_timer.clone();
        let cars_handle = cars.clone();
        let lap_count_handle = lap_count.clone();

        Callback::from(move |_: ()| {
            let text_val = (*target_text_handle).clone();
            if text_val.trim().is_empty() {
                target_err_handle.set(None); // Allow empty commit to clear errors, but don't change target
                return;
            }
            match parse_time_to_ms(&text_val) {
                Ok(ms) => {
                    let (min, max) = base_target_range(&cars_handle, *lap_count_handle);
                    if ms < min || ms > max {
                        target_err_handle.set(Some(format!(
                            "Target must be between {} and {}",
                            format_ms_to_minsecms(min),
                            format_ms_to_minsecms(max)
                        )));
                    } else {
                        target_err_handle.set(None);
                        target_num_handle.set(ms);
                        target_text_handle.set(format_ms_to_minsecms(ms));

                        let range = max - min;
                        let pos = if range > 0 {
                            ((ms - min) as f64 / range as f64 * SLIDER_MAX_INDEX as f64).round()
                                as usize
                        } else {
                            0
                        };
                        slider_idx_handle.set(pos.min(SLIDER_MAX_INDEX));
                        debounce_callback(
                            &debounce_timer,
                            calculate.clone(),
                            Some(ms),
                            DEBOUNCE_MS,
                        );
                    }
                }
                Err(error) => {
                    target_err_handle.set(Some(error));
                }
            }
        })
    };

    let handle_timeout_input = {
        let timeout_text_handle = timeout_seconds_text.clone();
        let timeout_num_handle = timeout_seconds.clone();
        let timeout_err_handle = timeout_error.clone();

        Callback::from(move |_: ()| {
            let text_val = (*timeout_text_handle).clone();
            if text_val.trim().is_empty() {
                // Allow empty commit to clear errors
                timeout_err_handle.set(None);
                return;
            }
            match text_val.parse::<f64>() {
                Ok(v) => {
                    if (MIN_TIMEOUT_SEC..=MAX_TIMEOUT_SEC).contains(&v) {
                        timeout_err_handle.set(None);
                        timeout_num_handle.set(v);
                        timeout_text_handle.set(v.to_string());
                    } else {
                        timeout_err_handle.set(Some(format!(
                            "Timeout must be between {} and {} seconds",
                            MIN_TIMEOUT_SEC, MAX_TIMEOUT_SEC
                        )));
                    }
                }
                Err(_) => {
                    timeout_err_handle.set(Some("Invalid number".to_string()));
                }
            }
        })
    };

    let handle_tolerance_input = {
        let tolerance_text_handle = tolerance_percent_text.clone();
        let tolerance_num_handle = tolerance_percent.clone();
        let tolerance_err_handle = tolerance_error.clone();

        Callback::from(move |_: ()| {
            let text_val = (*tolerance_text_handle).clone();
            if text_val.trim().is_empty() {
                // Allow empty commit to clear errors
                tolerance_err_handle.set(None);
                return;
            }
            match text_val.parse::<f64>() {
                Ok(v) => {
                    if (MIN_TOLERANCE_PCT..=MAX_TOLERANCE_PCT).contains(&v) {
                        tolerance_err_handle.set(None);
                        tolerance_num_handle.set(v);
                        tolerance_text_handle.set(v.to_string());
                    } else {
                        tolerance_err_handle.set(Some(format!(
                            "Tolerance must be between {} and {}%",
                            MIN_TOLERANCE_PCT, MAX_TOLERANCE_PCT
                        )));
                    }
                }
                Err(_) => {
                    tolerance_err_handle.set(Some("Invalid number".to_string()));
                }
            }
        })
    };

    // --- KeyDown Handlers for Enter Key ---
    let lap_count_onkeydown = {
        let commit_handler = handle_lap_count_input.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                commit_handler.emit(());
            }
        })
    };
    let player_count_onkeydown = {
        let commit_handler = handle_player_count_input.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                commit_handler.emit(());
            }
        })
    };
    let target_onkeydown = {
        let commit_handler = handle_target_input.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                commit_handler.emit(());
            }
        })
    };
    let timeout_onkeydown = {
        let commit_handler = handle_timeout_input.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                commit_handler.emit(());
            }
        })
    };
    let tolerance_onkeydown = {
        let commit_handler = handle_tolerance_input.clone();
        Callback::from(move |e: KeyboardEvent| {
            if e.key() == "Enter" {
                commit_handler.emit(());
            }
        })
    };

    // --- Synchronization Effects (Numeric State -> Text State) ---
    {
        // Sync lap_count -> lap_count_text
        let num_val = *lap_count;
        let text_setter = lap_count_text.clone();
        let error_setter = lap_count_error.clone();
        use_effect_with(num_val, move |&current_num_val| {
            let num_as_string = current_num_val.to_string();
            if *text_setter != num_as_string {
                text_setter.set(num_as_string);
                error_setter.set(None); // If synced from num, assume valid or error handled elsewhere
            }
            || ()
        });
    }
    {
        // Sync player_count -> player_count_text
        let num_val = *player_count;
        let text_setter = player_count_text.clone();
        let error_setter = player_count_error.clone();
        use_effect_with(num_val, move |&current_num_val| {
            let num_as_string = current_num_val.to_string();
            if *text_setter != num_as_string {
                text_setter.set(num_as_string);
                error_setter.set(None);
            }
            || ()
        });
    }
    {
        // Sync target -> target_text
        let num_val = *target;
        let text_setter = target_text.clone();
        let error_setter = target_error.clone();
        use_effect_with(num_val, move |&current_num_val| {
            let num_as_string = format_ms_to_minsecms(current_num_val);
            if *text_setter != num_as_string {
                text_setter.set(num_as_string);
                error_setter.set(None);
            }
            || ()
        });
    }
    {
        // Sync timeout_seconds -> timeout_seconds_text
        let num_val = *timeout_seconds;
        let text_setter = timeout_seconds_text.clone();
        let error_setter = timeout_error.clone();
        use_effect_with(num_val, move |&current_num_val| {
            let num_as_string = current_num_val.to_string();
            if *text_setter != num_as_string {
                text_setter.set(num_as_string);
                error_setter.set(None);
            }
            || ()
        });
    }
    {
        // Sync tolerance_percent -> tolerance_percent_text
        let num_val = *tolerance_percent;
        let text_setter = tolerance_percent_text.clone();
        let error_setter = tolerance_error.clone();
        use_effect_with(num_val, move |&current_num_val| {
            let num_as_string = current_num_val.to_string();
            if *text_setter != num_as_string {
                text_setter.set(num_as_string);
                error_setter.set(None);
            }
            || ()
        });
    }

    let handle_paste_from_clipboard = {
        let cars_setter = cars.clone();
        let feedback_setter = clipboard_feedback.clone();
        let results = results.clone();
        let error_message = error_message.clone();
        let is_calculating = is_calculating.clone();
        let active_request = active_request.clone();
        let dataset_generation = dataset_generation.clone();
        let precache_generation = precache_generation.clone();
        let cache_version = cache_version.clone();

        Callback::from(move |_: MouseEvent| {
            let cars_setter = cars_setter.clone();
            let feedback_setter = feedback_setter.clone();
            let results = results.clone();
            let error_message = error_message.clone();
            let is_calculating = is_calculating.clone();
            let active_request = active_request.clone();
            let dataset_generation = dataset_generation.clone();
            let precache_generation = precache_generation.clone();
            let cache_version = cache_version.clone();

            wasm_bindgen_futures::spawn_local(async move {
                let window = web_sys::window().expect("no global `window` exists");
                let navigator = window.navigator();
                let clipboard = navigator.clipboard();

                match wasm_bindgen_futures::JsFuture::from(clipboard.read_text()).await {
                    Ok(text) => {
                        if let Some(text_str) = text.as_string() {
                            if text_str.trim().is_empty() {
                                feedback_setter.set(Some("Clipboard is empty.".to_string()));
                                return;
                            }
                            match read_cars_from_csv_string(&text_str) {
                                Ok(new_cars) => {
                                    if new_cars.is_empty() {
                                        feedback_setter.set(Some(
                                            "No valid car data found in clipboard content."
                                                .to_string(),
                                        ));
                                    } else {
                                        let car_count = new_cars.len();
                                        // New rows invalidate every old index and all in-flight work.
                                        (*dataset_generation)
                                            .set(dataset_generation.get().wrapping_add(1));
                                        (*precache_generation)
                                            .set(precache_generation.get().wrapping_add(1));
                                        *active_request.borrow_mut() = None;
                                        CACHE_STORE.with(|c| c.borrow_mut().clear());
                                        update_cache_version(&cache_version);
                                        results.set(None);
                                        error_message.set(None);
                                        is_calculating.set(false);
                                        cars_setter.set(new_cars);
                                        feedback_setter.set(Some(format!(
                                            "Successfully loaded {} cars from clipboard.",
                                            car_count
                                        )));
                                    }
                                }
                                Err(e) => {
                                    feedback_setter.set(Some(format!(
                                        "Failed to parse clipboard content: {}",
                                        e
                                    )));
                                }
                            }
                        } else {
                            feedback_setter.set(Some("Failed to read clipboard text.".to_string()));
                        }
                    }
                    Err(_) => {
                        feedback_setter.set(Some(
                            "Failed to read from clipboard. Check browser permissions.".to_string(),
                        ));
                    }
                }
            });
        })
    };

    let handle_copy_results_to_clipboard = {
        let cars = cars.clone();
        let results = results.clone();
        let feedback_setter = copy_feedback.clone();

        Callback::from(move |_: MouseEvent| {
            let feedback_setter = feedback_setter.clone();
            let cars = cars.clone();
            let results = results.clone();

            wasm_bindgen_futures::spawn_local(async move {
                if let Some((result_sets, _, _)) = results.as_ref() {
                    if result_sets.is_empty() {
                        feedback_setter.set(Some("No results to copy.".to_string()));
                        return;
                    }

                    let csv_content = result_sets
                        .iter()
                        .filter_map(|car_indices| {
                            let row = car_indices
                                .iter()
                                .filter_map(|&index| cars.get(index).map(|car| car.id.clone()))
                                .collect::<Vec<String>>();
                            (!row.is_empty()).then(|| row.join(","))
                        })
                        .collect::<Vec<String>>()
                        .join("\n");
                    if csv_content.is_empty() {
                        feedback_setter
                            .set(Some("Results contain no valid cars to copy.".to_string()));
                        return;
                    }

                    let window = web_sys::window().expect("no global `window` exists");
                    let navigator = window.navigator();
                    match wasm_bindgen_futures::JsFuture::from(
                        navigator.clipboard().write_text(&csv_content),
                    )
                    .await
                    {
                        Ok(_) => {
                            feedback_setter.set(Some("Results copied to clipboard!".to_string()));
                        }
                        Err(_) => {
                            feedback_setter
                                .set(Some("Failed to copy. Check permissions.".to_string()));
                        }
                    }
                } else {
                    feedback_setter.set(Some("No results available to copy.".to_string()));
                }
            });
        })
    };

    html! {
        <div class="container">
            <h1>{ "Random Karma Configuration" }</h1>

            <div class="top-controls">
                <div class="form-group">
                    <label for="lap_count_text_input">{ "Lap Count:" }</label>
                    <div class="slider-with-value">
                        <input type="range"
                            min="1"
                            max={cars.len().to_string()}
                            value={lap_count.to_string()}
                            oninput={
                                let lap_count_setter = lap_count.clone();
                                let calculate = calculate.clone();
                                let debounce_timer = debounce_timer.clone();
                                let active_request = active_request.clone();
                                let results = results.clone();
                                let error_message = error_message.clone();
                                Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    if let Ok(val) = input.value().parse::<usize>() {
                                        lap_count_setter.set(val);
                                        *active_request.borrow_mut() = None;
                                        results.set(None);
                                        error_message.set(None);
                                        debounce_callback(&debounce_timer, calculate.clone(), None, DEBOUNCE_MS);
                                    }
                                })
                            }
                        />
                        // Custom input for lap_count
                        <input
                            type="number"
                            id="lap_count_text_input"
                            min="1"
                            max={cars.len().to_string()}
                            value={(*lap_count_text).clone()}
                            class={if (*lap_count_error).is_some() { "invalid" } else { "" }}
                            oninput={lap_count_text_oninput}
                            onchange={handle_lap_count_input.reform(|_|())}
                            onkeydown={lap_count_onkeydown}
                        />
                        if let Some(ref err) = *lap_count_error {
                            <div class="input-error">{ err }</div>
                        }
                    </div>
                    <span class="slider-info">{ format!("Max: {}", cars.len()) }</span>
                </div>

                <div class="form-group">
                    <label for="player_count_text_input">{ "Player Count:" }</label>
                    <div class="slider-with-value">
                        <input type="range"
                            min="0" // Assuming min player count is 0 or 1, adjust if needed
                            max={MAX_PLAYER_COUNT.to_string()}
                            value={player_count.to_string()}
                            oninput={
                                let player_count_setter = player_count.clone();
                                let calculate = calculate.clone();
                                let debounce_timer = debounce_timer.clone();
                                let active_request = active_request.clone();
                                let results = results.clone();
                                let error_message = error_message.clone();
                                Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    if let Ok(val) = input.value().parse::<usize>() {
                                        player_count_setter.set(val);
                                        *active_request.borrow_mut() = None;
                                        results.set(None);
                                        error_message.set(None);
                                        debounce_callback(&debounce_timer, calculate.clone(), None, DEBOUNCE_MS);
                                    }
                                })
                            }
                        />
                        // Custom input for player_count
                        <input
                            type="number"
                            id="player_count_text_input"
                            min="0"
                            max={MAX_PLAYER_COUNT.to_string()}
                            value={(*player_count_text).clone()}
                            class={if (*player_count_error).is_some() { "invalid" } else { "" }}
                            oninput={player_count_text_oninput}
                            onchange={handle_player_count_input.reform(|_|())}
                            onkeydown={player_count_onkeydown}
                        />
                        if let Some(ref err) = *player_count_error {
                            <div class="input-error">{ err }</div>
                        }
                    </div>
                </div>
            </div>

            // Chart section (full width)
            <div class="chart-section">
                <canvas id="similarityChart"></canvas>
            </div>

            // Target Time slider (full width, aligned with chart)
            <div class="target-slider-section">
                <div class="target-slider-container">
                    <div class="form-group">
                        <label for="target_text_input">{ "Target Time:" }</label>
                        <div class="slider-with-value">
                            <input type="range"
                                min={base_target_range(&cars, *lap_count).0.to_string()}
                                max={base_target_range(&cars, *lap_count).1.to_string()}
                                value={target.to_string()}
                                class="target-slider"
                                oninput={
                                    let target_setter = target.clone();
                                    let slider_idx_setter = slider_idx.clone();
                                    let cars_clone = cars.clone();
                                    let lap_count_clone = lap_count.clone();
                                    let calculate_cb = calculate.clone();
                                    let debounce_timer_cb = debounce_timer.clone();
                                    let active_request = active_request.clone();
                                    let results = results.clone();
                                    let error_message = error_message.clone();

                                    Callback::from(move |e: InputEvent| {
                                        let input: HtmlInputElement = e.target_unchecked_into();
                                        if let Ok(val) = input.value().parse::<u32>() {
                                            target_setter.set(val);
                                            *active_request.borrow_mut() = None;
                                            results.set(None);
                                            error_message.set(None);
                                            // Update slider_idx based on new target value
                                            let (min_target, max_target) = base_target_range(&cars_clone, *lap_count_clone);
                                            let range = max_target - min_target;
                                            let pos = if range > 0 {
                                                ((val - min_target) as f64 / range as f64 * SLIDER_MAX_INDEX as f64).round() as usize
                                            } else { 0 };
                                            slider_idx_setter.set(pos.min(SLIDER_MAX_INDEX));
                                            debounce_callback(&debounce_timer_cb, calculate_cb.clone(), Some(val), DEBOUNCE_MS);
                                        }
                                    })
                                }
                            />
                            // Custom input for target_text
                            <input
                                type="text"
                                id="target_text_input"
                                value={(*target_text).clone()}
                                class={format!("slider-value {}", if (*target_error).is_some() { "invalid" } else { "" })}
                                placeholder="MM:SS.mmm"
                                oninput={target_text_oninput}
                                onchange={handle_target_input.reform(|_|())}
                                onkeydown={target_onkeydown}
                            />
                        </div>
                        if let Some(ref error) = *target_error {
                            <div class="input-error">{ error }</div>
                        }
                    </div>
                </div>
            </div>
            // Settings section (collapsible)
            <div class="settings-section">
                <div class="settings-header">
                    <button class="settings-toggle"
                        aria-expanded={(*cache_settings_visible).to_string()}
                        onclick={
                            let cache_settings_visible = cache_settings_visible.clone();
                            Callback::from(move |_: MouseEvent| {
                                cache_settings_visible.set(!*cache_settings_visible);
                            })
                        }
                    >
                        <span class="settings-icon">
                            <svg xmlns="http://www.w3.org/2000/svg" fill="none" viewBox="0 0 24 24" stroke-width="1.5" stroke="currentColor" width="1.25rem" height="1.25rem">
                                <path stroke-linecap="round" stroke-linejoin="round" d="M9.594 3.94c.09-.542.56-.94 1.11-.94h2.593c.55 0 1.02.398 1.11.94l.213 1.281c.063.374.313.686.645.87.074.04.147.083.22.127.325.196.72.257 1.075.124l1.217-.456a1.125 1.125 0 0 1 1.37.49l1.296 2.247a1.125 1.125 0 0 1-.26 1.431l-1.003.827c-.293.241-.438.613-.43.992a7.723 7.723 0 0 1 0 .255c-.008.378.137.75.43.991l1.004.827c.424.35.534.955.26 1.43l-1.298 2.247a1.125 1.125 0 0 1-1.369.491l-1.217-.456c-.355-.133-.75-.072-1.076.124a6.47 6.47 0 0 1-.22.128c-.331.183-.581.495-.644.869l-.213 1.281c-.09.543-.56.94-1.11.94h-2.594c-.55 0-1.019-.398-1.11-.94l-.213-1.281c-.062-.374-.312-.686-.644-.87a6.52 6.52 0 0 1-.22-.127c-.325-.196-.72-.257-1.076-.124l-1.217.456a1.125 1.125 0 0 1-1.369-.49l-1.297-2.247a1.125 1.125 0 0 1 .26-1.431l1.004-.827c.292-.24.437-.613.43-.991a6.932 6.932 0 0 1 0-.255c.007-.38-.138-.751-.43-.992l-1.004-.827a1.125 1.125 0 0 1-.26-1.43l1.297-2.247a1.125 1.125 0 0 1 1.37-.491l1.216.456c.356.133.751.072 1.076-.124.072-.044.146-.086.22-.128.332-.183.582-.495.644-.869l.214-1.28Z" />
                                <path stroke-linecap="round" stroke-linejoin="round" d="M15 12a3 3 0 1 1-6 0 3 3 0 0 1 6 0Z" />
                            </svg>
                        </span>
                        <span class="settings-title">{ "Settings" }</span>
                        <span class="settings-chevron"></span>
                    </button>

                    // Show compact cache status when collapsed
                    if !*cache_settings_visible {
                        <div class="settings-summary">
                            <span class="cache-summary">{ format!("Cache: {}/{}", cached_count, SLIDER_MAX_INDEX + 1) }</span>
                            <span class="precache-indicator"></span>
                        </div>
                    }
                </div>

                if *cache_settings_visible {
                    <div class="settings-content">
                        <div class="clipboard-import-section">
                             <button onclick={handle_paste_from_clipboard} class="button-primary">
                                { "Paste Car Data from Clipboard" }
                            </button>
                            if let Some(feedback) = &*clipboard_feedback {
                                <div class="clipboard-feedback">{ feedback }</div>
                            }
                        </div>
                        <div class="form-group checkbox-group">
                            <label>
                                <input type="checkbox"
                                    checked={*precache_enabled}
                                    onchange={
                                        let precache_enabled = precache_enabled.clone();
                                        Callback::from(move |e: Event| {
                                            let input: web_sys::HtmlInputElement = e.target_unchecked_into();
                                            precache_enabled.set(input.checked());
                                        })
                                    }
                                />
                                { "Enable Pre-caching" }
                            </label>
                        </div>

                        <div class="form-row">
                            <div class="form-group">
                                <label for="timeout_seconds_text_input">{ "Calculation Timeout (seconds):" }</label>
                                // Custom input for timeout_seconds
                                <input
                                    type="number"
                                    id="timeout_seconds_text_input"
                                    step="0.1"
                                    min={MIN_TIMEOUT_SEC.to_string()}
                                    max={MAX_TIMEOUT_SEC.to_string()}
                                    value={(*timeout_seconds_text).clone()}
                                    class={if (*timeout_error).is_some() { "invalid" } else { "" }}
                                    placeholder={DEFAULT_TIMEOUT_SEC.to_string()}
                                    oninput={timeout_seconds_text_oninput}
                                    onchange={handle_timeout_input.reform(|_|())}
                                    onkeydown={timeout_onkeydown}
                                />
                                if let Some(ref err) = *timeout_error {
                                    <div class="input-error">{ err }</div>
                                }
                            </div>

                            <div class="form-group">
                                <label for="tolerance_percent_text_input">{ "Tolerance Threshold (%):" }</label>
                                // Custom input for tolerance_percent
                                <input
                                    type="number"
                                    id="tolerance_percent_text_input"
                                    step="0.1"
                                    min={MIN_TOLERANCE_PCT.to_string()}
                                    max={MAX_TOLERANCE_PCT.to_string()}
                                    value={(*tolerance_percent_text).clone()}
                                    class={if (*tolerance_error).is_some() { "invalid" } else { "" }}
                                    placeholder={DEFAULT_TOLERANCE_PCT.to_string()}
                                    oninput={tolerance_percent_text_oninput}
                                    onchange={handle_tolerance_input.reform(|_|())}
                                    onkeydown={tolerance_onkeydown}
                                />
                                if let Some(ref err) = *tolerance_error {
                                    <div class="input-error">{ err }</div>
                                }
                            </div>
                        </div>

                        <div class="cache-stats">
                    <div class="cache-status compact">
                        { format!("Cache: {}/{} calculations", cached_count, SLIDER_MAX_INDEX + 1) }
                    </div>

                    <div class="cache-status-global compact">
                        { format!("Total entries: {}", CACHE_STORE.with(|c| c.borrow().len())) }
                    </div>

                    <button class="btn-secondary small"
                        onclick={
                            let cache_version = cache_version.clone();
                            let lap_count = *lap_count; // Capture value, not state handle
                            let player_count = *player_count; // Capture value, not state handle
                            let cars = cars.clone(); // Clone the handle
                            let precache_enabled = *precache_enabled; // Capture value
                            let precache_trigger = precache_trigger.clone();
                            Callback::from(move |_: MouseEvent| {
                                CACHE_STORE.with(|c| c.borrow_mut().clear());
                                update_cache_version(&cache_version);

                                // Re-initialize the chart to clear any cached data points
                                let (min, max) = base_target_range(&cars, lap_count);
                                if max > min {
                                    init_similarity_chart(min, max, lap_count as u32, player_count as u32);
                                }

                                // Trigger pre-cache again if it's enabled
                                if precache_enabled {
                                    precache_trigger.set(*precache_trigger + 1);
                                }
                            })
                        }
                    >
                        { "Clear Cache" }
                    </button>
                </div>

                        if let Some(err) = &*error_message {
                            <div class="current-error compact">
                                { err }
                            </div>
                        }
                    </div>
                }
            </div>

            // Results section
            <div class="results-section">
                if *is_calculating {
                    <div class="loading-indicator">{ "Calculating..." }</div>
                } else if let Some(ref error) = *error_message {
                    <div class="error-message">{ error }</div>
                } else if let Some((sets, sim, calc_target)) = &*results {
                    <div class="results-header">
                        <button onclick={handle_copy_results_to_clipboard} class="button-secondary">
                            { "Copy Results as CSV" }
                        </button>
                        if let Some(feedback) = &*copy_feedback {
                            <div class="copy-feedback">{ feedback }</div>
                        }
                    </div>
                    <ResultsWrapper
                        cars={Rc::new((*cars).clone())}
                        all_results={Rc::new(sets.clone())}
                        similarity={*sim}
                        calculated_target={*calc_target}
                    />
                } else {
                    <div class="no-results-placeholder">{ "Select parameters and find karma" }</div>
                }
            </div>
        </div>
    }
}

/// App wrapper providing ReactorProvider for KarmaTask.
#[function_component]
pub fn App() -> Html {
    html! {
        <ReactorProvider<KarmaTask> path="worker.js">
            <Main />
        </ReactorProvider<KarmaTask>>
    }
}

/// Entry point: initializes Yew renderer for the App component.
fn main() {
    yew::Renderer::<App>::new().render();
}
