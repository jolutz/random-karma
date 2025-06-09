use log::{debug, info, warn};
use rand::distr::weighted::WeightedIndex;
use rand::seq::SliceRandom; // Add this line
use rand_distr::Distribution;
use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
#[cfg(not(target_arch = "wasm32"))]
use std::time::Instant;
use wasm_bindgen::prelude::*;

/// Default calculation parameters
pub mod defaults {
    pub const TIMEOUT_MS: f64 = 5000.0;
    pub const TOLERANCE_PERCENT: f64 = 0.5;
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Car {
    pub id: String,
    pub lap_time: u32,
}

pub type CarIndex = usize;

// Custom error type for subset search operations
#[derive(Debug)]
pub enum SubsetError {
    NoValidSubset,
    OutsideTolerance(f64),
    InsufficientCandidates(usize, usize),
    // New, more specific error variants
    TargetUnreachable {
        target: u32,
        current_sum: u32,
        min_possible: u32,
        max_possible: u32,
    },
    NoPreviouslySelectedAvailable,
    PreviouslySelectedInsufficient {
        needed: usize,
        available: usize,
    },
    /// Less than the required number of subsets could be produced
    NotEnoughSuccessfulRuns {
        required: usize,
        found: usize,
    },
}

impl fmt::Display for SubsetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SubsetError::NoValidSubset => write!(f, "Failed to find a valid subset"),
            SubsetError::OutsideTolerance(accuracy) => write!(
                f,
                "Found subset is outside tolerance: {}% of target (acceptable range: 99-101%)",
                accuracy
            ),
            SubsetError::InsufficientCandidates(needed, available) => write!(
                f,
                "Insufficient candidates: needed {}, but only {} available",
                needed, available
            ),
            SubsetError::TargetUnreachable { target, current_sum, min_possible, max_possible } => write!(
                f,
                "Target {} is unreachable. Current sum: {}. Possible range: [{} to {}]",
                target, current_sum, current_sum + min_possible, current_sum + max_possible
            ),
            SubsetError::NoPreviouslySelectedAvailable => write!(
                f,
                "No previously selected numbers available to use when needed"
            ),
            SubsetError::PreviouslySelectedInsufficient { needed, available } => write!(
                f,
                "Even with previously selected numbers, still insufficient: needed {}, but only {} available",
                needed, available
            ),
            SubsetError::NotEnoughSuccessfulRuns { required, found } => write!(
                f,
                "Only {}/{} satisfactory subsets found within tolerance",
                found, required
            ),
        }
    }
}

impl std::error::Error for SubsetError {}

pub fn get_lap_time(cars: &[Car], index: CarIndex) -> u32 {
    cars[index].lap_time
}

pub fn get_car_id(cars: &[Car], index: CarIndex) -> &str {
    &cars[index].id
}

// Order cars by lap_time for sorting operations
impl Ord for Car {
    fn cmp(&self, other: &Self) -> Ordering {
        self.lap_time.cmp(&other.lap_time)
    }
}

