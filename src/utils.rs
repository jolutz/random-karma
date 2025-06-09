use crate::config::SLIDER_MAX_INDEX;
use crate::get_target_range_for_subset;
use crate::{cache::CACHE_STORE, Car};
use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::VecDeque;

// Compiled regexes for time parsing
static TIME_MIN_SEC_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+)m\s*(\d+)s$").unwrap());
static TIME_COLON_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+):(\d+)$").unwrap());
static TIME_SEC_REGEX: Lazy<Regex> = Lazy::new(|| Regex::new(r"^(\d+)s$").unwrap());
static TIME_COLON_MSEC_REGEX: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"^(\d+):(\d{2})\.(\d{1,3})$").unwrap());

/// Return indices `0..n` in a “spread-out” order (0, n-1, mid, …).
pub fn spread_indices(n: usize) -> Vec<usize> {
    if n == 0 {
        return Vec::new();
    }

    let mut out = Vec::with_capacity(n);
    let mut seen = vec![false; n];
    let mut q: VecDeque<(usize, usize)> = VecDeque::new();

    out.push(0);
    seen[0] = true;
    if n > 1 {
        out.push(n - 1);
        seen[n - 1] = true;
    }
    q.push_back((0, n - 1));

    while out.len() < n {
        let (lo, hi) = q.pop_front().unwrap();
        if hi - lo <= 1 {
            continue;
        }
        let mid = (lo + hi) / 2;
        if !seen[mid] {
            out.push(mid);
            seen[mid] = true;
        }
        q.push_back((lo, mid));
        q.push_back((mid, hi));
    }
    out
}

/// Return the (min, max) possible total lap times for the given subset size.
pub fn base_target_range(cars: &[Car], subset_size: usize) -> (u32, u32) {
    if cars.is_empty() {
        (0, 0)
    } else {
        get_target_range_for_subset(cars, subset_size)
    }
}

/// Compute the step size between indices given a target range.
pub fn base_target_step(min: u32, max: u32) -> u32 {
    if max > min {
        ((max - min) as f64 / SLIDER_MAX_INDEX as f64).ceil() as u32
    } else {
        1
    }
}

/// Map a slider index into an actual target value within [min, max].
pub fn calc_target_from_idx(min: u32, max: u32, idx: usize) -> u32 {
    let step = base_target_step(min, max);
    (min + step * idx as u32).min(max)
}

/// Count how many cached entries exist for the given lap_count and player_count parameters.
pub fn calc_cached_count(
    min: u32,
    max: u32,
    step: u32,
    lap_count: usize,
    player_count: usize,
) -> usize {
    CACHE_STORE.with(|c| {
        let map = c.borrow();
        (0..=SLIDER_MAX_INDEX)
            .filter(|idx| {
                let target_val = (min + step * *idx as u32).min(max);
                map.contains_key(&(target_val, lap_count, player_count))
            })
            .count()
    })
}

/// Time parsing error types for better error handling
#[derive(Debug)]
pub enum TimeParseError {
    EmptyInput,
    InvalidFormat(String),
    InvalidMinutes,
    InvalidSeconds(u32),
    InvalidMilliseconds,
}

impl std::fmt::Display for TimeParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TimeParseError::EmptyInput => write!(f, "Time cannot be empty"),
            TimeParseError::InvalidFormat(hint) => write!(f, "Invalid time format. {}", hint),
            TimeParseError::InvalidMinutes => write!(f, "Invalid minutes value"),
            TimeParseError::InvalidSeconds(s) => write!(f, "Invalid seconds: {} (must be 0-59)", s),
            TimeParseError::InvalidMilliseconds => write!(f, "Invalid milliseconds value"),
        }
    }
}

impl std::error::Error for TimeParseError {}

