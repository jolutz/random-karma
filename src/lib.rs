use log::{debug, info, warn};
use rand::distr::weighted::WeightedIndex;

use rand::seq::SliceRandom;
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
    InvalidTolerance(f64),
    InvalidTimeout(f64),
    InvalidPriorIndex(CarIndex),
    ImpossibleCount {
        requested: usize,
        available: usize,
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
                target,
                current_sum,
                u64::from(*current_sum) + u64::from(*min_possible),
                u64::from(*current_sum) + u64::from(*max_possible)
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
            SubsetError::InvalidTolerance(value) => write!(f, "Invalid tolerance: {value}"),
            SubsetError::InvalidTimeout(value) => write!(f, "Invalid timeout: {value}"),
            SubsetError::InvalidPriorIndex(index) => write!(f, "Invalid prior index: {index}"),
            SubsetError::ImpossibleCount { requested, available } => write!(
                f,
                "Cannot select {requested} unique cars from {available} cars"
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
    match target {
        0 if sum == 0 => 100.0,
        0 => f64::INFINITY,
        _ => sum as f64 / target as f64 * 100.0,
    }
}

/// Check whether a percentage is inside ±`tolerance_percent`.
#[inline]
pub(crate) fn within_tolerance(value_pct: f64, tolerance_percent: f64) -> bool {
    if !tolerance_percent.is_finite() || tolerance_percent < 0.0 {
        return false;
    }

    let lower = 100.0 - tolerance_percent;
    let upper = 100.0 + tolerance_percent;
    (lower..=upper).contains(&value_pct)
}

#[inline]
fn target_is_reachable(
    current_sum: u32,
    min_possible: u32,
    max_possible: u32,
    target: u32,
    tolerance_percent: f64,
) -> bool {
    if !tolerance_percent.is_finite() || tolerance_percent < 0.0 {
        return false;
    }

    let target = target as f64;
    let lower_bound = target * (1.0 - tolerance_percent / 100.0);
    let upper_bound = target * (1.0 + tolerance_percent / 100.0);
    let min_total = current_sum as f64 + min_possible as f64;
    let max_total = current_sum as f64 + max_possible as f64;

    min_total <= upper_bound && max_total >= lower_bound
}

fn handle_last_number(
    cars: &[Car],
    candidates_for_current_selection: &[CarIndex], // Changed from &mut to &
    current_sum: u32,
    target: u32,
    tolerance_percent: f64,
) -> (CarIndex, u32) {
    let needed = target.saturating_sub(current_sum);

    // Binary search to find closest element to needed time
    let best_match_idx = find_closest_time(cars, candidates_for_current_selection, needed);
    let best_match_sum = current_sum.saturating_add(get_lap_time(cars, best_match_idx));

    // use new helpers
    let accuracy = accuracy_percent(best_match_sum, target);
    let within_tolerance = within_tolerance(accuracy, tolerance_percent);

    if !within_tolerance {
        debug!("Last number outside tolerance, calling fallback_strategy");
        // Need to make a mutable copy for fallback_strategy
        let mut candidates_copy: Vec<CarIndex> = candidates_for_current_selection.to_vec();
        let (fallback_idx, _) =
            fallback_strategy(cars, &mut candidates_copy, current_sum, target, 1);
        return (
            fallback_idx,
            current_sum.saturating_add(get_lap_time(cars, fallback_idx)),
        );
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
    let left_diff = get_lap_time(cars, indexes[left]).abs_diff(target_time);
    let right_diff = get_lap_time(cars, indexes[right]).abs_diff(target_time);

    if left_diff <= right_diff {
        indexes[left]
    } else {
        indexes[right]
    }
}

fn fallback_strategy(
    cars: &[Car],
    candidates_for_current_selection: &mut [CarIndex],
    current_sum: u32,
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
        .sort_by_key(|&idx| get_lap_time(cars, idx).abs_diff(current_target_avg));
    let best_match_idx = candidates_for_current_selection[0];

    // Previously selected cars are added to the candidate pool only when the
    // unused pool cannot satisfy this selection. Never bypass that pool here.

    // Return final chosen car and a flag indicating we used a "backtrack"
    (best_match_idx, true)
}

fn try_extend_with_previous(
    cars: &[Car],
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
        candidate_indexes.sort_unstable_by_key(|&idx| get_lap_time(cars, idx));
        candidate_indexes.dedup();
        true
    }
}

/// Helper function to calculate sum of lap times for a subset
#[inline]
fn calculate_subset_sum_u64(cars: &[Car], subset: &[CarIndex]) -> u64 {
    subset
        .iter()
        .map(|&idx| u64::from(get_lap_time(cars, idx)))
        .sum()
}

#[inline]
fn calculate_subset_sum(cars: &[Car], subset: &[CarIndex]) -> u32 {
    subset
        .iter()
        .map(|&idx| get_lap_time(cars, idx))
        .try_fold(0_u32, u32::checked_add)
        .unwrap_or(u32::MAX)
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

/// Selects which solver implementation backs the stable public API.
///
/// The legacy strategy remains available so a future solver can be introduced
/// and rolled back without restoring deleted code.
#[derive(Debug, Clone, Copy, serde::Deserialize, Eq, Hash, PartialEq, serde::Serialize)]
pub enum SolverStrategy {
    Legacy,
    Bounded,
}

/// Change this one constant to `Legacy` to roll back the public solver.
pub const DEFAULT_SOLVER_STRATEGY: SolverStrategy = SolverStrategy::Bounded;

const BOUNDED_NODE_LIMIT: usize = 500_000;
const BOUNDED_RANDOM_ATTEMPTS: usize = 512;

const EXACT_POOL_LIMIT: usize = 20;
const DEADLINE_CHECK_INTERVAL: usize = 256;

pub fn find_approximate_subset(
    cars: &[Car],
    target: u32,
    lap_count: usize,
    previously_selected: &HashSet<CarIndex>,
    tolerance_percent: f64,
) -> Result<Vec<CarIndex>, SubsetError> {
    let mut rng = rand::rng();
    find_approximate_subset_with_strategy_and_rng(
        DEFAULT_SOLVER_STRATEGY,
        cars,
        target,
        lap_count,
        previously_selected,
        tolerance_percent,
        &mut rng,
    )
}

fn find_approximate_subset_with_strategy_and_rng<R: rand::Rng>(
    strategy: SolverStrategy,
    cars: &[Car],
    target: u32,
    lap_count: usize,
    previously_selected: &HashSet<CarIndex>,
    tolerance_percent: f64,
    rng: &mut R,
) -> Result<Vec<CarIndex>, SubsetError> {
    if !tolerance_percent.is_finite() || tolerance_percent < 0.0 {
        return Err(SubsetError::InvalidTolerance(tolerance_percent));
    }
    if let Some(&index) = previously_selected
        .iter()
        .find(|&&index| index >= cars.len())
    {
        return Err(SubsetError::InvalidPriorIndex(index));
    }
    if lap_count > cars.len() {
        return Err(SubsetError::ImpossibleCount {
            requested: lap_count,
            available: cars.len(),
        });
    }

    let available_indexes: Vec<CarIndex> = (0..cars.len())
        .filter(|idx| !previously_selected.contains(idx))
        .collect();
    match strategy {
        SolverStrategy::Legacy => legacy_find_approximate_subset_from_candidates_with_rng(
            cars,
            target,
            lap_count,
            &available_indexes,
            previously_selected,
            tolerance_percent,
            rng,
        ),
        SolverStrategy::Bounded => {
            let request = BoundedRequest {
                target,
                lap_count,
                tolerance_percent,
                unused: &available_indexes,
                previously_selected,
                usage_counts: None,
            };
            bounded_find_approximate_subset_with_rng(cars, request, rng, || false)
        }
    }
}

fn accepted_sum_interval(target: u32, tolerance_percent: f64) -> (u64, u64) {
    let target = f64::from(target);
    let lower = (target * (1.0 - tolerance_percent / 100.0)).ceil().max(0.0) as u64;
    let upper = (target * (1.0 + tolerance_percent / 100.0))
        .floor()
        .max(0.0) as u64;
    (lower, upper)
}

fn validate_bounded_subset(
    cars: &[Car],
    subset: &[CarIndex],
    lap_count: usize,
    target: u32,
    accepted: (u64, u64),
) -> Result<(), SubsetError> {
    if subset.len() != lap_count
        || subset.iter().any(|&index| index >= cars.len())
        || subset.iter().copied().collect::<HashSet<_>>().len() != subset.len()
    {
        return Err(SubsetError::NoValidSubset);
    }
    let sum = calculate_subset_sum_u64(cars, subset);
    if (accepted.0..=accepted.1).contains(&sum) {
        Ok(())
    } else {
        Err(SubsetError::OutsideTolerance(if target == 0 {
            f64::INFINITY
        } else {
            sum as f64 / f64::from(target) * 100.0
        }))
    }
}

struct BoundedRequest<'a> {
    target: u32,
    lap_count: usize,
    tolerance_percent: f64,
    unused: &'a [CarIndex],
    previously_selected: &'a HashSet<CarIndex>,
    usage_counts: Option<&'a [usize]>,
}

struct BoundedSearch<'a, F> {
    cars: &'a [Car],
    pool: &'a [CarIndex],
    accepted: (u64, u64),
    target: u64,
    nodes: usize,
    deadline_exceeded: &'a mut F,
}