impl PartialOrd for Car {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// Return `sum / target` as a percentage (e.g. 100.0 means perfect hit).
#[inline]
pub(crate) fn accuracy_percent(sum: u32, target: u32) -> f64 {
    sum as f64 / target as f64 * 100.0
}

/// Check whether a percentage is inside ±`tolerance_percent`.
#[inline]
pub(crate) fn within_tolerance(value_pct: f64, tolerance_percent: f64) -> bool {
    let lower = 100.0 - tolerance_percent;
    let upper = 100.0 + tolerance_percent;
    (lower..=upper).contains(&value_pct)
}

fn handle_last_number(
    cars: &[Car],
    candidates_for_current_selection: &[CarIndex], // Changed from &mut to &
    previously_selected: &HashSet<CarIndex>,
    selected: &[CarIndex],
    current_sum: u32,
    target: u32,
    tolerance_percent: f64,
) -> (CarIndex, u32) {
    let needed = target.saturating_sub(current_sum);

    // Binary search to find closest element to needed time
    let best_match_idx = find_closest_time(cars, candidates_for_current_selection, needed);
    let best_match_sum = current_sum + get_lap_time(cars, best_match_idx);

    // use new helpers
    let accuracy = accuracy_percent(best_match_sum, target);
    let within_tolerance = within_tolerance(accuracy, tolerance_percent);

    if !within_tolerance {
        debug!("Last number outside tolerance, calling fallback_strategy");
        // Need to make a mutable copy for fallback_strategy
        let mut candidates_copy: Vec<CarIndex> = candidates_for_current_selection.to_vec();
        let (fallback_idx, _) = fallback_strategy(
            cars,
            &mut candidates_copy,
            previously_selected,
            selected,
            current_sum,
            false,
            target,
            1,
        );
        return (fallback_idx, current_sum + get_lap_time(cars, fallback_idx));
    }

    debug!(
        "Last number needed: {}. Using best available: {} (accuracy: {:.2}%)",
        needed,
        get_lap_time(cars, best_match_idx),
        accuracy
    );

    (best_match_idx, best_match_sum)
}

/// Finds the closest time to a target value using binary search.
///
/// # Arguments
/// * `cars` - Slice of cars to search through
/// * `indexes` - Sorted indices into the cars array
/// * `target_time` - The target time to find the closest match for
///
/// # Returns
/// The index of the car with the lap time closest to `target_time`
///
/// # Panics
/// Panics if `indexes` is empty
fn find_closest_time(cars: &[Car], indexes: &[CarIndex], target_time: u32) -> CarIndex {
    assert!(
        !indexes.is_empty(),
        "Cannot find closest time in empty array"
    );

    if indexes.len() == 1 {
        return indexes[0];
    }

    // Binary search
    let mut left = 0;
    let mut right = indexes.len() - 1;

    // Handle edge cases first
    if target_time <= get_lap_time(cars, indexes[left]) {
        return indexes[left];
    }
    if target_time >= get_lap_time(cars, indexes[right]) {
        return indexes[right];
    }

    // Binary search to find the two elements that target_time is between
    while left + 1 < right {
        let mid = left + (right - left) / 2;
        let mid_time = get_lap_time(cars, indexes[mid]);

        match mid_time.cmp(&target_time) {
            Ordering::Equal => return indexes[mid], // Exact match
            Ordering::Less => left = mid,
            Ordering::Greater => right = mid,
        }
    }

    // Now we have two candidates: indexes[left] and indexes[right]
    // Return the closer one
    let left_diff = (get_lap_time(cars, indexes[left]) as i32 - target_time as i32).abs();
    let right_diff = (get_lap_time(cars, indexes[right]) as i32 - target_time as i32).abs();

    if left_diff <= right_diff {
        indexes[left]
    } else {
        indexes[right]
    }
}

fn fallback_strategy(
    cars: &[Car],
    candidates_for_current_selection: &mut [CarIndex],
    previously_selected: &HashSet<CarIndex>,
    selected: &[CarIndex],
    current_sum: u32,
    using_previous_cars: bool,
    target: u32,
    remaining_needed: usize,
) -> (CarIndex, bool) {
    // Use saturating_sub to avoid underflow when current_sum > target
    let remaining_target = target.saturating_sub(current_sum);
    let current_target_avg = if remaining_target == 0 {
        0
    } else {
        remaining_target / remaining_needed as u32
    };

    // Sort to find the best match in current pool
    candidates_for_current_selection
        .sort_by_key(|&idx| (get_lap_time(cars, idx) as i32 - current_target_avg as i32).abs());
    let best_match_idx = candidates_for_current_selection[0];

    // Optionally consider previously selected numbers
    if !using_previous_cars && !previously_selected.is_empty() {
        let available_previous: Vec<CarIndex> = previously_selected
            .iter()
            .filter(|&&idx| !selected.contains(&idx))
            .cloned()
            .collect();

        if !available_previous.is_empty() {
            let best_previous_idx = *available_previous
                .iter()
                .min_by_key(|&&idx| {
                    (get_lap_time(cars, idx) as i32 - current_target_avg as i32).abs()
                })
                .unwrap();

            let best_match_diff =
                (get_lap_time(cars, best_match_idx) as i32 - current_target_avg as i32).abs();
            let best_previous_diff =
                (get_lap_time(cars, best_previous_idx) as i32 - current_target_avg as i32).abs();

            if best_previous_diff < best_match_diff {
                debug!(
                    "Using previously selected number {} instead of {} (closer to target avg: {})",
                    get_lap_time(cars, best_previous_idx),
                    get_lap_time(cars, best_match_idx),
                    current_target_avg
                );
                return (best_previous_idx, true);
            }
        }
    }

    // Return final chosen car and a flag indicating we used a "backtrack"
    (best_match_idx, true)
}

fn try_extend_with_previous(
    candidate_indexes: &mut Vec<CarIndex>,
    previously_selected: &HashSet<CarIndex>,
    selected: &[CarIndex],
) -> bool {
    if previously_selected.is_empty() {
        return false;
    }
    let available_previous: Vec<CarIndex> = previously_selected
        .iter()
        .filter(|&&idx| !selected.contains(&idx))
        .cloned()
        .collect();

    if available_previous.is_empty() {
        false
    } else {
        candidate_indexes.extend(available_previous);
        true
    }
}

/// Helper function to calculate sum of lap times for a subset
#[inline]
fn calculate_subset_sum(cars: &[Car], subset: &[CarIndex]) -> u32 {
    subset.iter().map(|&idx| get_lap_time(cars, idx)).sum()
}

/// Helper function to check if we need to abort due to timeout
#[cfg(not(target_arch = "wasm32"))]
#[inline]
fn is_timeout_exceeded(start_time: std::time::Instant, max_runtime_ms: f64) -> bool {
    start_time.elapsed().as_millis() as f64 > max_runtime_ms
}

#[cfg(target_arch = "wasm32")]
#[inline]
fn is_timeout_exceeded(start_time: f64, max_runtime_ms: f64) -> bool {
    js_sys::Date::now() - start_time > max_runtime_ms
}

pub fn find_approximate_subset(
    cars: &[Car],
    target: u32,
    lap_count: usize,
    previously_selected: &HashSet<CarIndex>,
    tolerance_percent: f64,
) -> Result<Vec<CarIndex>, SubsetError> {
    let mut rng = rand::rng();

    let mut selected = Vec::new();
    let mut current_sum = 0;
    let mut remaining_indexes: Vec<CarIndex> = (0..cars.len()).collect();
    remaining_indexes.sort_by_key(|&idx| get_lap_time(cars, idx));
    let mut total_backtracks = 0;

    while selected.len() < lap_count {
        // Calculate min and max possible sums for remaining needed numbers
        let remaining_needed = lap_count - selected.len();

        debug!(
            "\nSelection progress: {}/{} numbers, current sum: {}",
            selected.len(),
            lap_count,
            current_sum
        );

        // Create candidates for this selection - start with remaining pool
        let mut candidates_for_current_selection = remaining_indexes.clone();
        let mut using_previous_cars = false;

        if remaining_needed > candidates_for_current_selection.len() {
            debug!(
                "Not enough numbers left. Need {}, have {}",
                remaining_needed,
                candidates_for_current_selection.len()
            );
            // Check if we have previously selected numbers we could use
            if !try_extend_with_previous(
                &mut candidates_for_current_selection,
                previously_selected,
                &selected,
            ) {
                debug!("No previously selected numbers available");
                return Err(SubsetError::InsufficientCandidates(
                    remaining_needed,
                    candidates_for_current_selection.len(),
                ));
            } else {
                using_previous_cars = true;
                debug!(
                    "Expanded candidate pool to {} numbers",
                    candidates_for_current_selection.len()
                );

                if candidates_for_current_selection.len() < remaining_needed {
                    debug!("Still not enough numbers after adding previously selected ones");
                    return Err(SubsetError::PreviouslySelectedInsufficient {
                        needed: remaining_needed,
                        available: candidates_for_current_selection.len(),
                    });
                }
            }
        }

        let (min_possible, max_possible) =
            calculate_min_max_sums(cars, &candidates_for_current_selection, remaining_needed);
        debug!(
            "Range check: Need min {} to max {} for remaining {} numbers",
            min_possible, max_possible, remaining_needed
        );

        // Check if target is still reachable
        if current_sum + min_possible > target || current_sum + max_possible < target {
            debug!(
                "Target {} no longer reachable. Current sum: {}, Range: [{}, {}]",
                target,
                current_sum,
                current_sum + min_possible,
                current_sum + max_possible
            );

            // Consider previously selected numbers for this selection only if we haven't already
            if !using_previous_cars {
                if !try_extend_with_previous(
                    &mut candidates_for_current_selection,
                    previously_selected,
                    &selected,
                ) {
                    debug!("No previously selected numbers available to use");
                    return Err(SubsetError::TargetUnreachable {
                        target,
                        current_sum,
                        min_possible,
                        max_possible,
                    });
                } else {
                    using_previous_cars = true;

                    // Re-calculate min/max possible sums with expanded pool
                    let (new_min, new_max) = calculate_min_max_sums(
                        cars,
                        &candidates_for_current_selection,
                        remaining_needed,
                    );
                    debug!(
                        "After adding previously selected numbers, new range: [{}, {}]",
                        current_sum + new_min,
                        current_sum + new_max
                    );

                    // Check if target is now reachable
                    if current_sum + new_min <= target && current_sum + new_max >= target {
                        debug!("Target is now reachable with previously selected numbers");
                        // Continue with expanded pool
                    } else {
                        debug!("Target still not reachable even with previously selected numbers");
                        return Err(SubsetError::TargetUnreachable {
                            target,
                            current_sum,
                            min_possible: new_min,
                            max_possible: new_max,
                        });
                    }
                }
            } else {
                debug!("Already using previously selected numbers but target still unreachable");
                return Err(SubsetError::TargetUnreachable {
                    target,
                    current_sum,
                    min_possible,
                    max_possible,
                });
            }
        }

        // Special case for the last number
        if remaining_needed == 1 {
            let (final_choice, _) = handle_last_number(
                cars,
                &candidates_for_current_selection, // No longer needs mut
                previously_selected,
                &selected,
                current_sum,
                target,
                tolerance_percent,
            );
            selected.push(final_choice);
            break;
        }

        let chosen = select_candidate(
            cars,
            &mut candidates_for_current_selection,
            previously_selected,
            &selected,
            current_sum,
            using_previous_cars,
            target,
            remaining_needed,
            &mut rng,
            &mut total_backtracks,
        );

        current_sum += get_lap_time(cars, chosen);
        selected.push(chosen);
        debug!(
            "Added: {}. New sum: {}/{} ({}%)",
            get_lap_time(cars, chosen),
            current_sum,
            target,
            (current_sum as f64 / target as f64 * 100.0)
        );

        // Remove the chosen number from the original remaining numbers if it was from there
        if let Some(pos) = remaining_indexes.iter().position(|&idx| idx == chosen) {
            remaining_indexes.remove(pos);
        }
        // Note: We don't modify the previously_selected set here, as that happens in the main function
    }

    // Check if we found a valid subset
    if selected.len() == lap_count {
        info!(
            "\n✓ Found solution with {} total backtracks",
            total_backtracks
        );

        let final_sum = calculate_subset_sum(cars, &selected);
        let accuracy = accuracy_percent(final_sum, target);

        info!(
            "Final sum: {}/{} ({}% of target)",
            final_sum, target, accuracy
        );

        // Randomize the order of the selected subset before returning
        selected.shuffle(&mut rng);

        return Ok(selected);
    }

    warn!("All attempts failed completely");
    Err(SubsetError::NoValidSubset)
}

fn select_candidate(
    cars: &[Car],
    candidates_for_current_selection: &mut [CarIndex],
    previously_selected: &HashSet<CarIndex>,
    selected: &[CarIndex],
    current_sum: u32,
    using_previous_cars: bool,
    target: u32,
    remaining_needed: usize,
    rng: &mut impl rand::Rng,
    total_backtracks: &mut u32,
) -> CarIndex {
    let (min_possible_remaining, max_possible_remaining) =
        calculate_min_max_sums(cars, candidates_for_current_selection, remaining_needed - 1);

    let min_valid = target.saturating_sub(current_sum + max_possible_remaining);
    let max_valid = target.saturating_sub(current_sum + min_possible_remaining);

    debug!(
        "Valid range for next number: [{}, {}]",
        min_valid, max_valid
    );

    // Collect only the candidates that are inside the valid range once.
    let filtered: Vec<CarIndex> = candidates_for_current_selection
        .iter()
        .copied()
        .filter(|&idx| {
            let t = get_lap_time(cars, idx);
            t >= min_valid && t <= max_valid
        })
        .collect();

    if !filtered.is_empty() {
        let needed_avg = (target.saturating_sub(current_sum)) as f64 / (remaining_needed as f64);
        debug!(
            "Needed average for next number: {} ({}% of target)",
            needed_avg,
            (needed_avg / target as f64 * 100.0)
        );

        // Build weights paralleling `filtered` so indices match 1-to-1
        let weights = filtered
            .iter()
            .map(|&idx| 1.0 / ((get_lap_time(cars, idx) as f64 - needed_avg).abs() + 1.0));

        let dist = WeightedIndex::new(weights).expect("Non-empty filtered vec guarantees Ok");

        return filtered[dist.sample(rng)];
    }

    debug!("No valid candidates in range! Using fallback strategy");

    let (chosen_temp, used_backtrack) = fallback_strategy(
        cars,
        candidates_for_current_selection,
        previously_selected,
        selected,
        current_sum,
        using_previous_cars,
        target,
        remaining_needed,
    );
    if used_backtrack {
        *total_backtracks += 1;
    }
    chosen_temp
}

fn calculate_min_max_sums(cars: &[Car], indexes: &[CarIndex], x: usize) -> (u32, u32) {
    // Assuming 'indexes' is already sorted by lap_time ascending.
    if indexes.is_empty() || x == 0 {
        return (0, 0);
    }
    // For min sum, sum the first x lap times.
    // For max sum, sum the last x lap times.
    let min_sum: u32 = indexes
        .iter()
        .take(x)
        .map(|&idx| get_lap_time(cars, idx))
        .sum();
    let max_sum: u32 = indexes
        .iter()
        .rev()
        .take(x)
        .map(|&idx| get_lap_time(cars, idx))
        .sum();
    (min_sum, max_sum)
}

/// Returns the minimum and maximum possible target sum for a given subset size and car list.
pub fn get_target_range_for_subset(cars: &[Car], lap_count: usize) -> (u32, u32) {
    if cars.is_empty() || lap_count == 0 {
        return (0, 0);
    }
    let mut indexes: Vec<CarIndex> = (0..cars.len()).collect();
    indexes.sort_by_key(|&idx| get_lap_time(cars, idx));
    calculate_min_max_sums(cars, &indexes, lap_count)
}

pub fn read_cars_from_csv_string(
    csv_content: &str,
    id_column: usize,
    time_column: usize,
    start_line: usize,
) -> Result<Vec<Car>, Box<dyn std::error::Error>> {
    use std::collections::HashSet;

    let mut cars = Vec::new();
    let mut seen_ids = HashSet::new();

    for (i, line) in csv_content.lines().enumerate() {
        // Skip lines before start_line
        if i < start_line {
            continue;
        }

        let fields: Vec<&str> = line.split(',').collect();

        // Ensure we have enough fields
        if fields.len() <= id_column || fields.len() <= time_column {
            debug!("Warning: Line {} has fewer columns than required", i + 1);
            continue;
        }

        // Get ID as string directly
        let id = fields[id_column].trim().to_string();

        // Check for duplicate IDs
        if !seen_ids.insert(id.clone()) {
            debug!(
                "Warning: Duplicate ID '{}' found on line {}, skipping",
                id,
                i + 1
            );
            continue;
        }

        // Parse lap time
        let time_str = fields[time_column].trim();
        let lap_time = match parse_lap_time(time_str) {
            Ok(time) => time,
            Err(e) => {
                debug!("Warning: {} on line {}", e, i + 1);
                continue;
            }
        };

        cars.push(Car { id, lap_time });
    }

    info!("Successfully loaded {} cars from CSV content", cars.len());
    Ok(cars)
}

fn parse_lap_time(time_str: &str) -> Result<u32, String> {
    // Split by colon first (minutes:rest)
    let parts: Vec<&str> = time_str.split(':').collect();

    if parts.len() != 2 {
        return Err(format!(
            "Invalid lap time format: '{}', expected MM:SS.mmm",
            time_str
        ));
    }

    // Parse minutes
    let minutes = match parts[0].trim().parse::<u32>() {
        Ok(min) => min,
        Err(_) => return Err(format!("Failed to parse minutes part: '{}'", parts[0])),
    };

    // Split the second part by dot (seconds.milliseconds)
    let sec_parts: Vec<&str> = parts[1].split('.').collect();

    if sec_parts.len() != 2 {
        return Err(format!(
            "Invalid seconds format: '{}', expected SS.mmm",
            parts[1]
        ));
    }

    // Parse seconds
    let seconds = match sec_parts[0].trim().parse::<u32>() {
        Ok(sec) => sec,
        Err(_) => return Err(format!("Failed to parse seconds part: '{}'", sec_parts[0])),
    };

    // Validate seconds are in range 0-59
    if seconds > 59 {
        return Err(format!("Seconds must be between 0 and 59, got {}", seconds));
    }

    // Parse milliseconds
    let milliseconds_str = sec_parts[1].trim();
    let mut milliseconds = match milliseconds_str.parse::<u32>() {
        Ok(ms) => ms,
        Err(_) => {
            return Err(format!(
                "Failed to parse milliseconds part: '{}'",
                milliseconds_str
            ));
        }
    };

    // Adjust milliseconds based on number of digits
    if milliseconds_str.len() == 1 {
        milliseconds *= 100; // e.g., "4" → 400ms
    } else if milliseconds_str.len() == 2 {
        milliseconds *= 10; // e.g., "43" → 430ms
    }

    // Convert to total milliseconds
    let total_ms = minutes * 60 * 1000 + seconds * 1000 + milliseconds;

    Ok(total_ms)
}

pub fn format_ms_to_minsecms(ms: u32) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let milliseconds = ms % 1000;
    format!("{:02}:{:02}.{:03}", minutes, seconds, milliseconds)
}

