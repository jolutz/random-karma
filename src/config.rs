//! Application-level configuration constants.

// UI Behavior
pub const DEBOUNCE_MS: u32 = 300;
pub const WORKER_SCRIPT: &str = "worker.js";

// Default values for input fields
pub const DEFAULT_LAP_COUNT: usize = 25;
pub const DEFAULT_PLAYER_COUNT: usize = 32;
pub const DEFAULT_TARGET_MS: u32 = 2_800_000;
pub const DEFAULT_TIMEOUT_SEC: f64 = 5.0;
pub const DEFAULT_TOLERANCE_PCT: f64 = 0.5;

// Min/Max limits for input fields
pub const MIN_TIMEOUT_SEC: f64 = 1.0;
pub const MAX_TIMEOUT_SEC: f64 = 30.0;
pub const MIN_TOLERANCE_PCT: f64 = 0.1;
pub const MAX_TOLERANCE_PCT: f64 = 5.0;
pub const MAX_PLAYER_COUNT: usize = 250;

// UI constants
pub const SLIDER_MAX_INDEX: usize = 99;
