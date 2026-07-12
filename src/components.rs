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
    indices
        .iter()
        .filter_map(|&i| cars.get(i).map(|car| car.lap_time))
        .sum()
}

/// Calculate percentage difference from target
fn calculate_percentage_diff(actual: u32, target: u32) -> f64 {
    if target == 0 {
        return 0.0;
    }
    let diff = actual as i64 - target as i64;
    (diff as f64 / target as f64) * 100.0
}

/// Renders a single result row in the table
fn render_result_row(
    cars: &[Car],
    set: &[usize],
    idx: usize,
    calculated_target: u32,
    col_start: usize,
    col_end: usize,
) -> Html {
    let total = calculate_total_time(cars, set);
    let pct = calculate_percentage_diff(total, calculated_target);

    let mut all_cols = vec![];
    all_cols.push(html! { <td class="sticky-col">{ idx + 1 }</td> });
    all_cols.push(html! { <td>{ format_ms_to_minsecms(total) }</td> });
    all_cols.push(html! { <td>{ format!("{:.2}%", pct) }</td> });
    all_cols.extend(set.iter().map(|&i| {
        if let Some(c) = cars.get(i) {
            html! { <td>{ format!("{} ({})", c.id, format_ms_to_minsecms(c.lap_time)) }</td> }
        } else {
            html! { <td class="invalid-result">{ "Invalid car index" }</td> }
        }
    }));

    let visible_cols = all_cols
        .into_iter()
        .skip(col_start)
        .take(col_end.saturating_sub(col_start))
        .collect::<Html>();

    html! {
        <tr>
            { visible_cols }
        </tr>
    }
}

/// A wrapper component that implements virtualization for the results table.
#[derive(Properties, PartialEq)]
pub struct ResultsWrapperProps {
    pub cars: Rc<Vec<Car>>,
    pub all_results: Rc<Vec<Vec<usize>>>,
    pub similarity: f64,
    pub calculated_target: u32,
}

#[function_component(ResultsWrapper)]
pub fn results_wrapper(props: &ResultsWrapperProps) -> Html {
    let scroll_pos = use_state(|| (0.0, 0.0)); // (top, left)

    let on_scroll = {
        let scroll_pos = scroll_pos.clone();
        Callback::from(move |e: Event| {
            if let Some(target) = e.target_dyn_into::<web_sys::Element>() {
                scroll_pos.set((target.scroll_top() as f64, target.scroll_left() as f64));
            }
        })
    };

    // Early return for empty results
    if props.all_results.is_empty() {
        return html! {
            <div class="results">
                <p class="no-results-message">{ "No results to display" }</p>
            </div>
        };
    }

    // --- Virtualization Constants ---
    const ROW_HEIGHT: f64 = 38.0; // Corrected height based on CSS (padding + line-height + border)
    const CONTAINER_HEIGHT: f64 = 600.0; // Fixed height of the scrollable container in pixels
    const OVERSCAN_ROW_COUNT: usize = 10; // Number of rows to render above and below the viewport
    const COLUMN_WIDTH: f64 = 200.0; // Increased width for better readability
    const OVERSCAN_COL_COUNT: usize = 2; // Number of columns to render left and right of the viewport

    let total_rows = props.all_results.len();
    let subset_size = props.all_results.first().map(|s| s.len()).unwrap_or(0);
    let total_columns = 3 + subset_size;

    // --- Virtualization Calculations ---
    let (scroll_top, scroll_left) = *scroll_pos;
    let container_ref = use_node_ref();
    let container_width = use_state(|| 0.0f64);
    {
        let container_ref = container_ref.clone();
        let container_width = container_width.clone();
        use_effect_with((), move |_| {
            if let Some(element) = container_ref.cast::<web_sys::Element>() {
                container_width.set(element.client_width() as f64);
            }
            || ()
        });
    }

    // Rows
    let visible_rows = (CONTAINER_HEIGHT / ROW_HEIGHT).ceil() as usize;
    let start_row_node = (scroll_top / ROW_HEIGHT).floor() as usize;
    let start_row_index = start_row_node.saturating_sub(OVERSCAN_ROW_COUNT);
    let end_row_index = (start_row_node + visible_rows + OVERSCAN_ROW_COUNT).min(total_rows);

    // Columns
    let visible_cols = (*container_width / COLUMN_WIDTH).ceil() as usize;
    let start_col_node = (scroll_left / COLUMN_WIDTH).floor() as usize;
    let start_col_index = start_col_node.saturating_sub(OVERSCAN_COL_COUNT);
    let end_col_index = (start_col_node + visible_cols + OVERSCAN_COL_COUNT).min(total_columns);

    let visible_items = props
        .all_results
        .iter()
        .skip(start_row_index)
        .take(end_row_index - start_row_index)
        .enumerate()
        .map(|(i, set)| {
            let original_index = start_row_index + i;
            render_result_row(
                &props.cars,
                set,
                original_index,
                props.calculated_target,
                start_col_index,
                end_col_index,
            )
        })
        .collect::<Html>();

    let total_height = total_rows as f64 * ROW_HEIGHT;
    let offset_y = start_row_index as f64 * ROW_HEIGHT;

    let total_width = total_columns as f64 * COLUMN_WIDTH;
    let offset_x = start_col_index as f64 * COLUMN_WIDTH;

    // --- Header Rendering ---
    let mut header_cols = vec![];
    header_cols.push(html! { <th class="sticky-col">{ "Set #" }</th> });
    header_cols.push(html! { <th>{ "Total Time" }</th> });
    header_cols.push(html! {
        <th>
            { format!("% Off Target ({})", format_ms_to_minsecms(props.calculated_target)) }
        </th>
    });
    header_cols.extend((0..subset_size).map(|i| html! { <th>{ format!("Car {}", i + 1) }</th> }));

    let visible_header_cols = header_cols
        .into_iter()
        .skip(start_col_index)
        .take(end_col_index - start_col_index)
        .collect::<Html>();

    html! {
        <div class="results">
            <div class="similarity-status">
                { format!("Jaccard Similarity: {:.2}%", props.similarity * 100.0) }
            </div>
            <div class="result-sets">
                <h3>{ "All Car Selections" }</h3>
                <div ref={container_ref} class="big-car-table-container" onscroll={on_scroll} style={format!("height: {}px; overflow: auto;", CONTAINER_HEIGHT)}>
                    <div style={format!("height: {}px; width: {}px; position: relative;", total_height, total_width)}>
                        <table class="big-car-table" style={format!("position: absolute; top: {}px; left: {}px; table-layout: fixed; width: {}px;", offset_y, offset_x, total_width)}>
                            <colgroup>
                                { for (0..total_columns).map(|_| html!{ <col style={format!("width: {}px", COLUMN_WIDTH)} /> }) }
                            </colgroup>
                            <thead>
                                <tr>
                                    { visible_header_cols }
                                </tr>
                            </thead>
                            <tbody>
                                { visible_items }
                            </tbody>
                        </table>
                    </div>
                </div>
            </div>
        </div>
    }
}