pub fn compute_jaccard_similarity(results: &[Vec<CarIndex>]) -> Result<f64, String> {
    use std::collections::HashSet;

    // Convert each result to a HashSet for easy intersection/union
    let sets: Vec<HashSet<CarIndex>> = results
        .iter()
        .map(|subset| subset.iter().cloned().collect())
        .collect();

    let mut total_similarity = 0.0;
    let mut count = 0;

    for i in 0..sets.len() {
        for j in (i + 1)..sets.len() {
            let intersection_size = sets[i].intersection(&sets[j]).count();
            let union_size = sets[i].union(&sets[j]).count();
            let jaccard = intersection_size as f64 / union_size as f64;
            total_similarity += jaccard;
            count += 1;
        }
    }

    if count > 0 {
        let average_jaccard = total_similarity / count as f64;
        info!(
            "\nPairwise Jaccard similarity: {:.4} (range: 0.0 = no overlap, 1.0 = identical)",
            average_jaccard
        );
        Ok(average_jaccard)
    } else {
        info!("Only one or no valid subsets found, skipping similarity measurement");
        Err("Only one or no valid subsets found, skipping similarity measurement".to_string())
    }
}

/// Configuration for subset calculation
#[derive(Clone)]
pub struct SubsetCalculationConfig {
    pub target: u32,
    pub lap_count: usize,
    pub player_count: usize,
    pub timeout_ms: f64,
    pub tolerance_percent: f64,
}