impl<F: FnMut() -> bool> BoundedSearch<'_, F> {
    fn visit(
        &mut self,
        start: usize,
        remaining: usize,
        sum: u64,
        chosen: &mut Vec<CarIndex>,
        reverse: bool,
    ) -> Option<Vec<CarIndex>> {
        self.nodes += 1;
        if self.nodes > BOUNDED_NODE_LIMIT
            || (self.nodes.is_multiple_of(DEADLINE_CHECK_INTERVAL) && (self.deadline_exceeded)())
        {
            return None;
        }
        if remaining == 0 {
            return (self.accepted.0..=self.accepted.1)
                .contains(&sum)
                .then(|| chosen.clone());
        }
        if self.pool.len().saturating_sub(start) < remaining {
            return None;
        }

        let min_add = self.pool[start..start + remaining]
            .iter()
            .map(|&index| u64::from(self.cars[index].lap_time))
            .sum::<u64>();
        let max_add = self.pool[self.pool.len() - remaining..]
            .iter()
            .map(|&index| u64::from(self.cars[index].lap_time))
            .sum::<u64>();
        if sum + min_add > self.accepted.1 || sum + max_add < self.accepted.0 {
            return None;
        }

        let end = self.pool.len() - remaining;
        let wanted = self.target.saturating_sub(sum) / remaining as u64;
        let pivot = self.pool[start..=end]
            .partition_point(|&index| u64::from(self.cars[index].lap_time) < wanted)
            + start;
        for distance in 0..=end - start {
            let right = pivot
                .checked_add(distance)
                .filter(|&position| position <= end);
            let left = pivot
                .checked_sub(distance + 1)
                .filter(|&position| position >= start);
            for position in if reverse {
                [right, left]
            } else {
                [left, right]
            }
            .into_iter()
            .flatten()
            {
                let index = self.pool[position];
                chosen.push(index);
                if let Some(result) = self.visit(
                    position + 1,
                    remaining - 1,
                    sum + u64::from(self.cars[index].lap_time),
                    chosen,
                    reverse,
                ) {
                    return Some(result);
                }
                chosen.pop();
            }
        }
        None
    }
}

struct RandomizedSearchRequest<'a> {
    pool: &'a [CarIndex],
    lap_count: usize,
    target: u64,
    accepted: (u64, u64),
    attempt_limit: usize,
    usage_counts: Option<&'a [usize]>,
    complement: bool,
}

fn randomized_bounded_search<R: rand::Rng, F: FnMut() -> bool>(
    cars: &[Car],
    request: RandomizedSearchRequest<'_>,
    rng: &mut R,
    deadline_exceeded: &mut F,
) -> Option<Vec<CarIndex>> {
    let RandomizedSearchRequest {
        pool,
        lap_count,
        target,
        accepted,
        attempt_limit,
        usage_counts,
        complement,
    } = request;
    let mut selected = Vec::with_capacity(lap_count);
    let mut unselected = Vec::with_capacity(pool.len().saturating_sub(lap_count));
    let mut used = vec![false; cars.len()];

    // Cheap directed boundary constructions are also valuable repair anchors:
    // exact range-edge requests should not spend attempts rediscovering them.
    let mut exact_boundary = None;
    let mut exact_boundary_score = usize::MAX;
    for reverse in [false, true] {
        selected.clear();
        if reverse {
            selected.extend(pool.iter().rev().take(lap_count).copied());
        } else {
            selected.extend(pool.iter().take(lap_count).copied());
        }
        let boundary_sum = calculate_subset_sum_u64(cars, &selected);
        if boundary_sum.abs_diff(target) <= 1
            || (complement
                && usage_counts.is_some()
                && (accepted.0..=accepted.1).contains(&boundary_sum))
        {
            let usage = selected
                .iter()
                .map(|&index| usage_counts.map_or(0, |counts| counts[index]))
                .sum::<usize>();
            let balance = if complement {
                usize::MAX - usage
            } else {
                usage
            };
            if balance < exact_boundary_score
                || (balance == exact_boundary_score && rng.random_bool(0.5))
            {
                exact_boundary_score = balance;
                exact_boundary = Some(selected.clone());
            }
        }
    }
    if exact_boundary.is_some() {
        return exact_boundary;
    }
    selected.clear();

    for attempt in 0..attempt_limit {
        if attempt % 8 == 0 && deadline_exceeded() {
            return None;
        }
        for &index in &selected {
            used[index] = false;
        }
        selected.clear();
        let mut sum = 0_u64;

        // The first construction is the nearer cardinality boundary. The same
        // swap path can then move monotonically into the interior without an
        // average-greedy construction exhausting one side of the value range.
        let usage_is_uniform = usage_counts.is_none_or(|counts| {
            counts
                .first()
                .is_none_or(|first| counts.iter().all(|count| count == first))
        });
        if attempt == 0 && (complement || usage_is_uniform) {
            let minimum = pool
                .iter()
                .take(lap_count)
                .map(|&index| u64::from(cars[index].lap_time))
                .sum::<u64>();
            let maximum = pool
                .iter()
                .rev()
                .take(lap_count)
                .map(|&index| u64::from(cars[index].lap_time))
                .sum::<u64>();
            let shift = if maximum == minimum {
                0
            } else {
                let position = target.saturating_sub(minimum).min(maximum - minimum);
                (position * pool.len().saturating_sub(lap_count) as u64 / (maximum - minimum))
                    as usize
            };
            selected.extend(pool[shift..shift + lap_count].iter().copied());
            for &index in &selected {
                used[index] = true;
                sum += u64::from(cars[index].lap_time);
            }
        }

        // Build every cardinality through the same target-directed path. The
        // pool is sorted once by the caller and reusable membership state
        // avoids per-slot candidate allocation.
        for slot in selected.len()..lap_count {
            if slot.is_multiple_of(32) && deadline_exceeded() {
                return None;
            }
            let remaining = lap_count - slot;
            let required_average = if target >= sum {
                (target - sum) / remaining as u64
            } else {
                0
            };
            let wanted = if usage_counts.is_none() && !complement {
                let span = u64::from(cars[*pool.last()?].lap_time)
                    .saturating_sub(u64::from(cars[pool[0]].lap_time));
                let jitter = span / 64;
                if jitter > 0 && rng.random_bool(0.5) {
                    required_average.saturating_add(rng.random_range(0..=jitter))
                } else if jitter > 0 {
                    required_average.saturating_sub(rng.random_range(0..=jitter))
                } else {
                    required_average
                }
            } else {
                required_average
            };
            let mut best = None;
            let mut best_score = (usize::MAX, u64::MAX);
            let mut ties = 0_u32;
            for &index in pool {
                if used[index] {
                    continue;
                }
                let usage = usage_counts.map_or(0, |counts| counts[index]);
                let balance = if complement {
                    usize::MAX - usage
                } else {
                    usage
                };
                let difference = u64::from(cars[index].lap_time).abs_diff(wanted);
                let score = (balance, difference);
                if score < best_score {
                    best = Some(index);
                    best_score = score;
                    ties = 1;
                } else if score == best_score {
                    ties += 1;
                    if rng.random_ratio(1, ties) {
                        best = Some(index);
                    }
                }
            }
            let index = best?;
            used[index] = true;
            selected.push(index);
            sum += u64::from(cars[index].lap_time);
        }

        // Improve with 1-for-1 swaps. Binary search locates the replacement
        // value and membership is checked in the reusable `used` array.
        for repair in 0..lap_count.saturating_mul(2).min(512) {
            if (accepted.0..=accepted.1).contains(&sum) {
                return Some(selected.clone());
            }
            if repair.is_multiple_of(32) && deadline_exceeded() {
                return None;
            }
            let old_error = sum.abs_diff(target);
            let mut best_swap = None;
            let mut best_score = (u64::MAX, usize::MAX, usize::MAX);
            let start = if selected.is_empty() {
                0
            } else {
                rng.random_range(0..selected.len())
            };
            for offset in 0..selected.len() {
                let selected_position = (start + offset) % selected.len();
                let outgoing = selected[selected_position];
                let base = sum - u64::from(cars[outgoing].lap_time);
                let wanted = target.saturating_sub(base);
                let pivot = pool.partition_point(|&index| u64::from(cars[index].lap_time) < wanted);
                for distance in 0..pool.len() {
                    let positions = [
                        pivot
                            .checked_add(distance)
                            .filter(|&position| position < pool.len()),
                        pivot.checked_sub(distance + 1),
                    ];
                    let mut saw_unselected = false;
                    for position in positions.into_iter().flatten() {
                        let incoming = pool[position];
                        if used[incoming] {
                            continue;
                        }
                        saw_unselected = true;
                        let candidate_sum = base + u64::from(cars[incoming].lap_time);
                        if (accepted.0..=accepted.1).contains(&candidate_sum) {
                            used[outgoing] = false;
                            used[incoming] = true;
                            selected[selected_position] = incoming;
                            return Some(selected.clone());
                        }
                        let error = candidate_sum.abs_diff(target);
                        if error < old_error {
                            let incoming_usage = usage_counts.map_or(0, |counts| counts[incoming]);
                            let outgoing_usage = usage_counts.map_or(0, |counts| counts[outgoing]);
                            let balance = if complement {
                                (usize::MAX - incoming_usage, outgoing_usage)
                            } else {
                                (incoming_usage, usize::MAX - outgoing_usage)
                            };
                            let score = (error, balance.0, balance.1);
                            if score < best_score || (score == best_score && rng.random_bool(0.5)) {
                                best_score = score;
                                best_swap =
                                    Some((selected_position, outgoing, incoming, candidate_sum));
                            }
                        }
                    }
                    if saw_unselected && distance >= 2 {
                        break;
                    }
                }
            }
            let Some((position, outgoing, incoming, candidate_sum)) = best_swap else {
                break;
            };
            used[outgoing] = false;
            used[incoming] = true;
            selected[position] = incoming;
            sum = candidate_sum;
        }

        // A 1-swap local optimum can still be separated from the interval by
        // two coordinated replacements. Rebuild one sorted scratch list and
        // use a two-pointer exact pair search for a bounded set of outgoing
        // pairs; no pair candidates are allocated.
        for round in 0..8 {
            if (accepted.0..=accepted.1).contains(&sum) {
                return Some(selected.clone());
            }
            if deadline_exceeded() {
                return None;
            }
            unselected.clear();
            unselected.extend(pool.iter().copied().filter(|&index| !used[index]));
            if selected.len() < 2 || unselected.len() < 2 {
                break;
            }
            let old_error = sum.abs_diff(target);
            let mut best = None;
            let mut best_score = (old_error, usize::MAX, usize::MAX);
            let pair_limit = selected.len().saturating_mul(8).min(1_024);
            let pair_start = rng.random_range(0..selected.len());
            for pair_number in 0..pair_limit {
                if pair_number.is_multiple_of(16) && deadline_exceeded() {
                    return None;
                }
                let first_position = (pair_start + pair_number) % selected.len();
                let second_position =
                    (first_position + 1 + pair_number / selected.len()) % selected.len();
                if first_position == second_position {
                    continue;
                }
                let first_out = selected[first_position];
                let second_out = selected[second_position];
                let base = sum
                    - u64::from(cars[first_out].lap_time)
                    - u64::from(cars[second_out].lap_time);
                let mut left = 0;
                let mut right = unselected.len() - 1;
                while left < right {
                    let first_in = unselected[left];
                    let second_in = unselected[right];
                    let candidate_sum = base
                        + u64::from(cars[first_in].lap_time)
                        + u64::from(cars[second_in].lap_time);
                    if (accepted.0..=accepted.1).contains(&candidate_sum) {
                        used[first_out] = false;
                        used[second_out] = false;
                        used[first_in] = true;
                        used[second_in] = true;
                        selected[first_position] = first_in;
                        selected[second_position] = second_in;
                        return Some(selected.clone());
                    }
                    let error = candidate_sum.abs_diff(target);
                    if error < old_error {
                        let incoming_usage = usage_counts.map_or(0, |counts| {
                            counts[first_in].saturating_add(counts[second_in])
                        });
                        let outgoing_usage = usage_counts.map_or(0, |counts| {
                            counts[first_out].saturating_add(counts[second_out])
                        });
                        let balance = if complement {
                            (usize::MAX - incoming_usage, outgoing_usage)
                        } else {
                            (incoming_usage, usize::MAX - outgoing_usage)
                        };
                        let score = (error, balance.0, balance.1);
                        if score < best_score || (score == best_score && rng.random_bool(0.5)) {
                            best_score = score;
                            best = Some((
                                first_position,
                                second_position,
                                first_out,
                                second_out,
                                first_in,
                                second_in,
                                candidate_sum,
                            ));
                        }
                    }
                    if candidate_sum < target {
                        left += 1;
                    } else {
                        right -= 1;
                    }
                }
            }
            let Some((
                first_position,
                second_position,
                first_out,
                second_out,
                first_in,
                second_in,
                candidate_sum,
            )) = best
            else {
                break;
            };
            used[first_out] = false;
            used[second_out] = false;
            used[first_in] = true;
            used[second_in] = true;
            selected[first_position] = first_in;
            selected[second_position] = second_in;
            sum = candidate_sum;
            if round == 7 && (accepted.0..=accepted.1).contains(&sum) {
                return Some(selected.clone());
            }
        }
    }
    None
}

