//! Pure Yew view components for the Random Karma UI.

use crate::{format_ms_to_minsecms, Car};
use std::rc::Rc;
use yew::prelude::*;

fn calculate_total_time(cars: &[Car], indices: &[usize]) -> u32 {
    indices
        .iter()
        .filter_map(|&index| cars.get(index).map(|car| car.lap_time))
        .sum()
}

fn calculate_percentage_diff(actual: u32, target: u32) -> f64 {
    if target == 0 {
        return 0.0;
    }

    (actual as i64 - target as i64) as f64 / target as f64 * 100.0
}

fn render_result_row(cars: &[Car], set: &[usize], index: usize, target: u32) -> Html {
    let total = calculate_total_time(cars, set);
    let percentage = calculate_percentage_diff(total, target);

    html! {
        <tr>
            <td class="sticky-col">{ index + 1 }</td>
            <td>{ format_ms_to_minsecms(total) }</td>
            <td>{ format!("{percentage:.2}%") }</td>
            { for set.iter().map(|&car_index| {
                match cars.get(car_index) {
                    Some(car) => html! {
                        <td>{ format!("{} ({})", car.id, format_ms_to_minsecms(car.lap_time)) }</td>
                    },
                    None => html! { <td class="invalid-result">{ "Invalid car index" }</td> },
                }
            }) }
        </tr>
    }
}

#[derive(Properties, PartialEq)]
pub struct ResultsWrapperProps {
    pub cars: Rc<Vec<Car>>,
    pub all_results: Rc<Vec<Vec<usize>>>,
    pub similarity: f64,
    pub calculated_target: u32,
}

/// Virtualizes rows while retaining a native, horizontally scrollable table.
///
/// Column virtualization made table geometry and sticky headers unreliable.
/// Result sets typically have many more rows than columns, so row-only
/// virtualization is both simpler and cheaper.
#[function_component(ResultsWrapper)]
pub fn results_wrapper(props: &ResultsWrapperProps) -> Html {
    const ROW_HEIGHT: f64 = 44.0;
    const VIEWPORT_HEIGHT: f64 = 600.0;
    const OVERSCAN_ROWS: usize = 10;

    if props.all_results.is_empty() {
        return html! {
            <div class="results" role="status">
                <p class="no-results-message">{ "No matching selections for this target" }</p>
            </div>
        };
    }

    let scroll_top = use_state(|| 0.0);
    let on_scroll = {
        let scroll_top = scroll_top.clone();
        Callback::from(move |event: Event| {
            if let Some(target) = event.target_dyn_into::<web_sys::Element>() {
                scroll_top.set(target.scroll_top() as f64);
            }
        })
    };

    let total_rows = props.all_results.len();
    let subset_size = props.all_results.first().map_or(0, Vec::len);
    let total_columns = 3 + subset_size;
    let visible_rows = (VIEWPORT_HEIGHT / ROW_HEIGHT).ceil() as usize;
    let first_visible_row = (*scroll_top / ROW_HEIGHT).floor() as usize;
    let start_row = first_visible_row.saturating_sub(OVERSCAN_ROWS);
    let end_row = (first_visible_row + visible_rows + OVERSCAN_ROWS).min(total_rows);
    let leading_spacer_height = start_row as f64 * ROW_HEIGHT;
    let trailing_spacer_height = (total_rows - end_row) as f64 * ROW_HEIGHT;

    html! {
        <div class="results">
            <div class="results-overview">
                <div class="similarity-status">
                    { format!("Jaccard similarity · {:.2}%", props.similarity * 100.0) }
                </div>
                <span class="results-count">{ format!("{} selections", total_rows) }</span>
            </div>
            <div class="result-sets">
                <div class="result-sets-header">
                    <h3>{ "Car selections" }</h3>
                    <span>{ format!("Target · {}", format_ms_to_minsecms(props.calculated_target)) }</span>
                </div>
                <div class="big-car-table-container" onscroll={on_scroll} tabindex="0" aria-label="Car selection results">
                    <table class="big-car-table">
                        <thead>
                            <tr>
                                <th class="sticky-col">{ "Set #" }</th>
                                <th>{ "Total Time" }</th>
                                <th>{ "% Off Target" }</th>
                                { for (0..subset_size).map(|index| html! { <th>{ format!("Car {}", index + 1) }</th> }) }
                            </tr>
                        </thead>
                        <tbody>
                            if leading_spacer_height > 0.0 {
                                <tr class="table-spacer">
                                    <td colspan={total_columns.to_string()} style={format!("height: {leading_spacer_height}px")}></td>
                                </tr>
                            }
                            { for props.all_results.iter().enumerate().skip(start_row).take(end_row - start_row).map(|(index, set)| {
                                render_result_row(&props.cars, set, index, props.calculated_target)
                            }) }
                            if trailing_spacer_height > 0.0 {
                                <tr class="table-spacer">
                                    <td colspan={total_columns.to_string()} style={format!("height: {trailing_spacer_height}px")}></td>
                                </tr>
                            }
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    }
}