impl Default for SubsetCalculationConfig {
    fn default() -> Self {
        Self {
            target: 0,
            lap_count: 0,
            player_count: 0,
            timeout_ms: defaults::TIMEOUT_MS,
            tolerance_percent: defaults::TOLERANCE_PERCENT,
        }
    }
}

/// Performs multiple subset calculations with progress tracking and timeout handling.
///
/// This is the main entry point for the karma calculation algorithm. It attempts to find
/// `player_count` subsets of size `lap_count` from the provided cars, where each subset
/// sums to approximately the target time.
///
/// # Algorithm
/// 1. Sort available cars by lap time
/// 2. For each run, attempt to find a valid subset
/// 3. Remove selected cars from the pool for subsequent runs
/// 4. Track previously selected cars for potential reuse
/// 5. Apply timeout and tolerance constraints
///
/// # Arguments
/// * `global_cars` - All available cars to select from
/// * `target` - Target sum in milliseconds
/// * `lap_count` - Number of cars per subset
/// * `player_count` - Number of subsets to generate
/// * `timeout_ms` - Maximum time allowed for calculation
/// * `tolerance_percent` - Acceptable deviation from target (e.g., 0.5 for ±0.5%)
///
/// # Returns
/// * `Ok(Vec<Vec<CarIndex>>)` - Successfully found all requested subsets
/// * `Err(SubsetError)` - Failed to find valid subsets within constraints
pub fn perform_multiple_runs(
    global_cars: &[Car],
    target: u32,
    lap_count: usize,
    player_count: usize,
    timeout_ms: f64,
    tolerance_percent: f64,
) -> Result<Vec<Vec<CarIndex>>, SubsetError> {
    // ---------- timeout set-up ----------
    // Use the provided timeout instead of hardcoded value
    let max_runtime_ms: f64 = timeout_ms.max(100.0); // Ensure minimum 100ms
    #[cfg(not(target_arch = "wasm32"))]
    let start_time = Instant::now();
    #[cfg(target_arch = "wasm32")]
    let start_time = js_sys::Date::now();
    // ---------- existing logging ----------
    info!("Starting search for multiple subset sum approximations");
    info!("-----------------------------------------------------");
    info!("Performing {} runs", player_count);
    info!(
        "Each run: finding {} numbers that sum approximately to {}",
        lap_count, target
    );
    info!("Initial pool size: {} numbers", global_cars.len());
    info!("-----------------------------------------------------");

    let mut available_indexes: Vec<CarIndex> = (0..global_cars.len()).collect();
    let mut all_results: Vec<Vec<CarIndex>> = Vec::with_capacity(player_count);
    let mut previously_selected = HashSet::new();

    for run in 1..=player_count {
        info!("\n=== Run {}/{} ===", run, player_count);
        info!("Available pool size: {} numbers", available_indexes.len());

        let result = loop {
            // Check timeout using helper function
            #[cfg(not(target_arch = "wasm32"))]
            if is_timeout_exceeded(start_time, max_runtime_ms) {
                warn!(
                    "Timeout while searching, produced {}/{} subsets",
                    all_results.len(),
                    player_count
                );
                return Err(SubsetError::NotEnoughSuccessfulRuns {
                    required: player_count,
                    found: all_results.len(),
                });
            }
            #[cfg(target_arch = "wasm32")]
            if is_timeout_exceeded(start_time, max_runtime_ms) {
                warn!(
                    "Timeout while searching, produced {}/{} subsets",
                    all_results.len(),
                    player_count
                );
                return Err(SubsetError::NotEnoughSuccessfulRuns {
                    required: player_count,
                    found: all_results.len(),
                });
            }

            let attempt = match find_approximate_subset(
                global_cars,
                target,
                lap_count,
                &previously_selected,
                tolerance_percent,
            ) {
                Ok(subset) => subset,
                Err(err) => {
                    warn!(
                        "Run {}/{}: Failed to find a valid subset: {}",
                        run, player_count, err
                    );
                    return Err(err);
                }
            };

            let subset_sum = calculate_subset_sum(global_cars, &attempt);
            let accuracy = accuracy_percent(subset_sum, target);

            if !within_tolerance(accuracy, tolerance_percent) {
                warn!(
                    "Current run's sum is more than {}% off ({}%), retrying...",
                    tolerance_percent, accuracy
                );
                continue;
            }
            break attempt;
        };

        // Update our previously selected numbers set
        for &idx in &result {
            previously_selected.insert(idx);
        }

        // Remove selected numbers from the pool
        for &idx in &result {
            if let Some(pos) = available_indexes.iter().position(|&i| i == idx) {
                available_indexes.remove(pos);
            }
        }

        all_results.push(result);

        // Quick summary of this run
        let current_sum = calculate_subset_sum(global_cars, all_results.last().unwrap());
        info!(
            "Run {}/{} complete: sum = {} ({}% of target)",
            run,
            player_count,
            current_sum,
            accuracy_percent(current_sum, target)
        );
    }

    // Print final summary
    info!("\n=== FINAL RESULTS ===");
    info!("Completed {} runs", all_results.len());

    let mut total_elements = 0;
    let mut total_sum = 0;

    for (i, subset) in all_results.iter().enumerate() {
        let subset_sum = calculate_subset_sum(global_cars, subset);
        total_elements += subset.len();
        total_sum += subset_sum;

        info!(
            "Run {}: {} numbers, sum = {} ({}% of target)",
            i + 1,
            subset.len(),
            subset_sum,
            accuracy_percent(subset_sum, target)
        );
    }

    if !all_results.is_empty() {
        let avg_accuracy = (total_sum as f64) / (all_results.len() as f64 * target as f64) * 100.0;

        info!("\n=== SUMMARY ===");
        info!(
            "Total numbers selected: {}/{}",
            total_elements,
            lap_count * all_results.len()
        );
        info!(
            "Total sum across all runs: {}/{}",
            total_sum,
            target * all_results.len() as u32
        );
        info!("Average accuracy: {:.2}%", avg_accuracy);
        info!("Remaining numbers in pool: {}", available_indexes.len());
    } else {
        warn!("No successful runs completed");
    }

    if all_results.len() < player_count {
        return Err(SubsetError::NotEnoughSuccessfulRuns {
            required: player_count,
            found: all_results.len(),
        });
    }

    Ok(all_results)
}