struct DiversificationRequest<'a> {
    pool: &'a [CarIndex],
    selected: &'a mut [CarIndex],
    accepted: (u64, u64),
    usage_counts: &'a [usize],
    prefer_selected_usage: bool,
}

fn diversify_valid_selection<R: rand::Rng, F: FnMut() -> bool>(
    cars: &[Car],
    request: DiversificationRequest<'_>,
    rng: &mut R,
    deadline_exceeded: &mut F,
) {
    let DiversificationRequest {
        pool,
        selected,
        accepted,
        usage_counts,
        prefer_selected_usage,
    } = request;
    let mut used = vec![false; cars.len()];
    for &index in selected.iter() {
        used[index] = true;
    }
    let mut sum = calculate_subset_sum_u64(cars, selected);
    let attempts = selected.len().saturating_mul(8).min(2_048);
    for attempt in 0..attempts {
        if attempt.is_multiple_of(DEADLINE_CHECK_INTERVAL) && deadline_exceeded() {
            break;
        }
        let position = rng.random_range(0..selected.len());
        let outgoing = selected[position];
        let incoming = pool[rng.random_range(0..pool.len())];
        if used[incoming] {
            continue;
        }
        let improves_usage = if prefer_selected_usage {
            usage_counts[incoming] > usage_counts[outgoing]
        } else {
            usage_counts[incoming] < usage_counts[outgoing]
        };
        if !improves_usage {
            continue;
        }
        let candidate_sum =
            sum - u64::from(cars[outgoing].lap_time) + u64::from(cars[incoming].lap_time);
        if (accepted.0..=accepted.1).contains(&candidate_sum) {
            used[outgoing] = false;
            used[incoming] = true;
            selected[position] = incoming;
            sum = candidate_sum;
        }
    }
}

fn bounded_find_approximate_subset_with_rng<R: rand::Rng, F: FnMut() -> bool>(
    cars: &[Car],
    request: BoundedRequest<'_>,
    rng: &mut R,
    mut deadline_exceeded: F,
) -> Result<Vec<CarIndex>, SubsetError> {
    let original_accepted = accepted_sum_interval(request.target, request.tolerance_percent);
    if request.lap_count == 0 {
        let result = Vec::new();
        validate_bounded_subset(cars, &result, 0, request.target, original_accepted)?;
        return Ok(result);
    }

    let complement = request.lap_count > cars.len() - request.lap_count;
    let search_count = request.lap_count.min(cars.len() - request.lap_count);
    let total = cars.iter().map(|car| u64::from(car.lap_time)).sum::<u64>();
    let accepted = if complement {
        if original_accepted.0 > total {
            return Err(SubsetError::NoValidSubset);
        }
        (
            total.saturating_sub(original_accepted.1),
            total - original_accepted.0,
        )
    } else {
        original_accepted
    };
    let target = accepted.0 + accepted.1.saturating_sub(accepted.0) / 2;

    // Selecting S should prefer unused cars. When searching its complement Q,
    // that means preferring previously selected cars for exclusion first.
    let preferred_pool = if complement {
        request.previously_selected.iter().copied().collect()
    } else {
        request.unused.to_vec()
    };
    let has_fallback = if complement {
        !request.unused.is_empty()
    } else {
        !request.previously_selected.is_empty()
    };
    let mut pools = vec![preferred_pool];
    if has_fallback {
        pools.push((0..cars.len()).collect());
    }
    let reverse = rng.random_bool(0.5);

    for pool in &mut pools {
        pool.sort_unstable_by_key(|&index| (cars[index].lap_time, index));
        pool.dedup();
        if pool.len() < search_count {
            continue;
        }
        let attempt_limit = BOUNDED_RANDOM_ATTEMPTS;
        let mut result = randomized_bounded_search(
            cars,
            RandomizedSearchRequest {
                pool,
                lap_count: search_count,
                target,
                accepted,
                attempt_limit,
                usage_counts: request.usage_counts,
                complement,
            },
            rng,
            &mut deadline_exceeded,
        );
        if result.is_none() && pool.len() <= EXACT_POOL_LIMIT {
            let mut search = BoundedSearch {
                cars,
                pool,
                accepted,
                target,
                nodes: 0,
                deadline_exceeded: &mut deadline_exceeded,
            };
            result = search.visit(
                0,
                search_count,
                0,
                &mut Vec::with_capacity(search_count),
                reverse,
            );
        }
        if let Some(mut result) = result {
            if let Some(usage_counts) = request.usage_counts {
                diversify_valid_selection(
                    cars,
                    DiversificationRequest {
                        pool,
                        selected: &mut result,
                        accepted,
                        usage_counts,
                        prefer_selected_usage: complement,
                    },
                    rng,
                    &mut deadline_exceeded,
                );
            }
            if complement {
                let excluded = result.into_iter().collect::<HashSet<_>>();
                result = (0..cars.len())
                    .filter(|index| !excluded.contains(index))
                    .collect();
            }
            result.shuffle(rng);
            validate_bounded_subset(
                cars,
                &result,
                request.lap_count,
                request.target,
                original_accepted,
            )?;
            return Ok(result);
        }
        if deadline_exceeded() {
            return Err(SubsetError::NoValidSubset);
        }
    }
    Err(SubsetError::NoValidSubset)
}

fn legacy_find_approximate_subset_from_candidates(
    cars: &[Car],
    target: u32,
    lap_count: usize,
    candidate_indexes: &[CarIndex],
    previously_selected: &HashSet<CarIndex>,
    tolerance_percent: f64,
) -> Result<Vec<CarIndex>, SubsetError> {
    let mut rng = rand::rng();
    legacy_find_approximate_subset_from_candidates_with_rng(
        cars,
        target,
        lap_count,
        candidate_indexes,
        previously_selected,
        tolerance_percent,
        &mut rng,
    )
}

