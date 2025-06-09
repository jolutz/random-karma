//! Main module for Random Karma application using Yew.
//! Wires UI components, state hooks, and side-effect logic.

use futures::StreamExt;
use gloo_timers::callback::Timeout;
use random_karma::{
    get_target_range_for_subset,
    read_cars_from_csv_string,
    format_ms_to_minsecms,
    worker_agent::{KarmaArgs, KarmaTask},
    Car,
};
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

use cache::CACHE_STORE;
use chart::{add_failed_target_marker, add_similarity_data, init_similarity_chart};
use components::render_results;
use config::*; // This will bring SLIDER_MAX_INDEX and other config constants into scope
use utils::{
    base_target_range,
    base_target_step,
    calc_cached_count,
    calc_target_from_idx,
    parse_time_to_ms,
    spread_indices,
};

// ──────────────────────────────────────────────────────────────────────────────
// Type aliases for better readability
type CacheKey = (u32, usize, usize); // (target_ms, lap_count, player_count)
type CacheValue = (Vec<Vec<usize>>, f64, u32); // (results, similarity, calculated_target)

// Get optimal worker count once
const WORKER_COUNT: usize = {
    #[cfg(target_arch = "wasm32")]
    {
        4 // Default for WASM since we can't call web APIs in const context
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4)
    }
};

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

/// Process a single pre-cache target
async fn process_precache_target(
    bridge: &mut (impl futures::Stream<Item = Result<(Vec<Vec<usize>>, f64, u32, usize, usize), String>>
              + futures::Sink<KarmaArgs>
              + Unpin),
    args: KarmaArgs,
    cache_version: UseStateHandle<usize>,
    precache_error_count: UseStateHandle<usize>,
    precache_failed_targets: UseStateHandle<Rc<Vec<u32>>>,
) -> Result<(), ()> {
    let target_val = args.target;
    let ss = args.lap_count;
    let nr = args.player_count;

    use futures::SinkExt;
    bridge.send(args).await.map_err(|_| ())?;

    match bridge
        .next()
        .await
        .unwrap_or_else(|| Err("worker closed".into()))
    {
        Ok((res, sim, calc_target, ss_ret, nr_ret)) => {
            add_similarity_data(calc_target, sim * 100.0, ss as u32, nr as u32);
            CACHE_STORE.with(|c| {
                c.borrow_mut()
                    .insert((calc_target, ss_ret, nr_ret), (res, sim, calc_target));
            });
            update_cache_version(&cache_version);
            Ok(())
        }
        Err(_) => {
            add_failed_target_marker(target_val, ss as u32, nr as u32);
            precache_error_count.set(*precache_error_count + 1);
            let mut failed = (*precache_failed_targets).to_vec();
            failed.push(target_val);
            precache_failed_targets.set(Rc::new(failed));
            Err(())
        }
    }
}