pub fn analyze_multiple_runs(
    global_cars: &[Car],
    all_results: &[Vec<CarIndex>],
    total_elements: usize,
) {
    use std::collections::HashMap;
    info!("\n=== CAR FREQUENCY ANALYSIS ===");
    let mut car_id_freq: HashMap<String, usize> = HashMap::new();
    let mut lap_time_freq: HashMap<u32, usize> = HashMap::new();

    // Count how many times each car and lap time was selected
    for subset in all_results {
        for &idx in subset {
            *car_id_freq
                .entry(get_car_id(global_cars, idx).to_string())
                .or_insert(0) += 1;
            *lap_time_freq
                .entry(get_lap_time(global_cars, idx))
                .or_insert(0) += 1;
        }
    }

    let mut car_freq: Vec<(&String, &usize)> = car_id_freq.iter().collect();
    car_freq.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));
    let top_count = if car_freq.len() < 10 {
        car_freq.len()
    } else {
        10
    };
    info!("Top {} most frequently selected cars:", top_count);
    for (i, &(car_id, &count)) in car_freq.iter().take(top_count).enumerate() {
        let car_lap_time = global_cars
            .iter()
            .find(|car| car.id == *car_id)
            .map(|car| car.lap_time)
            .unwrap_or(0);
        info!(
            "#{}: Car {} - lap time {} ms - used in {} runs ({:.0}%)",
            i + 1,
            car_id,
            car_lap_time,
            count,
            (count as f64 / all_results.len() as f64 * 100.0).round()
        );
    }

    info!("\n=== LAP TIME DISTRIBUTION ANALYSIS ===");
    if !lap_time_freq.is_empty() {
        let min_time = lap_time_freq.keys().min().unwrap();
        let max_time = lap_time_freq.keys().max().unwrap();
        info!("Fastest lap time selected: {} ms", min_time);
        info!("Slowest lap time selected: {} ms", max_time);

        let ranges = [
            (30000, 40000),
            (40000, 50000),
            (50000, 60000),
            (60000, 70000),
            (70000, 80000),
            (80000, 90000),
        ];
        for (start, end) in ranges.iter() {
            let count: usize = lap_time_freq
                .iter()
                .filter(|&(&time, _)| time >= *start && time < *end)
                .map(|(_, &cnt)| cnt)
                .sum();
            if count > 0 {
                info!(
                    "Lap times {}-{} ms: {} selections ({:.0}%)",
                    start,
                    end,
                    count,
                    (count as f64 / total_elements as f64 * 100.0).round()
                );
            }
        }
    }
}