fn legacy_find_approximate_subset_from_candidates_with_rng<R: rand::Rng>(
    cars: &[Car],
    target: u32,
    lap_count: usize,
    candidate_indexes: &[CarIndex],
    previously_selected: &HashSet<CarIndex>,
    tolerance_percent: f64,
    rng: &mut R,
) -> Result<Vec<CarIndex>, SubsetError> {
    if !tolerance_percent.is_finite() || tolerance_percent < 0.0 {
        return Err(SubsetError::NoValidSubset);
    }
    if lap_count == 0 {
        return if within_tolerance(accuracy_percent(0, target), tolerance_percent) {
            Ok(Vec::new())
        } else {
            Err(SubsetError::NoValidSubset)
        };
    }

    let mut selected = Vec::new();
    let mut current_sum = 0;
    let mut remaining_indexes = candidate_indexes.to_vec();
    remaining_indexes.sort_unstable_by_key(|&idx| get_lap_time(cars, idx));
    remaining_indexes.dedup();
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
                cars,
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

        // Check whether the possible range overlaps the tolerance-adjusted target range.
        if !target_is_reachable(
            current_sum,
            min_possible,
            max_possible,
            target,
            tolerance_percent,
        ) {
            debug!(
                "Target {} no longer reachable. Current sum: {}, Range: [{}, {}]",
                target,
                current_sum,
                u64::from(current_sum) + u64::from(min_possible),
                u64::from(current_sum) + u64::from(max_possible)
            );

            // Consider previously selected numbers for this selection only if we haven't already
            if !using_previous_cars {
                if !try_extend_with_previous(
                    cars,
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
                    // Re-calculate min/max possible sums with expanded pool
                    let (new_min, new_max) = calculate_min_max_sums(
                        cars,
                        &candidates_for_current_selection,
                        remaining_needed,
                    );
                    debug!(
                        "After adding previously selected numbers, new range: [{}, {}]",
                        u64::from(current_sum) + u64::from(new_min),
                        u64::from(current_sum) + u64::from(new_max)
                    );

                    // Check whether the expanded range reaches the tolerance-adjusted target.
                    if target_is_reachable(current_sum, new_min, new_max, target, tolerance_percent)
                    {
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
                current_sum,
                target,
                tolerance_percent,
            );
            selected.push(final_choice);
            break;
        }

        let chosen = select_candidate(
            &mut candidates_for_current_selection,
            CandidateSelectionContext {
                cars,
                current_sum,
                target,
                remaining_needed,
                rng,
                total_backtracks: &mut total_backtracks,
            },
        );

        current_sum = current_sum.saturating_add(get_lap_time(cars, chosen));
        selected.push(chosen);
        debug!(
            "Added: {}. New sum: {}/{} ({}%)",
            get_lap_time(cars, chosen),
            current_sum,
            target,
            accuracy_percent(current_sum, target)
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
        selected.shuffle(rng);

        return Ok(selected);
    }

    warn!("All attempts failed completely");
    Err(SubsetError::NoValidSubset)
}

struct CandidateSelectionContext<'a, R> {
    cars: &'a [Car],
    current_sum: u32,
    target: u32,
    remaining_needed: usize,
    rng: &'a mut R,
    total_backtracks: &'a mut u32,
}

fn select_candidate<R: rand::Rng>(
    candidates_for_current_selection: &mut [CarIndex],
    context: CandidateSelectionContext<'_, R>,
) -> CarIndex {
    let CandidateSelectionContext {
        cars,
        current_sum,
        target,
        remaining_needed,
        rng,
        total_backtracks,
    } = context;

    let (min_possible_remaining, max_possible_remaining) =
        calculate_min_max_sums(cars, candidates_for_current_selection, remaining_needed - 1);

    let min_valid = u64::from(target)
        .saturating_sub(u64::from(current_sum) + u64::from(max_possible_remaining))
        .min(u64::from(u32::MAX)) as u32;
    let max_valid = u64::from(target)
        .saturating_sub(u64::from(current_sum) + u64::from(min_possible_remaining))
        .min(u64::from(u32::MAX)) as u32;

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
        current_sum,
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
    if x > indexes.len() {
        return (0, 0);
    }
    if indexes.is_empty() || x == 0 {
        return (0, 0);
    }
    // For min sum, sum the first x lap times.
    // For max sum, sum the last x lap times.
    let sum = |values: &mut dyn Iterator<Item = CarIndex>| {
        values
            .map(|idx| u64::from(get_lap_time(cars, idx)))
            .try_fold(0_u64, u64::checked_add)
            .unwrap_or(u64::MAX)
            .min(u64::from(u32::MAX)) as u32
    };
    let min_sum = sum(&mut indexes.iter().copied().take(x));
    let max_sum = sum(&mut indexes.iter().rev().copied().take(x));
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CsvImportWarningKind {
    MalformedCsv,
    EmptyId,
    MissingLapTime,
    InvalidLapTime,
    DuplicateId,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvImportWarning {
    /// One-based logical CSV record number.
    pub row: usize,
    pub kind: CsvImportWarningKind,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CsvImportReport {
    pub cars: Vec<Car>,
    pub warnings: Vec<CsvImportWarning>,
    pub row_count: usize,
    pub accepted_count: usize,
    pub rejected_count: usize,
}

pub fn read_cars_from_csv_string_detailed(csv_content: &str) -> CsvImportReport {
    let mut cars = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_ids = HashSet::new();
    let mut row_count = 0;
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(csv_content.as_bytes());

    for (i, result) in reader.records().enumerate() {
        let row = i + 1;
        row_count += 1;
        let record = match result {
            Ok(record) => record,
            Err(error) => {
                warnings.push(CsvImportWarning {
                    row,
                    kind: CsvImportWarningKind::MalformedCsv,
                    message: error.to_string(),
                });
                continue;
            }
        };
        let id = record.get(0).unwrap_or_default().trim().to_string();
        if id.is_empty() {
            warnings.push(CsvImportWarning {
                row,
                kind: CsvImportWarningKind::EmptyId,
                message: "vehicle ID is empty".to_string(),
            });
            continue;
        }
        let Some(time_str) = record.get(1).map(str::trim) else {
            warnings.push(CsvImportWarning {
                row,
                kind: CsvImportWarningKind::MissingLapTime,
                message: format!("missing lap time for ID '{id}'"),
            });
            continue;
        };
        let lap_time = match parse_lap_time(time_str) {
            Ok(time) => time,
            Err(message) => {
                warnings.push(CsvImportWarning {
                    row,
                    kind: CsvImportWarningKind::InvalidLapTime,
                    message,
                });
                continue;
            }
        };
        // Only accepted rows reserve an ID, so an invalid row cannot suppress a later valid one.
        if !seen_ids.insert(id.clone()) {
            warnings.push(CsvImportWarning {
                row,
                kind: CsvImportWarningKind::DuplicateId,
                message: format!("duplicate ID '{id}'"),
            });
            continue;
        }
        cars.push(Car { id, lap_time });
    }

    let accepted_count = cars.len();
    CsvImportReport {
        cars,
        warnings,
        row_count,
        accepted_count,
        rejected_count: row_count - accepted_count,
    }
}

pub fn read_cars_from_csv_string(
    csv_content: &str,
) -> Result<Vec<Car>, Box<dyn std::error::Error>> {
    let report = read_cars_from_csv_string_detailed(csv_content);
    for warning in &report.warnings {
        debug!("CSV row {}: {}", warning.row, warning.message);
    }
    info!(
        "Successfully loaded {} cars from CSV content",
        report.accepted_count
    );
    Ok(report.cars)
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

    // Split the second part by dot (seconds with optional milliseconds).
    let sec_parts: Vec<&str> = parts[1].split('.').collect();
    if sec_parts.len() > 2 {
        return Err(format!(
            "Invalid seconds format: '{}', expected SS or SS.mmm",
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

    // Parse optional milliseconds; whole-second values default to zero.
    let milliseconds_str = sec_parts.get(1).copied().unwrap_or("0").trim();
    if milliseconds_str.is_empty() || milliseconds_str.len() > 3 {
        return Err(format!(
            "Milliseconds must contain between 1 and 3 digits, got '{}'",
            milliseconds_str
        ));
    }
    let mut milliseconds = match milliseconds_str.parse::<u64>() {
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
    let total_ms = u64::from(minutes)
        .checked_mul(60_000)
        .and_then(|value| value.checked_add(u64::from(seconds) * 1_000))
        .and_then(|value| value.checked_add(milliseconds))
        .ok_or_else(|| format!("Lap time is too large: '{time_str}'"))?;

    u32::try_from(total_ms).map_err(|_| format!("Lap time is too large: '{time_str}'"))
}

pub fn format_ms_to_minsecms(ms: u32) -> String {
    let total_seconds = ms / 1000;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    let milliseconds = ms % 1000;
    format!("{:02}:{:02}.{:03}", minutes, seconds, milliseconds)
}

pub fn compute_jaccard_similarity(results: &[Vec<CarIndex>]) -> Result<f64, String> {
    if results.len() < 2 {
        info!("Only one or no valid subsets found, skipping similarity measurement");
        return Err(
            "Only one or no valid subsets found, skipping similarity measurement".to_string(),
        );
    }
    if results[1..].iter().all(|subset| subset == &results[0]) {
        return Ok(1.0);
    }

    // Compress the arbitrary index values before making dense bitsets. This avoids sizing an
    // allocation from the largest index (which may be usize::MAX) while retaining exact set
    // semantics for duplicates.
    let mut indices: Vec<CarIndex> = results.iter().flatten().copied().collect();
    indices.sort_unstable();
    indices.dedup();
    let words_per_set = indices.len().div_ceil(u64::BITS as usize);
    let mut bits = vec![0_u64; results.len() * words_per_set];
    let mut cardinalities = vec![0_usize; results.len()];

    for (set_index, subset) in results.iter().enumerate() {
        let set = &mut bits[set_index * words_per_set..(set_index + 1) * words_per_set];
        for index in subset {
            let compressed = indices
                .binary_search(index)
                .expect("index came from the input");
            let word = &mut set[compressed / u64::BITS as usize];
            let mask = 1_u64 << (compressed % u64::BITS as usize);
            if *word & mask == 0 {
                *word |= mask;
                cardinalities[set_index] += 1;
            }
        }
    }

    let mut total_similarity = 0.0;
    let mut count = 0;
    for i in 0..results.len() {
        let left = &bits[i * words_per_set..(i + 1) * words_per_set];
        for j in (i + 1)..results.len() {
            let right = &bits[j * words_per_set..(j + 1) * words_per_set];
            let intersection_size: usize = left
                .iter()
                .zip(right)
                .map(|(left, right)| (left & right).count_ones() as usize)
                .sum();
            let union_size = cardinalities[i] + cardinalities[j] - intersection_size;
            let jaccard = if union_size == 0 {
                1.0
            } else {
                intersection_size as f64 / union_size as f64
            };
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
    perform_multiple_runs_with_strategy(
        DEFAULT_SOLVER_STRATEGY,
        global_cars,
        target,
        lap_count,
        player_count,
        timeout_ms,
        tolerance_percent,
    )
}

/// Performs multiple runs using the explicitly selected solver implementation.
pub fn perform_multiple_runs_with_strategy(
    strategy: SolverStrategy,
    global_cars: &[Car],
    target: u32,
    lap_count: usize,
    player_count: usize,
    timeout_ms: f64,
    tolerance_percent: f64,
) -> Result<Vec<Vec<CarIndex>>, SubsetError> {
    if !timeout_ms.is_finite() || timeout_ms < 0.0 {
        return Err(SubsetError::InvalidTimeout(timeout_ms));
    }
    if !tolerance_percent.is_finite() || tolerance_percent < 0.0 {
        return Err(SubsetError::InvalidTolerance(tolerance_percent));
    }
    if lap_count > global_cars.len() {
        return Err(SubsetError::ImpossibleCount {
            requested: lap_count,
            available: global_cars.len(),
        });
    }

    // ---------- timeout set-up ----------
    let max_runtime_ms: f64 = timeout_ms.max(100.0);
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
    let mut usage_counts = vec![0_usize; global_cars.len()];
    let bounded_indexes = (0..global_cars.len()).collect::<Vec<_>>();
    let bounded_previous = HashSet::new();

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

            let mut rng = rand::rng();
            let attempt = match match strategy {
                SolverStrategy::Legacy => legacy_find_approximate_subset_from_candidates(
                    global_cars,
                    target,
                    lap_count,
                    &available_indexes,
                    &previously_selected,
                    tolerance_percent,
                ),
                SolverStrategy::Bounded => bounded_find_approximate_subset_with_rng(
                    global_cars,
                    BoundedRequest {
                        target,
                        lap_count,
                        tolerance_percent,
                        unused: &bounded_indexes,
                        previously_selected: &bounded_previous,
                        usage_counts: Some(&usage_counts),
                    },
                    &mut rng,
                    || is_timeout_exceeded(start_time, max_runtime_ms),
                ),
            } {
                Ok(subset) => subset,
                Err(err) => {
                    if is_timeout_exceeded(start_time, max_runtime_ms) {
                        return Err(SubsetError::NotEnoughSuccessfulRuns {
                            required: player_count,
                            found: all_results.len(),
                        });
                    }
                    warn!(
                        "Run {}/{}: Failed to find a valid subset: {}",
                        run, player_count, err
                    );
                    return Err(err);
                }
            };

            if strategy == SolverStrategy::Legacy {
                let subset_sum = calculate_subset_sum(global_cars, &attempt);
                let accuracy = accuracy_percent(subset_sum, target);
                if !within_tolerance(accuracy, tolerance_percent) {
                    warn!(
                        "Current run's sum is more than {}% off ({}%), retrying...",
                        tolerance_percent, accuracy
                    );
                    continue;
                }
            }
            break attempt;
        };

        // Update our previously selected numbers set
        for &idx in &result {
            previously_selected.insert(idx);
            usage_counts[idx] += 1;
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
        total_sum = u64::from(total_sum)
            .saturating_add(u64::from(subset_sum))
            .min(u64::from(u32::MAX)) as u32;

        info!(
            "Run {}: {} numbers, sum = {} ({}% of target)",
            i + 1,
            subset.len(),
            subset_sum,
            accuracy_percent(subset_sum, target)
        );
    }

    if !all_results.is_empty() {
        let avg_accuracy = all_results
            .iter()
            .map(|subset| accuracy_percent(calculate_subset_sum(global_cars, subset), target))
            .sum::<f64>()
            / all_results.len() as f64;

        info!("\n=== SUMMARY ===");
        info!(
            "Total numbers selected: {}/{}",
            total_elements,
            lap_count * all_results.len()
        );
        info!(
            "Total sum across all runs: {}/{}",
            total_sum,
            u64::from(target).saturating_mul(all_results.len() as u64)
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

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{rngs::StdRng, Rng, SeedableRng};
    use std::time::Instant;

    fn car(id: &str, lap_time: u32) -> Car {
        Car {
            id: id.to_string(),
            lap_time,
        }
    }

    #[test]
    fn multiple_runs_use_unused_candidates_before_reusing_cars() {
        let cars = vec![car("first", 10), car("second", 10)];

        let results = perform_multiple_runs(&cars, 10, 1, 2, 1_000.0, 0.0).unwrap();

        assert_eq!(results.len(), 2);
        assert_ne!(results[0][0], results[1][0]);
    }

    #[test]
    fn extending_candidates_sorts_and_deduplicates_indexes() {
        let cars = vec![car("slow", 30), car("fast", 10), car("middle", 20)];
        let previously_selected = HashSet::from([0, 1, 2]);
        let mut candidates = vec![2];

        assert!(try_extend_with_previous(
            &cars,
            &mut candidates,
            &previously_selected,
            &[],
        ));
        assert_eq!(candidates, vec![1, 2, 0]);
    }

    #[test]
    fn closest_time_handles_full_u32_range() {
        let cars = vec![car("zero", 0), car("max", u32::MAX)];

        assert_eq!(find_closest_time(&cars, &[0, 1], u32::MAX - 1), 1);
    }

    #[test]
    fn reachability_accepts_tolerance_boundary() {
        let cars = vec![car("near", 99)];

        assert!(target_is_reachable(0, 99, 99, 100, 1.0));
        assert!(!target_is_reachable(0, 98, 98, 100, 1.0));
        assert_eq!(
            find_approximate_subset(&cars, 100, 1, &HashSet::new(), 1.0).unwrap(),
            vec![0]
        );
    }

    fn reference_jaccard_similarity(results: &[Vec<CarIndex>]) -> Result<f64, String> {
        if results.len() < 2 {
            return Err("too few subsets".to_string());
        }
        let sets: Vec<HashSet<_>> = results
            .iter()
            .map(|subset| subset.iter().copied().collect())
            .collect();
        let mut total = 0.0;
        let mut pairs = 0;
        for i in 0..sets.len() {
            for j in (i + 1)..sets.len() {
                let intersection = sets[i].intersection(&sets[j]).count();
                let union = sets[i].len() + sets[j].len() - intersection;
                total += if union == 0 {
                    1.0
                } else {
                    intersection as f64 / union as f64
                };
                pairs += 1;
            }
        }
        Ok(total / pairs as f64)
    }

    #[test]
    fn jaccard_handles_empty_duplicates_arbitrary_indices_and_unequal_sets() {
        assert_eq!(compute_jaccard_similarity(&[vec![], vec![]]).unwrap(), 1.0);
        let cases = [
            vec![vec![1, 1, 2], vec![1, 2]],
            vec![vec![usize::MAX, 0, usize::MAX], vec![0], vec![3, 4, 5]],
            vec![vec![], vec![7, 7], vec![7, 8, 9]],
        ];
        for results in cases {
            assert_eq!(
                compute_jaccard_similarity(&results).unwrap(),
                reference_jaccard_similarity(&results).unwrap()
            );
        }
        assert!(compute_jaccard_similarity(&[]).is_err());
        assert!(compute_jaccard_similarity(&[vec![1]]).is_err());
    }

    #[test]
    fn jaccard_matches_reference_for_deterministic_random_inputs() {
        let mut rng = StdRng::seed_from_u64(0x4a_4143_4341_5244);
        for _ in 0..500 {
            let set_count = rng.random_range(2..20);
            let results: Vec<Vec<CarIndex>> = (0..set_count)
                .map(|_| {
                    let len = rng.random_range(0..80);
                    (0..len)
                        .map(|_| match rng.random_range(0..20) {
                            0 => usize::MAX,
                            1 => usize::MAX - 1,
                            _ => rng.random_range(0..200),
                        })
                        .collect()
                })
                .collect();
            assert_eq!(
                compute_jaccard_similarity(&results).unwrap(),
                reference_jaccard_similarity(&results).unwrap(),
                "results={results:?}"
            );
        }
    }

    #[test]
    fn detailed_csv_import_reports_malformed_and_missing_fields() {
        let report =
            read_cars_from_csv_string_detailed("good,01:02.003\ntoo,many,fields\nmissing-time\n");

        assert_eq!(report.cars, vec![car("good", 62_003)]);
        assert_eq!(
            (
                report.row_count,
                report.accepted_count,
                report.rejected_count
            ),
            (3, 1, 2)
        );
        assert_eq!(report.warnings.len(), 2);
        assert_eq!(report.warnings[0].kind, CsvImportWarningKind::MalformedCsv);
        assert_eq!(report.warnings[0].row, 2);
        assert_eq!(report.warnings[1].kind, CsvImportWarningKind::MalformedCsv);
    }

    #[test]
    fn csv_import_supports_quoted_fields_and_legacy_api() {
        let input = "\"car, one\",\"01:02.3\"\n";
        let report = read_cars_from_csv_string_detailed(input);

        assert_eq!(report.cars, vec![car("car, one", 62_300)]);
        assert!(report.warnings.is_empty());
        assert_eq!(read_cars_from_csv_string(input).unwrap(), report.cars);
    }

    #[test]
    fn invalid_duplicate_does_not_reserve_id_and_valid_duplicates_are_rejected() {
        let report =
            read_cars_from_csv_string_detailed("same,not-a-time\nsame,00:01.000\nsame,00:02.000\n");

        assert_eq!(report.cars, vec![car("same", 1_000)]);
        assert_eq!(
            report.warnings[0].kind,
            CsvImportWarningKind::InvalidLapTime
        );
        assert_eq!(report.warnings[1].kind, CsvImportWarningKind::DuplicateId);
        assert_eq!((report.accepted_count, report.rejected_count), (1, 2));
    }

    #[test]
    fn csv_import_rejects_empty_ids() {
        let report = read_cars_from_csv_string_detailed(",00:01.000\n   ,00:02.000\n");

        assert!(report.cars.is_empty());
        assert_eq!(report.rejected_count, 2);
        assert!(report
            .warnings
            .iter()
            .all(|warning| warning.kind == CsvImportWarningKind::EmptyId));
    }

    #[test]
    fn lap_time_parser_rejects_overflow_and_excess_precision() {
        assert!(parse_lap_time("71583:00.000").is_err());
        assert!(parse_lap_time("4294967295:00.000").is_err());
        assert!(parse_lap_time("00:00.0000").is_err());
        assert_eq!(parse_lap_time("00:00.1").unwrap(), 100);
    }

    #[test]
    fn accumulated_subset_and_range_arithmetic_clamps_instead_of_overflowing() {
        let cars = vec![car("a", u32::MAX), car("b", u32::MAX), car("c", 1)];

        assert_eq!(calculate_subset_sum(&cars, &[0, 1]), u32::MAX);
        assert_eq!(get_target_range_for_subset(&cars, 2), (u32::MAX, u32::MAX));
        assert!(!target_is_reachable(
            u32::MAX,
            u32::MAX,
            u32::MAX,
            u32::MAX,
            0.0,
        ));
    }

    fn assert_valid_subset(
        cars: &[Car],
        subset: &[CarIndex],
        expected_len: usize,
        target: u32,
        tolerance_percent: f64,
    ) {
        assert_eq!(subset.len(), expected_len, "subset has the wrong length");
        assert!(
            subset.iter().all(|&index| index < cars.len()),
            "subset contains an out-of-bounds index"
        );
        assert_eq!(
            subset.iter().copied().collect::<HashSet<_>>().len(),
            subset.len(),
            "subset contains duplicate indices"
        );
        let sum = subset
            .iter()
            .map(|&index| u64::from(cars[index].lap_time))
            .sum::<u64>();
        let accuracy = match (sum, target) {
            (0, 0) => 100.0,
            (_, 0) => f64::INFINITY,
            _ => sum as f64 / target as f64 * 100.0,
        };
        assert!(
            within_tolerance(accuracy, tolerance_percent),
            "subset sum {sum} is outside ±{tolerance_percent}% of target {target}"
        );
    }

    #[test]
    fn successful_subset_always_satisfies_structural_and_tolerance_invariants() {
        let cars = vec![car("low", 90), car("high", 110)];
        let subset = find_approximate_subset(&cars, 100, 1, &HashSet::new(), 1.0)
            .expect_err("a discrete gap must not be reported as a valid subset");
        assert!(matches!(
            subset,
            SubsetError::OutsideTolerance(_) | SubsetError::NoValidSubset
        ));
    }

    #[test]
    fn self_complement_trap_never_returns_an_invalid_subset() {
        let cars = vec![car("low", 20), car("middle", 60), car("high", 100)];

        for _ in 0..100 {
            if let Ok(subset) = find_approximate_subset(&cars, 120, 2, &HashSet::new(), 0.0) {
                assert_valid_subset(&cars, &subset, 2, 120, 0.0);
                assert_eq!(
                    subset.iter().copied().collect::<HashSet<_>>(),
                    HashSet::from([0, 2])
                );
            }
        }
    }

    #[test]
    fn invalid_previously_selected_index_returns_an_error_instead_of_panicking() {
        let cars = vec![car("only", 10)];
        let previous = HashSet::from([99]);
        let call =
            std::panic::catch_unwind(|| find_approximate_subset(&cars, 20, 2, &previous, 0.0));

        assert!(
            call.is_ok(),
            "public solver panicked for an invalid prior index"
        );
        assert!(call.unwrap().is_err());
    }

    #[test]
    fn invalid_timeout_and_tolerance_are_rejected() {
        let cars = vec![car("exact", 100)];

        assert!(perform_multiple_runs(&cars, 100, 1, 1, f64::NAN, 0.0).is_err());
        assert!(perform_multiple_runs(&cars, 100, 1, 1, -1.0, 0.0).is_err());
        assert!(perform_multiple_runs(&cars, 100, 1, 1, 100.0, f64::NAN).is_err());
        assert!(perform_multiple_runs(&cars, 100, 1, 1, 100.0, -1.0).is_err());
    }

    #[test]
    fn impossible_lap_count_has_no_target_range_and_returns_an_error() {
        let cars = vec![car("a", 10), car("b", 20)];

        assert_eq!(get_target_range_for_subset(&cars, 3), (0, 0));
        assert!(find_approximate_subset(&cars, 30, 3, &HashSet::new(), 0.0).is_err());
    }

    #[test]
    fn target_zero_only_accepts_a_zero_sum() {
        let zero = vec![car("zero", 0)];
        let positive = vec![car("positive", 1)];

        let subset = find_approximate_subset(&zero, 0, 1, &HashSet::new(), 0.0).unwrap();
        assert_valid_subset(&zero, &subset, 1, 0, 0.0);
        assert!(find_approximate_subset(&positive, 0, 1, &HashSet::new(), 0.0).is_err());
    }

    #[test]
    fn aggregate_sum_preserves_values_above_u32_max() {
        let cars = vec![car("max", u32::MAX), car("one", 1)];
        let mathematical_sum = calculate_subset_sum_u64(&cars, &[0, 1]);

        assert_eq!(mathematical_sum, u64::from(u32::MAX) + 1);
    }

    fn brute_force_valid_subsets(
        cars: &[Car],
        count: usize,
        target: u32,
        tolerance_percent: f64,
    ) -> Vec<Vec<CarIndex>> {
        fn visit(
            cars: &[Car],
            count: usize,
            start: usize,
            current: &mut Vec<CarIndex>,
            output: &mut Vec<Vec<CarIndex>>,
            target: u32,
            tolerance_percent: f64,
        ) {
            if current.len() == count {
                let sum = current
                    .iter()
                    .map(|&index| u64::from(cars[index].lap_time))
                    .sum::<u64>();
                let accuracy = match (sum, target) {
                    (0, 0) => 100.0,
                    (_, 0) => f64::INFINITY,
                    _ => sum as f64 / target as f64 * 100.0,
                };
                if within_tolerance(accuracy, tolerance_percent) {
                    output.push(current.clone());
                }
                return;
            }

            let remaining = count - current.len();
            if cars.len().saturating_sub(start) < remaining {
                return;
            }
            for index in start..cars.len() {
                current.push(index);
                visit(
                    cars,
                    count,
                    index + 1,
                    current,
                    output,
                    target,
                    tolerance_percent,
                );
                current.pop();
            }
        }

        let mut output = Vec::new();
        visit(
            cars,
            count,
            0,
            &mut Vec::with_capacity(count),
            &mut output,
            target,
            tolerance_percent,
        );
        output
    }

    #[test]
    fn brute_force_oracle_enumerates_all_valid_small_subsets() {
        let cars = vec![car("a", 20), car("b", 60), car("c", 100), car("d", 110)];
        let subsets = brute_force_valid_subsets(&cars, 2, 120, 0.0);

        assert_eq!(subsets, vec![vec![0, 2]]);
    }

    #[test]
    fn legacy_solver_is_reproducible_for_a_fixed_seed() {
        let cars = vec![car("a", 20), car("b", 60), car("c", 100), car("d", 110)];
        let previous = HashSet::new();
        let mut first_rng = StdRng::seed_from_u64(42);
        let mut second_rng = StdRng::seed_from_u64(42);

        let first = find_approximate_subset_with_strategy_and_rng(
            SolverStrategy::Legacy,
            &cars,
            120,
            2,
            &previous,
            0.0,
            &mut first_rng,
        );
        let second = find_approximate_subset_with_strategy_and_rng(
            SolverStrategy::Legacy,
            &cars,
            120,
            2,
            &previous,
            0.0,
            &mut second_rng,
        );

        assert_eq!(format!("{first:?}"), format!("{second:?}"));
    }

    #[test]
    fn seeded_bounded_results_match_the_brute_force_oracle() {
        let cars = vec![car("low", 20), car("middle", 60), car("high", 100)];
        let oracle = brute_force_valid_subsets(&cars, 2, 120, 0.0);
        assert_eq!(oracle, vec![vec![0, 2]]);

        for seed in 0..32 {
            let mut rng = StdRng::seed_from_u64(seed);
            if let Ok(mut subset) = find_approximate_subset_with_strategy_and_rng(
                SolverStrategy::Bounded,
                &cars,
                120,
                2,
                &HashSet::new(),
                0.0,
                &mut rng,
            ) {
                subset.sort_unstable();
                assert!(
                    oracle.contains(&subset),
                    "seed {seed} returned non-oracle subset {subset:?}"
                );
            }
        }
    }

    #[test]
    fn legacy_seed_zero_documents_non_oracle_result() {
        let cars = vec![car("low", 20), car("middle", 60), car("high", 100)];
        let mut rng = StdRng::seed_from_u64(0);
        let mut subset = find_approximate_subset_with_strategy_and_rng(
            SolverStrategy::Legacy,
            &cars,
            120,
            2,
            &HashSet::new(),
            0.0,
            &mut rng,
        )
        .expect("legacy seed zero returns a subset");
        subset.sort_unstable();
        assert_eq!(subset, vec![0, 1], "documents the legacy validation defect");
    }

    #[test]
    fn bounded_solver_is_complete_on_tractable_random_instances() {
        let mut data_rng = StdRng::seed_from_u64(0x5eed);
        for case in 0..100 {
            let count = data_rng.random_range(5..=10);
            let cars: Vec<_> = (0..count)
                .map(|index| car(&index.to_string(), data_rng.random_range(0..=100)))
                .collect();
            let lap_count = data_rng.random_range(0..=count.min(5));
            let target = data_rng.random_range(0..=400);
            let tolerance = data_rng.random_range(0..=10) as f64;
            let oracle = brute_force_valid_subsets(&cars, lap_count, target, tolerance);
            let mut solver_rng = StdRng::seed_from_u64(case);
            let result = find_approximate_subset_with_strategy_and_rng(
                SolverStrategy::Bounded,
                &cars,
                target,
                lap_count,
                &HashSet::new(),
                tolerance,
                &mut solver_rng,
            );
            assert_eq!(
                result.is_ok(),
                !oracle.is_empty(),
                "case {case}: target={target}, count={lap_count}, cars={cars:?}"
            );
            if let Ok(subset) = result {
                assert_valid_subset(&cars, &subset, lap_count, target, tolerance);
            }
        }
    }

    const UI_TARGET: u32 = 2_800_000;
    const UI_LAP_COUNT: usize = 25;
    const UI_PLAYER_COUNT: usize = 32;

    #[derive(Default)]
    struct SolverMetrics {
        valid: usize,
        failures: usize,
        invalid: usize,
        latencies: Vec<std::time::Duration>,
        subsets: HashSet<Vec<CarIndex>>,
    }

    impl SolverMetrics {
        fn record(
            &mut self,
            cars: &[Car],
            result: Result<Vec<CarIndex>, SubsetError>,
            elapsed: std::time::Duration,
        ) {
            self.latencies.push(elapsed);
            match result {
                Ok(mut subset)
                    if validate_bounded_subset(
                        cars,
                        &subset,
                        UI_LAP_COUNT,
                        UI_TARGET,
                        accepted_sum_interval(UI_TARGET, defaults::TOLERANCE_PERCENT),
                    )
                    .is_ok() =>
                {
                    self.valid += 1;
                    subset.sort_unstable();
                    self.subsets.insert(subset);
                }
                Ok(_) => self.invalid += 1,
                Err(_) => self.failures += 1,
            }
        }

        fn percentile(&self, numerator: usize, denominator: usize) -> std::time::Duration {
            let mut values = self.latencies.clone();
            values.sort_unstable();
            values[(values.len() - 1) * numerator / denominator]
        }

        fn report(&self, name: &str) {
            println!(
                "{name}: valid={}, failures={}, invalid={}, unique={}, total={:?}, median={:?}, p95={:?}",
                self.valid,
                self.failures,
                self.invalid,
                self.subsets.len(),
                self.latencies.iter().sum::<std::time::Duration>(),
                self.percentile(1, 2),
                self.percentile(95, 100),
            );
        }
    }

    fn bundled_strategy_metrics(strategy: SolverStrategy, seeds: u64) -> SolverMetrics {
        let cars = read_cars_from_csv_string(include_str!("cars.csv")).unwrap();
        let mut metrics = SolverMetrics::default();
        for seed in 0..seeds {
            let mut rng = StdRng::seed_from_u64(seed);
            let started = Instant::now();
            let result = find_approximate_subset_with_strategy_and_rng(
                strategy,
                &cars,
                UI_TARGET,
                UI_LAP_COUNT,
                &HashSet::new(),
                defaults::TOLERANCE_PERCENT,
                &mut rng,
            );
            metrics.record(&cars, result, started.elapsed());
        }
        metrics
    }

    #[test]
    fn bundled_cars_solver_comparison_reports_metrics() {
        let cars = read_cars_from_csv_string(include_str!("cars.csv")).unwrap();
        assert_eq!(
            cars.len(),
            635,
            "the comparison expects the bundled fixture"
        );

        let bounded = bundled_strategy_metrics(SolverStrategy::Bounded, 16);
        let legacy = bundled_strategy_metrics(SolverStrategy::Legacy, 16);
        bounded.report("bounded");
        legacy.report("legacy");

        assert_eq!(bounded.valid, 16, "bounded must validate for every seed");
        assert_eq!(
            bounded.invalid, 0,
            "bounded must never return invalid subsets"
        );
        assert!(
            bounded.valid >= legacy.valid,
            "bounded valid-return rate should be at least legacy's"
        );
        assert!(
            bounded.subsets.len() > 1,
            "seeded traversal should retain useful diversity"
        );
    }

    #[test]
    fn bounded_full_default_workload_is_valid_and_reports_elapsed_time() {
        let cars = read_cars_from_csv_string(include_str!("cars.csv")).unwrap();
        let started = Instant::now();
        let results = perform_multiple_runs(
            &cars,
            UI_TARGET,
            UI_LAP_COUNT,
            UI_PLAYER_COUNT,
            defaults::TIMEOUT_MS,
            defaults::TOLERANCE_PERCENT,
        )
        .expect("bounded solver should complete within the UI timeout");
        let elapsed = started.elapsed();

        assert_eq!(results.len(), UI_PLAYER_COUNT);
        for subset in &results {
            assert_valid_subset(
                &cars,
                subset,
                UI_LAP_COUNT,
                UI_TARGET,
                defaults::TOLERANCE_PERCENT,
            );
        }
        let unique = results
            .iter()
            .flatten()
            .copied()
            .collect::<HashSet<_>>()
            .len();
        let selections = UI_LAP_COUNT * UI_PLAYER_COUNT;
        let reused = selections - unique;
        let total_available = cars
            .iter()
            .map(|entry| u64::from(entry.lap_time))
            .sum::<u64>();
        let minimum_valid_sum = accepted_sum_interval(UI_TARGET, defaults::TOLERANCE_PERCENT).0;
        let sum_limited_runs = total_available / minimum_valid_sum;
        let cardinality_limited_runs = cars.len() / UI_LAP_COUNT;
        let theoretical_reuse_free_runs =
            sum_limited_runs.min(cardinality_limited_runs as u64) as usize;
        println!(
            "default workload: players={UI_PLAYER_COUNT}, elapsed={elapsed:?}, unique_cars={unique}, reused_selections={reused}, theoretical_reuse_free_runs={theoretical_reuse_free_runs}"
        );
        assert_eq!(theoretical_reuse_free_runs, 17);
        assert!(
            unique >= 16 * UI_LAP_COUNT,
            "adaptive unused-first search should preserve at least 16 reuse-free groups"
        );
    }

    #[test]
    #[ignore = "bounded chart-range strategy comparison; run with --ignored --nocapture"]
    fn bundled_cars_chart_range_diagnostic() {
        use std::io::Write;
        use std::time::Duration;

        const TARGET_SAMPLES: usize = 10;
        const TIMEOUT_MS: f64 = 1_000.0;
        const LAP_COUNT: usize = 25;
        const PLAYER_COUNT: usize = 32;
        const TOLERANCE: f64 = 0.5;

        #[derive(Debug)]
        struct TargetMetrics {
            target: u32,
            jaccard: f64,
            reused: usize,
            elapsed: Duration,
        }

        #[derive(Default)]
        struct StrategyMetrics {
            successful: Vec<TargetMetrics>,
            invalid_outputs: usize,
            elapsed: Duration,
        }

        fn median(values: &mut [f64]) -> f64 {
            values.sort_by(f64::total_cmp);
            let middle = values.len() / 2;
            if values.len().is_multiple_of(2) {
                (values[middle - 1] + values[middle]) / 2.0
            } else {
                values[middle]
            }
        }

        fn report(name: &str, metrics: &StrategyMetrics) {
            let count = metrics.successful.len();
            let total_reused: usize = metrics.successful.iter().map(|entry| entry.reused).sum();
            let average_reused = total_reused as f64 / count.max(1) as f64;
            let average_jaccard = metrics
                .successful
                .iter()
                .map(|entry| entry.jaccard)
                .sum::<f64>()
                / count.max(1) as f64;
            let mut similarities: Vec<_> = metrics
                .successful
                .iter()
                .map(|entry| entry.jaccard)
                .collect();
            let median_jaccard = if similarities.is_empty() {
                f64::NAN
            } else {
                median(&mut similarities)
            };
            let successful_elapsed: Duration =
                metrics.successful.iter().map(|entry| entry.elapsed).sum();
            println!(
                "{name}: successful_targets={count}/{TARGET_SAMPLES}, average_jaccard={average_jaccard:.6}, median_jaccard={median_jaccard:.6}, total_reused={total_reused}, average_reused={average_reused:.2}, successful_elapsed={successful_elapsed:?}, total_elapsed={:?}, invalid_outputs={}",
                metrics.elapsed, metrics.invalid_outputs,
            );

            assert_eq!(
                total_reused,
                metrics
                    .successful
                    .iter()
                    .map(|entry| entry.reused)
                    .sum::<usize>(),
                "reported reuse total must equal its target components"
            );
            if count > 0 {
                assert!((average_reused * count as f64 - total_reused as f64).abs() < 1e-9);
                assert!((0.0..=1.0).contains(&average_jaccard));
                assert!((0.0..=1.0).contains(&median_jaccard));
            }
        }

        let cars = read_cars_from_csv_string(include_str!("cars.csv")).unwrap();
        let (min_target, max_target) = get_target_range_for_subset(&cars, LAP_COUNT);
        let targets: Vec<u32> = (0..TARGET_SAMPLES)
            .map(|sample| {
                (u64::from(min_target)
                    + (u64::from(max_target) - u64::from(min_target)) * sample as u64
                        / (TARGET_SAMPLES - 1) as u64) as u32
            })
            .collect();
        let mut bounded = StrategyMetrics::default();
        let mut legacy = StrategyMetrics::default();

        for (sample, &target) in targets.iter().enumerate() {
            print!(
                "chart-range {}/{TARGET_SAMPLES}: target={target}",
                sample + 1
            );
            std::io::stdout().flush().unwrap();

            for (name, strategy, metrics) in [
                ("bounded", SolverStrategy::Bounded, &mut bounded),
                ("legacy", SolverStrategy::Legacy, &mut legacy),
            ] {
                let started = Instant::now();
                let result = perform_multiple_runs_with_strategy(
                    strategy,
                    &cars,
                    target,
                    LAP_COUNT,
                    PLAYER_COUNT,
                    TIMEOUT_MS,
                    TOLERANCE,
                );
                let elapsed = started.elapsed();
                metrics.elapsed += elapsed;
                let accepted = accepted_sum_interval(target, TOLERANCE);
                let structurally_valid = result.as_ref().is_ok_and(|subsets| {
                    subsets.len() == PLAYER_COUNT
                        && subsets.iter().all(|subset| {
                            validate_bounded_subset(&cars, subset, LAP_COUNT, target, accepted)
                                .is_ok()
                        })
                });
                if result.is_ok() && !structurally_valid {
                    metrics.invalid_outputs += 1;
                }

                if structurally_valid {
                    let subsets = result.unwrap();
                    let total_selections: usize = subsets.iter().map(Vec::len).sum();
                    let distinct = subsets
                        .iter()
                        .flatten()
                        .copied()
                        .collect::<HashSet<_>>()
                        .len();
                    let reused = total_selections - distinct;
                    let jaccard = compute_jaccard_similarity(&subsets).unwrap();
                    assert_eq!(total_selections, LAP_COUNT * PLAYER_COUNT);
                    assert_eq!(reused + distinct, total_selections);
                    metrics.successful.push(TargetMetrics {
                        target,
                        jaccard,
                        reused,
                        elapsed,
                    });
                    print!(", {name}=ok({elapsed:?}, jaccard={jaccard:.4}, reused={reused})");
                } else {
                    print!(", {name}=failed({elapsed:?})");
                }
                std::io::stdout().flush().unwrap();
            }
            println!();
            std::io::stdout().flush().unwrap();
        }

        report("bounded", &bounded);
        report("legacy", &legacy);
        assert_eq!(
            bounded.invalid_outputs, 0,
            "bounded returned an invalid output"
        );

        let matched: Vec<_> = bounded
            .successful
            .iter()
            .filter_map(|bounded_target| {
                legacy
                    .successful
                    .iter()
                    .find(|legacy_target| legacy_target.target == bounded_target.target)
                    .map(|legacy_target| (bounded_target, legacy_target))
            })
            .collect();
        let divisor = matched.len().max(1) as f64;
        let jaccard_delta = matched
            .iter()
            .map(|(bounded_target, legacy_target)| bounded_target.jaccard - legacy_target.jaccard)
            .sum::<f64>()
            / divisor;
        let reused_delta = matched
            .iter()
            .map(|(bounded_target, legacy_target)| {
                bounded_target.reused as f64 - legacy_target.reused as f64
            })
            .sum::<f64>()
            / divisor;
        let elapsed_delta_ms = matched
            .iter()
            .map(|(bounded_target, legacy_target)| {
                bounded_target.elapsed.as_secs_f64() - legacy_target.elapsed.as_secs_f64()
            })
            .sum::<f64>()
            * 1_000.0
            / divisor;
        println!(
            "matched-success: targets={}, average_direct_delta(bounded-legacy): jaccard={jaccard_delta:.6}, reused={reused_delta:.2}, elapsed_ms={elapsed_delta_ms:.3}",
            matched.len(),
        );
        assert!(matched.iter().all(|(bounded_target, legacy_target)| {
            bounded_target.target == legacy_target.target
        }));
        std::io::stdout().flush().unwrap();
    }

    #[test]
    #[ignore = "extended deterministic solver metrics; run with --ignored --nocapture"]
    fn bundled_cars_extended_solver_comparison() {
        let bounded = bundled_strategy_metrics(SolverStrategy::Bounded, 128);
        let legacy = bundled_strategy_metrics(SolverStrategy::Legacy, 128);
        bounded.report("bounded extended");
        legacy.report("legacy extended");
        assert_eq!((bounded.valid, bounded.invalid), (128, 0));
        assert!(bounded.valid >= legacy.valid);
    }

    #[test]
    fn bounded_complement_matches_dual_oracle() {
        let cars = vec![
            car("a", 3),
            car("b", 7),
            car("c", 11),
            car("d", 19),
            car("e", 23),
        ];
        let total = calculate_subset_sum_u64(&cars, &[0, 1, 2, 3, 4]);
        let target = 56;
        let oracle = brute_force_valid_subsets(&cars, 4, target, 0.0);
        assert!(!oracle.is_empty());

        let mut rng = StdRng::seed_from_u64(7);
        let subset = find_approximate_subset_with_strategy_and_rng(
            SolverStrategy::Bounded,
            &cars,
            target,
            4,
            &HashSet::new(),
            0.0,
            &mut rng,
        )
        .unwrap();
        let selected = subset.iter().copied().collect::<HashSet<_>>();
        let excluded = (0..cars.len())
            .filter(|index| !selected.contains(index))
            .collect::<Vec<_>>();
        assert_eq!(calculate_subset_sum_u64(&cars, &subset), u64::from(target));
        assert_eq!(
            calculate_subset_sum_u64(&cars, &excluded),
            total - u64::from(target)
        );
    }

    fn run_increment_one_benchmark(lap_count: usize, player_count: usize, target: u32) {
        use std::io::Write;
        let cars = read_cars_from_csv_string(include_str!("cars.csv")).unwrap();
        print!("benchmark {lap_count}x{player_count} target={target}: starting... ");
        std::io::stdout().flush().unwrap();
        let started = Instant::now();
        let result = perform_multiple_runs_with_strategy(
            SolverStrategy::Bounded,
            &cars,
            target,
            lap_count,
            player_count,
            5_000.0,
            defaults::TOLERANCE_PERCENT,
        );
        let elapsed = started.elapsed();
        let completed = result.as_ref().map_or(0, Vec::len);
        let valid = result.as_ref().is_ok_and(|sets| {
            sets.iter().all(|set| {
                validate_bounded_subset(
                    &cars,
                    set,
                    lap_count,
                    target,
                    accepted_sum_interval(target, defaults::TOLERANCE_PERCENT),
                )
                .is_ok()
            })
        });
        let (jaccard, reuse, usage_min, usage_max) =
            result.as_ref().map_or((f64::NAN, 0, 0, 0), |sets| {
                let mut usage = vec![0_usize; cars.len()];
                for &index in sets.iter().flatten() {
                    usage[index] += 1;
                }
                let distinct = usage.iter().filter(|&&count| count > 0).count();
                (
                    compute_jaccard_similarity(sets).unwrap_or(f64::NAN),
                    sets.iter().map(Vec::len).sum::<usize>() - distinct,
                    usage.iter().copied().min().unwrap_or(0),
                    usage.iter().copied().max().unwrap_or(0),
                )
            });
        println!("completion={completed}/{player_count}, elapsed={elapsed:?}, valid={valid}, jaccard={jaccard:.6}, reuse={reuse}, usage_min={usage_min}, usage_max={usage_max}");
        assert!(
            elapsed.as_secs_f64() <= 5.5,
            "internal five-second deadline was exceeded"
        );
    }

    #[test]
    #[ignore = "increment-one small benchmark"]
    fn increment_one_benchmark_small() {
        run_increment_one_benchmark(25, 32, 2_800_000);
    }

    #[test]
    #[ignore = "increment-one big benchmark"]
    fn increment_one_benchmark_big() {
        run_increment_one_benchmark(416, 181, 27_547_641);
    }

    #[test]
    #[ignore = "bounded 416x181 inclusive chart-range diagnostic"]
    fn bounded_big_chart_range_diagnostic() {
        use std::io::Write;
        use std::time::Duration;

        const TARGET_SAMPLES: usize = 10;
        const LAP_COUNT: usize = 416;
        const PLAYER_COUNT: usize = 181;
        const TOLERANCE: f64 = 0.5;
        const TIMEOUT_MS: f64 = 2_000.0;

        fn percentile_duration(values: &mut [Duration], percentile: usize) -> Duration {
            values.sort_unstable();
            values[(values.len() - 1) * percentile / 100]
        }

        fn percentile_f64(values: &mut [f64], percentile: usize) -> f64 {
            values.sort_by(f64::total_cmp);
            values[(values.len() - 1) * percentile / 100]
        }

        let cars = read_cars_from_csv_string(include_str!("cars.csv")).unwrap();
        let (minimum_target, maximum_target) = get_target_range_for_subset(&cars, LAP_COUNT);
        let targets = (0..TARGET_SAMPLES)
            .map(|sample| {
                (u64::from(minimum_target)
                    + (u64::from(maximum_target) - u64::from(minimum_target)) * sample as u64
                        / (TARGET_SAMPLES - 1) as u64) as u32
            })
            .collect::<Vec<_>>();
        let mut successful_elapsed = Vec::new();
        let mut successful_jaccard = Vec::new();
        let mut successful_reuse = Vec::new();

        println!(
            "bounded 416x181 chart range: targets={TARGET_SAMPLES}, range={minimum_target}..={maximum_target}, tolerance={TOLERANCE}%, internal_timeout_ms={TIMEOUT_MS}"
        );
        std::io::stdout().flush().unwrap();

        for (sample, target) in targets.into_iter().enumerate() {
            print!(
                "target {}/{TARGET_SAMPLES}: target={target}... ",
                sample + 1
            );
            std::io::stdout().flush().unwrap();
            let started = Instant::now();
            let result = perform_multiple_runs_with_strategy(
                SolverStrategy::Bounded,
                &cars,
                target,
                LAP_COUNT,
                PLAYER_COUNT,
                TIMEOUT_MS,
                TOLERANCE,
            );
            let elapsed = started.elapsed();
            let completed = result.as_ref().map_or_else(|_| 0, |subsets| subsets.len());
            let valid = result.as_ref().is_ok_and(|subsets| {
                subsets.len() == PLAYER_COUNT
                    && subsets.iter().all(|subset| {
                        validate_bounded_subset(
                            &cars,
                            subset,
                            LAP_COUNT,
                            target,
                            accepted_sum_interval(target, TOLERANCE),
                        )
                        .is_ok()
                    })
            });
            let (jaccard, reused, distinct, usage_min, usage_max, usage_stddev) = result
                .as_ref()
                .map_or((f64::NAN, 0, 0, 0, 0, f64::NAN), |subsets| {
                    let mut usage = vec![0_usize; cars.len()];
                    for &index in subsets.iter().flatten() {
                        usage[index] += 1;
                    }
                    let total = subsets.iter().map(Vec::len).sum::<usize>();
                    let distinct = usage.iter().filter(|&&count| count > 0).count();
                    let mean = total as f64 / usage.len() as f64;
                    let variance = usage
                        .iter()
                        .map(|&count| {
                            let difference = count as f64 - mean;
                            difference * difference
                        })
                        .sum::<f64>()
                        / usage.len() as f64;
                    (
                        compute_jaccard_similarity(subsets).unwrap_or(f64::NAN),
                        total - distinct,
                        distinct,
                        usage.iter().copied().min().unwrap_or(0),
                        usage.iter().copied().max().unwrap_or(0),
                        variance.sqrt(),
                    )
                });
            println!(
                "completion={completed}/{PLAYER_COUNT}, elapsed={elapsed:?}, valid={valid}, jaccard={jaccard:.6}, reused={reused}, distinct={distinct}, usage_min={usage_min}, usage_max={usage_max}, usage_stddev={usage_stddev:.6}"
            );
            std::io::stdout().flush().unwrap();
            if valid {
                successful_elapsed.push(elapsed);
                successful_jaccard.push(jaccard);
                successful_reuse.push(reused as f64);
            }
        }

        if successful_elapsed.is_empty() {
            println!("summary: successes=0/{TARGET_SAMPLES}");
        } else {
            let elapsed_p50 = percentile_duration(&mut successful_elapsed.clone(), 50);
            let elapsed_p95 = percentile_duration(&mut successful_elapsed.clone(), 95);
            let jaccard_p50 = percentile_f64(&mut successful_jaccard.clone(), 50);
            let jaccard_p95 = percentile_f64(&mut successful_jaccard.clone(), 95);
            let reuse_p50 = percentile_f64(&mut successful_reuse.clone(), 50);
            let reuse_p95 = percentile_f64(&mut successful_reuse.clone(), 95);
            println!(
                "summary: successes={}/{TARGET_SAMPLES}, elapsed_p50={elapsed_p50:?}, elapsed_p95={elapsed_p95:?}, jaccard_p50={jaccard_p50:.6}, jaccard_p95={jaccard_p95:.6}, reused_p50={reuse_p50:.0}, reused_p95={reuse_p95:.0}",
                successful_elapsed.len(),
            );
        }
        std::io::stdout().flush().unwrap();
    }

    #[test]
    fn infinite_timeout_is_rejected() {
        let cars = vec![car("exact", 100)];
        assert!(perform_multiple_runs(&cars, 100, 1, 1, f64::INFINITY, 0.0).is_err());
    }
}

pub mod worker_agent;
