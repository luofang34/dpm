//! Value encodings shared by import and export.
//!
//! Import compares a source value with the encoding of the current local value before changing
//! anything, so a file exported by DPM re-imports without semantic differences even where the
//! encoding rounds (seconds for durations, tenths of minutes for lags) or collapses (a three-point
//! estimate written as one duration).

use dpm_model::{DependencyKind, Priority, ThreePointEstimate, WorkItemId};
use uuid::Uuid;

const SECONDS_PER_HOUR: f64 = 3600.0;
/// MSPDI `LinkLag` counts tenths of a minute.
const LAG_UNITS_PER_HOUR: f64 = 600.0;
/// MSPDI duration/lag format code for elapsed hours; DPM durations and lags are elapsed hours.
pub(crate) const ELAPSED_HOURS_FORMAT: u32 = 6;
/// MSPDI priority when a file omits it.
pub(crate) const DEFAULT_PRIORITY: i64 = 500;

/// Whether an MSPDI duration or lag counts calendar working time or elapsed wall-clock time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TimeBasis {
    /// Minutes, hours, days, weeks or months of the project calendar's working time.
    Working,
    /// Wall-clock time, independent of any calendar.
    Elapsed,
}

/// Classify an MSPDI `DurationFormat`/`LagFormat` code; percentages and unknown codes have no
/// hour equivalent. Codes 35..=44 are the estimated (`?`) variants of 3..=12.
pub(crate) fn time_basis(format: u32) -> Option<TimeBasis> {
    let base = if (35..=44).contains(&format) {
        format - 32
    } else {
        format
    };
    match base {
        3 | 5 | 7 | 9 | 11 => Some(TimeBasis::Working),
        4 | 6 | 8 | 10 | 12 => Some(TimeBasis::Elapsed),
        _ => None,
    }
}

/// Exported duration of a task: its PERT expectation rounded to whole seconds; unestimated work
/// is zero, matching the schedule's treatment of absent estimates.
pub(crate) fn duration_seconds(estimate: Option<ThreePointEstimate>) -> u64 {
    estimate.map_or(0, |estimate| {
        (estimate.pert_expected_hours() * SECONDS_PER_HOUR).round() as u64
    })
}

/// Single-point estimate for a source duration; zero stays unestimated.
pub(crate) fn estimate_from_seconds(seconds: u64) -> Option<ThreePointEstimate> {
    (seconds > 0).then(|| {
        let hours = seconds as f64 / SECONDS_PER_HOUR;
        ThreePointEstimate {
            optimistic_hours: hours,
            likely_hours: hours,
            pessimistic_hours: hours,
        }
    })
}

/// Hours as a whole number of seconds, for display in reports.
pub(crate) fn hours(seconds: u64) -> f64 {
    seconds as f64 / SECONDS_PER_HOUR
}

/// MSPDI `LinkLag` for a lag in hours.
pub(crate) fn lag_tenths(lag_hours: f64) -> i64 {
    (lag_hours * LAG_UNITS_PER_HOUR).round() as i64
}

/// Lag in hours for an MSPDI `LinkLag`.
pub(crate) fn lag_hours(tenths: i64) -> f64 {
    tenths as f64 / LAG_UNITS_PER_HOUR
}

/// Format seconds as the `PTnHnMnS` form MSPDI writers use.
pub(crate) fn format_duration(seconds: u64) -> String {
    format!(
        "PT{}H{}M{}S",
        seconds / 3600,
        seconds % 3600 / 60,
        seconds % 60
    )
}