/// Web worker entry point for performing multiple subset calculations.
///
/// This function is designed to be called from JavaScript in a web worker context.
/// It handles serialization/deserialization of data across the JS/WASM boundary.
///
/// # Arguments
/// * `cars_js` - Serialized car data from JavaScript
/// * `target` - Target sum in milliseconds
/// * `lap_count` - Number of laps per subset
/// * `player_count` - Number of players (subsets to generate)
///
/// # Returns
/// Serialized result containing all subsets, or error details
#[wasm_bindgen]
pub async fn worker_perform_multiple_runs(
    cars_js: JsValue,
    target: u32,
    lap_count: usize,
    player_count: usize,
) -> JsValue {
    // Deserialize cars from JsValue
    let cars: Vec<Car> = match serde_wasm_bindgen::from_value(cars_js) {
        Ok(c) => c,
        Err(e) => {
            return serde_wasm_bindgen::to_value(&format!("Failed to deserialize cars: {}", e))
                .unwrap_or(JsValue::NULL);
        }
    };

    // Run the calculation with defined constants
    match perform_multiple_runs(
        &cars,
        target,
        lap_count,
        player_count,
        defaults::TIMEOUT_MS,
        defaults::TOLERANCE_PERCENT,
    ) {
        Ok(result) => serde_wasm_bindgen::to_value(&result).unwrap_or(JsValue::NULL),
        Err(e) => serde_wasm_bindgen::to_value(&format!("Calculation failed: {}", e))
            .unwrap_or(JsValue::NULL),
    }
}

pub mod worker_agent;
