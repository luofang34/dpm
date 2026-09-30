//! `status` and `calibration`: forecasts, and what history says about estimates and flow.

use crate::{error::CliError, output};
use dpm_app::{Application, Query};
use dpm_engine::{
    CalibrationReport, ClaimOutcomes, Durations, Exclusion, FlowReport, RatioGroup, WaitReport,
};
use serde::Serialize;

/// `status`, with measured factors applied when `calibrated`.
pub(crate) fn status_blocking(
    app: &Application,
    probabilistic: bool,
    calibrated: bool,
    json: bool,
) -> Result<(), CliError> {
    let response = app.query_blocking(Query::Status {
        probabilistic,
        calibrated,
    })?;
    if json {
        return output::response_blocking(response, true);
    }
    let unestimated: Vec<dpm_model::Key> = serde_json::from_value(
        response
            .data
            .get("unestimated")
            .cloned()
            .unwrap_or_default(),
    )
    .unwrap_or_default();
    if let Some(line) = output::unestimated_line(&unestimated) {
        output::text_blocking(&line)?;
    }
    output::text_blocking(&serde_json::to_string_pretty(&response.data)?)
}

/// `calibration`: the report for `--json`, otherwise a short summary.
pub(crate) fn calibration_blocking(app: &Application, json: bool) -> Result<(), CliError> {
    let response = app.query_blocking(Query::Calibration)?;
    if json {
        return output::response_blocking(response, true);
    }
    let report: CalibrationReport = serde_json::from_value(response.data)?;
    output::text_blocking(&summary(&report))
}

/// Excluded keys are listed in the text summary up to this many per reason; `--json` has them all.
const LISTED_KEYS: usize = 5;

fn summary(report: &CalibrationReport) -> String {
    let mut lines = vec![format!(
        "History: {} operation(s); actual time in {} hours; groups need {} samples to apply",
        report.history.operations,
        snake(report.rules.actual_hours),
        report.rules.min_samples
    )];
    lines.push(format!(
        "Estimates: {} sample(s)",
        report.estimates.samples.len()
    ));
    lines.extend(report.estimates.by_executor.iter().map(ratio));
    // Groups holding only exclusions are in the JSON; the summary keeps to measured ones.
    let capabilities: Vec<String> = report
        .estimates
        .by_capability
        .iter()
        .filter_map(|g| {
            g.median.map(|median| {
                format!(
                    "{}/{} {median:.3} over {}",
                    g.executor,
                    g.capability.as_deref().unwrap_or("-"),
                    g.samples
                )
            })
        })
        .collect();
    if !capabilities.is_empty() {
        lines.push(format!("  by capability: {}", capabilities.join(", ")));
    }
    lines.extend(report.estimates.excluded.iter().map(excluded));
    lines.extend(waits("Reviews", &report.reviews));
    lines.extend(waits("Decisions", &report.decisions));
    lines.extend(flow(&report.flow));
    lines.join("\n")
}

fn flow(flow: &FlowReport) -> Vec<String> {
    let hours = |value: Option<f64>| value.map_or("-".into(), |h| format!("{h:.1}h"));
    let skipped = |d: &Durations| d.excluded.iter().map(|e| e.count).sum::<usize>();
    let mut lines = vec![format!(
        "Flow: cycle median {} over {} ({} excluded), lead median {} over {} ({} excluded); \
         verified {} in 7 days, {} in 28",
        hours(flow.cycle_time.median_hours),
        flow.cycle_time.count,
        skipped(&flow.cycle_time),
        hours(flow.lead_time.median_hours),
        flow.lead_time.count,
        skipped(&flow.lead_time),
        flow.throughput.last_7_days,
        flow.throughput.last_28_days
    )];
    lines.push(format!("Claims: {}", outcomes(&flow.reliability.total)));
    lines.extend(
        flow.reliability
            .by_holder
            .iter()
            .map(|h| format!("  {}: {}", h.holder, outcomes(&h.outcomes))),
    );
    lines.extend(flow.aging.iter().map(|w| {
        format!(
            "  aging {} {} held {}",
            w.key,
            snake(w.status),
            hours(w.hours_since_claim)
        )
    }));
    lines
}

fn outcomes(outcomes: &ClaimOutcomes) -> String {
    format!(
        "{} episode(s): {} verified ({} after rejection), {} released, {} handed off, {} open; \
         verified fraction {}",
        outcomes.episodes,
        outcomes.verified,
        outcomes.verified_after_rejection,
        outcomes.released,
        outcomes.handed_off,
        outcomes.open,
        outcomes
            .verified_fraction
            .map_or("-".into(), |f| format!("{:.0}%", f * 100.0))
    )
}

fn ratio(group: &RatioGroup) -> String {
    let spread = match (group.median, group.p25, group.p75) {
        (Some(median), Some(p25), Some(p75)) => {
            format!("median {median:.3} (IQR {p25:.3}-{p75:.3})")
        }
        _ => "no measurement".into(),
    };
    let mut line = format!(
        "  {}: {spread} over {}{}",
        group.executor,
        group.samples,
        if group.sufficient {
            ""
        } else {
            ", too few to apply"
        }
    );
    if !group.excluded.is_empty() {
        let reasons: Vec<String> = group
            .excluded
            .iter()
            .map(|r| format!("{} {}", snake(r.reason), r.count))
            .collect();
        line.push_str(&format!("; excluded {}", reasons.join(", ")));
    }
    line
}

fn excluded(exclusion: &Exclusion) -> String {
    let mut line = format!(
        "  excluded {}: {}",
        snake(exclusion.reason),
        exclusion.count
    );
    if exclusion.keys.len() <= LISTED_KEYS {
        let keys: Vec<&str> = exclusion.keys.iter().map(|k| k.0.as_str()).collect();
        line.push_str(&format!(" ({})", keys.join(", ")));
    }
    line
}

fn waits(label: &str, report: &WaitReport) -> Vec<String> {
    let mut lines = vec![format!("{label}:")];
    lines.extend(report.by_kind.iter().map(|g| {
        format!(
            "  {}: median {:.1}h, p80 {:.1}h over {}",
            g.kind, g.median_hours, g.p80_hours, g.count
        )
    }));
    lines.extend(report.excluded.iter().map(excluded));
    lines
}

/// The snake_case spelling of a value's serialized name, such as `in_progress` for a status.
fn snake(value: impl Serialize) -> String {
    let name = serde_json::to_value(value)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default();
    let mut spelled = String::with_capacity(name.len());
    for (position, character) in name.chars().enumerate() {
        if character.is_ascii_uppercase() {
            if position > 0 {
                spelled.push('_');
            }
            spelled.push(character.to_ascii_lowercase());
        } else {
            spelled.push(character);
        }
    }
    spelled
}

#[cfg(test)]
mod tests;