/// Parse an MSPDI `PTnHnMnS` duration into whole seconds.
///
/// Day, week, month and year designators are rejected: their length depends on a calendar that
/// DPM does not import.
pub(crate) fn parse_duration(text: &str) -> Result<u64, String> {
    let body = text
        .trim()
        .strip_prefix("PT")
        .ok_or_else(|| format!("duration {text} is not an hour/minute/second duration"))?;
    let mut total = 0.0;
    let mut number = String::new();
    let mut last_rank = 0;
    for character in body.chars() {
        if character.is_ascii_digit() || character == '.' {
            number.push(character);
            continue;
        }
        let (rank, scale) = match character {
            'H' => (1, SECONDS_PER_HOUR),
            'M' => (2, 60.0),
            'S' => (3, 1.0),
            _ => {
                return Err(format!(
                    "duration {text} has unsupported designator {character}"
                ));
            }
        };
        let value: f64 = number
            .parse()
            .map_err(|_| format!("duration {text} has a malformed number"))?;
        if rank <= last_rank {
            return Err(format!("duration {text} repeats or reorders designators"));
        }
        total += value * scale;
        last_rank = rank;
        number.clear();
    }
    if !number.is_empty() || last_rank == 0 || !total.is_finite() {
        return Err(format!("duration {text} is incomplete"));
    }
    Ok(total.round() as u64)
}

/// MSPDI priority (0..=1000) written for a DPM priority.
pub(crate) fn priority_value(priority: Priority) -> i64 {
    match priority {
        Priority::P0 => 900,
        Priority::P1 => 700,
        Priority::P2 => 500,
        Priority::P3 => 300,
        Priority::P4 => 100,
    }
}

/// Nearest DPM priority band for an MSPDI priority; it inverts [`priority_value`] exactly.
pub(crate) fn priority_from_value(value: i64) -> Priority {
    match value {
        800.. => Priority::P0,
        600..=799 => Priority::P1,
        400..=599 => Priority::P2,
        200..=399 => Priority::P3,
        _ => Priority::P4,
    }
}

/// MSPDI `PredecessorLink/Type` code.
pub(crate) fn relation_code(kind: DependencyKind) -> u32 {
    match kind {
        DependencyKind::FinishFinish => 0,
        DependencyKind::FinishStart => 1,
        DependencyKind::StartFinish => 2,
        DependencyKind::StartStart => 3,
    }
}

/// Relation for an MSPDI `PredecessorLink/Type` code.
pub(crate) fn relation_from_code(code: i64) -> Option<DependencyKind> {
    match code {
        0 => Some(DependencyKind::FinishFinish),
        1 => Some(DependencyKind::FinishStart),
        2 => Some(DependencyKind::StartFinish),
        3 => Some(DependencyKind::StartStart),
        _ => None,
    }
}

/// GUID spelling used by Microsoft Project: upper-case and hyphenated.
pub(crate) fn format_guid(id: Uuid) -> String {
    id.hyphenated().to_string().to_uppercase()
}

const FNV_OFFSET: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
const FNV_PRIME: u128 = 0x0000_0000_0100_0000_0000_0000_0000_013b;
const TASK_IDENTITY_DOMAIN: &[u8] = b"dpm.mspdi.task.v1\0";

/// MurmurHash3's 64-bit finalizer: a bijection in which every input bit affects every output bit.
fn avalanche(mut value: u64) -> u64 {
    value ^= value >> 33;
    value = value.wrapping_mul(0xff51_afd7_ed55_8ccd);
    value ^= value >> 33;
    value = value.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    value ^ (value >> 33)
}

/// Stable work identity for a source task without its own GUID, scoped by the project GUID.
///
/// UIDs are unique only within one file, so the project GUID keeps equal UIDs from different
/// projects apart, and every re-import of the same file resolves the same work.
pub(crate) fn derived_work_id(project: Uuid, uid: i64) -> WorkItemId {
    let mut hash = FNV_OFFSET;
    let uid = uid.to_be_bytes();
    let bytes = TASK_IDENTITY_DOMAIN
        .iter()
        .chain(project.as_bytes())
        .chain(&uid);
    for byte in bytes {
        hash ^= u128::from(*byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // FNV alone leaves inputs that differ only near the end sharing most high bits; the UID is the
    // last input, so both halves are cross-mixed.
    let (high, low) = ((hash >> 64) as u64, hash as u64);
    let high = avalanche(high ^ low.rotate_left(29));
    let low = avalanche(low ^ high);
    let high = avalanche(high ^ low);
    let mixed = (u128::from(high) << 64) | u128::from(low);
    WorkItemId(uuid::Builder::from_custom_bytes(mixed.to_be_bytes()).into_uuid())
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