/// Run pre-cache for a range of targets
fn run_precache(
    cars: Vec<Car>,
    ss: usize,
    nr: usize,
    timeout_secs: f64,
    tolerance_val: f64,
    cache_version: UseStateHandle<usize>,
    precache_error_count: UseStateHandle<usize>,
    precache_failed_targets: UseStateHandle<Rc<Vec<u32>>>,
    current_token: u32,
    token_ref: UseStateHandle<u32>,
) {
    let (min, max) = get_target_range_for_subset(&cars, ss);
    let step = base_target_step(min, max);
    let order = Rc::new(spread_indices(SLIDER_MAX_INDEX + 1));

    // Spawn task for each worker
    for worker_idx in 0..WORKER_COUNT {
        let cars_loop = cars.clone();
        let token_ref = token_ref.clone();
        let cache_version = cache_version.clone();
        let order = order.clone();
        let precache_error_count = precache_error_count.clone();
        let precache_failed_targets = precache_failed_targets.clone();

        wasm_bindgen_futures::spawn_local(async move {
            let mut bridge = <KarmaTask as Spawnable>::spawner().spawn(WORKER_SCRIPT);

            for pos in (worker_idx..order.len()).step_by(WORKER_COUNT) {
                let idx = order[pos];

                // Check if we should stop
                if *token_ref != current_token {
                    return;
                }

                let target_val = (min + step * idx as u32).min(max);
                let key: CacheKey = (target_val, ss, nr);

                // Skip if already cached
                if CACHE_STORE.with(|c| c.borrow().contains_key(&key)) {
                    continue;
                }

                let args = KarmaArgs {
                    cars: cars_loop.clone(),
                    target: target_val,
                    lap_count: ss,
                    player_count: nr,
                    timeout_ms: timeout_secs * 1000.0,
                    tolerance_percent: tolerance_val,
                };

                let _ = process_precache_target(
                    &mut bridge,
                    args,
                    cache_version.clone(),
                    precache_error_count.clone(),
                    precache_failed_targets.clone(),
                )
                .await;
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
    // Pre-cache control state - using simple counter instead of mutable ref
    let precache_token = use_state(|| 0u32);
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
    let slider_idx = use_state(|| 0usize);

    // Validation error states only (no text states or dirty flags)
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
            let loaded = read_cars_from_csv_string(csv_data, 1, 3, 4).unwrap_or_default();
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
        Callback::from(move |target_override: Option<u32>| {
            let target_to_use = target_override.unwrap_or(*target_state);
            let lap_count = *lap_count_state;
            let player_count = *player_count_state;
            let timeout_value = *timeout_state;
            let tolerance_value = *tolerance_state;

            is_calculating.set(true);

            let key: CacheKey = (target_to_use, lap_count, player_count);

            if let Some(cached) = CACHE_STORE.with(|c| c.borrow().get(&key).cloned()) {
                last_from_cache.set(true);
                results.set(Some(cached));
                error_message.set(None);
                is_calculating.set(false);
                return;
            }

            let args = KarmaArgs {
                cars: (*cars_state).clone(),
                target: target_to_use,
                lap_count,
                player_count,
                timeout_ms: timeout_value * 1000.0,
                tolerance_percent: tolerance_value,
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

    // Debounced pre-cache effect - simplified
    use_effect_with(
        (
            *lap_count,
            *player_count,
            cars.len(),
            *timeout_seconds,
            *tolerance_percent,
            *precache_trigger,
        ),
        {
            let cars = cars.clone();
            let precache_enabled = precache_enabled.clone();
            let precache_error_count = precache_error_count.clone();
            let precache_failed_targets = precache_failed_targets.clone();
            let cache_version = cache_version.clone();
            let precache_token = precache_token.clone();
            let debounce_precache = debounce_precache.clone();

            move |&(ss, nr, car_count, timeout_secs, tolerance_val, _trigger)| -> Box<dyn FnOnce()> {
                if !*precache_enabled || car_count == 0 {
                    return Box::new(|| ());
                }

                // Cancel pending timer and bump token
                debounce_precache.set(None);
                precache_token.set(*precache_token + 1);
                let current_token = *precache_token;

                // Reset error count and failed targets for the new parameters
                precache_error_count.set(0);
                precache_failed_targets.set(Rc::new(Vec::new()));

                // Run pre-cache immediately
                let handle = Timeout::new(0, move || {
                    run_precache(
                        (*cars).clone(),
                        ss,
                        nr,
                        timeout_secs,
                        tolerance_val,
                        cache_version,
                        precache_error_count,
                        precache_failed_targets,
                        current_token,
                        precache_token,
                    );
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
        calc_cached_count(min, max, step, ss, nr)
    };

    // (re-)initialise the chart on lap_count or player_count changes, and replay cache
    {
        let lap_handle = lap_count.clone();
        let player_handle = player_count.clone();
        let cars = cars.clone();
        use_effect_with((*lap_count, *player_count, cars.len()), move |&_| {
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
                        .filter(|((_, s, r), _)| *s == ss && *r == nr)
                        .map(|((t, _, _), (_sets, sim, _calc))| (*t, *sim))
                        .collect();
                    entries.sort_by_key(|(t, _)| *t);
                    for (t, sim) in entries {
                        add_similarity_data(t, sim * 100.0, lap_count_val, player_count_val);
                    }
                });
            }
            || ()
        });
    }

    // effect that consumes new worker messages
    {
        let karma_sub_consumer = karma_sub.clone();
        let handled_idx = handled_idx.clone();
        let lap_count = lap_count.clone();
        let player_count = player_count.clone();
        let cache_version = cache_version.clone();
        let last_from_cache = last_from_cache.clone();
        let results = results.clone();
        let error_message = error_message.clone();
        let target_state = target.clone();
        let is_calculating_cb = is_calculating.clone();
        use_effect_with(karma_sub.len(), move |_| {
            let all = karma_sub_consumer.iter();
            let new_total = all.len();
            for msg in all.skip(*handled_idx.borrow()) {
                match msg.as_ref() {
                    Ok((sets, sim, calc_target, ss_ret, nr_ret)) => {
                        // only plot if the message belongs to the *current* subset/runs
                        if *ss_ret == *lap_count && *nr_ret == *player_count {
                            add_similarity_data(
                                *calc_target,
                                *sim * 100.0,
                                *lap_count as u32,
                                *player_count as u32,
                            );
                        }

                        let key: CacheKey = (*calc_target, *ss_ret, *nr_ret);
                        CACHE_STORE.with(|c| {
                            c.borrow_mut()
                                .insert(key, (sets.clone(), *sim, *calc_target));
                        });
                        update_cache_version(&cache_version);

                        // show the result only if it matches the current selection
                        if *ss_ret == *lap_count
                            && *nr_ret == *player_count
                            && *calc_target == *target_state
                        {
                            last_from_cache.set(false);
                            results.set(Some((sets.clone(), *sim, *calc_target)));
                            error_message.set(None);
                            is_calculating_cb.set(false);
                        }
                    }
                    Err(e) => {
                        add_failed_target_marker(
                            *target_state,
                            *lap_count as u32,
                            *player_count as u32,
                        );
                        results.set(None);
                        error_message.set(Some(e.clone()));
                        is_calculating_cb.set(false);
                    }
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

    html! {
        <div class="container">
            <h1>{ "Random Karma Configuration" }</h1>

            // Top Control Bar section - Lap Count & Player Count
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
                                Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    if let Ok(val) = input.value().parse::<usize>() {
                                        lap_count_setter.set(val);
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
                                Callback::from(move |e: InputEvent| {
                                    let input: HtmlInputElement = e.target_unchecked_into();
                                    if let Ok(val) = input.value().parse::<usize>() {
                                        player_count_setter.set(val);
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

                                    Callback::from(move |e: InputEvent| {
                                        let input: HtmlInputElement = e.target_unchecked_into();
                                        if let Ok(val) = input.value().parse::<u32>() {
                                            target_setter.set(val);
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
                            Callback::from(move |_| {
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
                            Callback::from(move |_| {
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
            <div class="results-area">
                if let Some((ref all_results, similarity, calculated_target)) = *results {
                    { render_results(&cars,
                                    all_results,
                                    similarity,
                                    calculated_target) }
                } else if !*is_calculating {
                    <div class="no-results-message">
                        <p>{ "Adjust parameters and wait for calculation results." }</p>
                    </div>
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
