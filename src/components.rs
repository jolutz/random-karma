//! Pure Yew view components for the Random Karma UI.
//!
//! This module contains stateless components that render based on props,
//! making them easy to test and reuse.

use crate::format_ms_to_minsecms; // Assuming format_ms_to_minsecms is pub in lib.rs
use crate::Car;
use std::rc::Rc;
use yew::prelude::*;

/// Calculate total time for a subset
fn calculate_total_time(cars: &[Car], indices: &[usize]) -> u32 {
    indices.iter().map(|&i| cars[i].lap_time).sum()
}

/// Calculate percentage difference from target
fn calculate_percentage_diff(actual: u32, target: u32) -> f64 {
    let diff = actual as i64 - target as i64;
    (diff as f64 / target as f64) * 100.0
}

/// Renders the table with all calculated car selections.
///
/// Displays each subset as a row, showing:
/// - Set number
/// - Total time for the subset
/// - Percentage deviation from target
/// - Individual cars in the subset
pub fn render_results(
    cars: &[Car],
    all_results: &[Vec<usize>],
    similarity: f64,
    calculated_target: u32,
) -> Html {
    // Early return for empty results
    if all_results.is_empty() {
        return html! {
            <div class="results">
                <p class="no-results-message">{ "No results to display" }</p>
            </div>
        };
    }

    let subset_size = all_results.first().map(|s| s.len()).unwrap_or(0);

    html! {
        <div class="results">
            <div class="similarity-status">
                { format!("Jaccard Similarity: {:.2}%", similarity * 100.0) }
            </div>
            <div class="result-sets">
                <h3>{ "All Car Selections" }</h3>
                <div class="big-car-table-container">
                    <table class="big-car-table">
                        <thead>
                            <tr>
                                <th>{ "Set #" }</th>
                                <th>{ "Total Time" }</th>
                                <th>{ format!("% Off Target ({})",
                                              format_ms_to_minsecms(calculated_target)) }</th>
                                { (0..subset_size).map(|i| {
                                    html!{ <th>{ format!("Car {}", i + 1) }</th> }
                                }).collect::<Html>() }
                            </tr>
                        </thead>
                        <tbody>
                            { all_results.iter().enumerate().map(|(idx, set)| {
                                render_result_row(cars, set, idx, calculated_target)
                            }).collect::<Html>() }
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    }
}

/// Renders a single result row in the table
fn render_result_row(cars: &[Car], set: &[usize], idx: usize, calculated_target: u32) -> Html {
    let total = calculate_total_time(cars, set);
    let pct = calculate_percentage_diff(total, calculated_target);

    html! {
        <tr>
            <td>{ idx + 1 }</td>
            <td>{ format_ms_to_minsecms(total) }</td>
            <td>{ format!("{:+.2}%", pct) }</td>
            { set.iter().map(|&i| {
                let c = &cars[i];
                html! {
                    <td>{ format!("{} ({})", c.id, format_ms_to_minsecms(c.lap_time)) }</td>
                }
            }).collect::<Html>() }
        </tr>
    }
}

/// Slider component for selecting target value with index-to-value mapping.
#[derive(Properties, PartialEq)]
pub struct TargetSliderProps {
    pub slider_idx: usize,
    pub target: u32,
    pub oninput: Callback<InputEvent>,
}

#[function_component(TargetSlider)]
pub fn target_slider(props: &TargetSliderProps) -> Html {
    html! {
        <div class="form-group">
            <label for="target">{ "Target Time:" }</label>
            <div class="slider-with-value">
                <input type="range"
                    min="0"
                    max="99"
                    step="1"
                    value={props.slider_idx.to_string()}
                    oninput={props.oninput.clone()}
                />
                <span class="slider-value">{
                    format!("{} (index {}/99)",
                            format_ms_to_minsecms(props.target), props.slider_idx)
                }</span>
            </div>
        </div>
    }
}

/// Slider component for selecting subset size with dynamic max value.
#[derive(Properties, PartialEq)]
pub struct LapCountSliderProps {
    pub lap_count: usize,
    pub max: usize,
    pub oninput: Callback<InputEvent>,
}

#[function_component(SubsetSlider)]
pub fn subset_slider(props: &LapCountSliderProps) -> Html {
    html! {
        <div class="form-group">
            <label for="lap_count">{ "Lap Count:" }</label>
            <div class="slider-with-value">
                <input type="range"
                    min="1"
                    max={props.max.to_string()}
                    value={props.lap_count.to_string()}
                    oninput={props.oninput.clone()}
                />
                <span class="slider-value">{ format!("{} (max: {})", props.lap_count, props.max) }</span>
            </div>
        </div>
    }
}

/// Slider component for selecting number of runs with dynamic max value.
#[derive(Properties, PartialEq)]
pub struct PlayerCountSliderProps {
    pub player_count: usize,
    pub max: usize,
    pub oninput: Callback<InputEvent>,
}

#[function_component(RunsSlider)]
pub fn runs_slider(props: &PlayerCountSliderProps) -> Html {
    html! {
        <div class="form-group">
            <label for="player_count">{ "Player Count:" }</label>
            <div class="slider-with-value">
                <input type="range"
                    min="0"
                    max={props.max.to_string()}
                    value={props.player_count.to_string()}
                    oninput={props.oninput.clone()}
                />
                <span class="slider-value">{ props.player_count }</span>
            </div>
        </div>
    }
}

/// Displays cache status including filled count, error count, failed targets, and total entries.
#[derive(Properties, PartialEq)]
pub struct CacheInfoProps {
    pub cached_count: usize,
    pub precache_error_count: usize,
    pub precache_failed_targets: Rc<Vec<u32>>,
    pub total: usize,
}

#[function_component(CacheInfo)]
pub fn cache_info(props: &CacheInfoProps) -> Html {
    html! {
        <div class="cache-info-container">
            <div class="cache-status">
                { format!("Cache filled for current Subset & Runs: {}/100", props.cached_count) }
            </div>
            { if props.precache_error_count > 0 {
                html!{
                    <div class="cache-error-status">
                        { format!("Pre-cache tasks failed for current Subset & Runs: {}", props.precache_error_count) }
                    </div>
                }
            } else { html!{} } }
            { if !props.precache_failed_targets.is_empty() {
                html!{
                    <div class="cache-failed-targets">
                        { "Failed Targets (ms): " }
                        { props.precache_failed_targets.iter().map(|t| t.to_string()).collect::<Vec<_>>().join(", ") }
                    </div>
                }
            } else { html!{} } }
            <div class="cache-status-global">
                { format!("Total entries in cache: {}", props.total) }
            </div>
        </div>
    }
}