/// Parse a time string in various formats to milliseconds.
///
/// Supported formats:
/// - Pure number: "150000" (interpreted as milliseconds)
/// - Minutes:seconds.milliseconds: "2:30.500"
/// - Minutes and seconds: "2m 30s" or "2m30s"
/// - Colon format: "2:30" (minutes:seconds)
/// - Seconds only: "150s"
///
/// # Examples
/// ```
/// assert_eq!(parse_time_to_ms("2:30"), Ok(150_000));
/// assert_eq!(parse_time_to_ms("2m30s"), Ok(150_000));
/// assert_eq!(parse_time_to_ms("150s"), Ok(150_000));
/// assert_eq!(parse_time_to_ms("150000"), Ok(150_000));
/// ```
pub fn parse_time_to_ms(input: &str) -> Result<u32, String> {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(TimeParseError::EmptyInput.to_string());
    }

    // Try parsing as pure number (assume milliseconds)
    if let Ok(ms) = trimmed.parse::<u32>() {
        return Ok(ms);
    }

    // Try parsing mm:ss.SSS format
    if let Some(captures) = TIME_COLON_MSEC_REGEX.captures(trimmed) {
        let minutes: u32 = captures[1]
            .parse()
            .map_err(|_| TimeParseError::InvalidMinutes.to_string())?;
        let seconds: u32 = captures[2]
            .parse()
            .map_err(|_| TimeParseError::InvalidSeconds(0).to_string())?;
        let mut milliseconds: u32 = captures[3]
            .parse()
            .map_err(|_| TimeParseError::InvalidMilliseconds.to_string())?;

        if seconds > 59 {
            return Err(TimeParseError::InvalidSeconds(seconds).to_string());
        }

        // Normalize ms: 1 digit = x100, 2 digits = x10, 3 digits = as is
        match captures[3].len() {
            1 => milliseconds *= 100,
            2 => milliseconds *= 10,
            _ => {}
        }
        return Ok(minutes * 60_000 + seconds * 1_000 + milliseconds);
    }

    // Try parsing "XmYs" format
    if let Some(captures) = TIME_MIN_SEC_REGEX.captures(trimmed) {
        let minutes: u32 = captures[1].parse().map_err(|_| "Invalid minutes")?;
        let seconds: u32 = captures[2].parse().map_err(|_| "Invalid seconds")?;
        if seconds > 59 {
            return Err("Invalid time format: seconds must be 0-59".to_string());
        }
        return Ok(minutes * 60_000 + seconds * 1_000);
    }

    // Try parsing "X:Y" format (minutes:seconds)
    if let Some(captures) = TIME_COLON_REGEX.captures(trimmed) {
        let minutes: u32 = captures[1].parse().map_err(|_| "Invalid minutes")?;
        let seconds: u32 = captures[2].parse().map_err(|_| "Invalid seconds")?;
        if seconds > 59 {
            return Err("Invalid time format: seconds must be 0-59".to_string());
        }
        return Ok(minutes * 60_000 + seconds * 1_000);
    }

    // Try parsing "Xs" format (seconds)
    if let Some(captures) = TIME_SEC_REGEX.captures(trimmed) {
        let seconds: u32 = captures[1].parse().map_err(|_| "Invalid seconds")?;
        return Ok(seconds * 1_000);
    }

    Err(TimeParseError::InvalidFormat("Use: 2:30, 2m30s, 150s, or 150000".to_string()).to_string())
}

/// Generic numeric input validation
pub fn validate_numeric_input<T>(
    input: &str,
    min: Option<T>,
    max: Option<T>,
    field_name: &str,
) -> Result<T, String>
where
    T: std::str::FromStr + std::fmt::Display + PartialOrd,
{
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return Err(format!("{} cannot be empty", field_name));
    }

    match trimmed.parse::<T>() {
        Ok(val) => {
            if let Some(min_val) = min {
                if val < min_val {
                    return Err(format!("{} must be at least {}", field_name, min_val));
                }
            }
            if let Some(max_val) = max {
                if val > max_val {
                    return Err(format!("{} cannot exceed {}", field_name, max_val));
                }
            }
            Ok(val)
        }
        Err(_) => Err(format!("{} must be a valid number", field_name)),
    }
}

/// Validate lap count input
pub fn validate_lap_count(input: &str, max_cars: usize) -> Result<usize, String> {
    validate_numeric_input(input, Some(1), Some(max_cars), "Lap count")
}

/// Validate player count input
pub fn validate_player_count(input: &str) -> Result<usize, String> {
    validate_numeric_input(input, Some(0), Some(250), "Player count")
}
