//! JavaScript interop for Chart.js visualization.
//! Provides Rust bindings to chart helper functions defined in chart_helpers.js.

use wasm_bindgen::prelude::*;

#[wasm_bindgen(module = "/chart_helpers.js")]
extern "C" {
    #[wasm_bindgen(js_name = initSimilarityChart)]
    pub fn init_similarity_chart(min: u32, max: u32, lap_count: u32, player_count: u32);

    #[wasm_bindgen(js_name = addSimilarityData)]
    pub fn add_similarity_data(target: u32, similarity_pct: f64, lap_count: u32, player_count: u32);

    #[wasm_bindgen(js_name = chartAddFailedTargetMarker)]
    fn chart_add_failed_target_marker(target: u32, lap_count: u32, player_count: u32);
}

/// Plot a red “✖” marker at the given target position so users
/// can immediately spot targets that could not be calculated.
pub fn add_failed_target_marker(target: u32, lap_count: u32, player_count: u32) {
    chart_add_failed_target_marker(target, lap_count, player_count);
}
