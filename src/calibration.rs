//! `status` and `calibration`: forecasts, and what history says about estimates and flow.

use crate::{error::CliError, output};
use dpm_app::{Application, Query};
use dpm_engine::{CalibrationReport, Exclusion, RatioGroup, WaitReport};

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

fn summary(report: &CalibrationReport) -> String {
    let mut lines = vec![format!(
        "History: {} operation(s); actual time in {:?} hours; groups need {} samples to apply",
        report.history.operations, report.rules.actual_hours, report.rules.min_samples
    )];
    lines.push(format!(
        "Estimates: {} sample(s)",
        report.estimates.samples.len()
    ));
    lines.extend(report.estimates.by_executor.iter().map(ratio));
    lines.extend(report.estimates.excluded.iter().map(excluded));
    lines.extend(waits("Reviews", &report.reviews));
    lines.extend(waits("Decisions", &report.decisions));
    let flow = &report.flow;
    let hours = |value: Option<f64>| value.map_or("-".into(), |h| format!("{h:.1}h"));
    lines.push(format!(
        "Flow: cycle median {} over {}, lead median {} over {}; verified {} in 7 days, {} in 28",
        hours(flow.cycle_time.median_hours),
        flow.cycle_time.count,
        hours(flow.lead_time.median_hours),
        flow.lead_time.count,
        flow.throughput.last_7_days,
        flow.throughput.last_28_days
    ));
    let total = &flow.reliability.total;
    lines.push(format!(
        "Claims: {} episode(s): {} verified ({} after rejection), {} released, {} handed off, {} open",
        total.episodes,
        total.verified,
        total.verified_after_rejection,
        total.released,
        total.handed_off,
        total.open
    ));
    lines.extend(flow.aging.iter().map(|w| {
        format!(
            "  aging {} {:?} held {}",
            w.key,
            w.status,
            hours(w.hours_since_claim)
        )
    }));
    lines.join("\n")
}

fn ratio(group: &RatioGroup) -> String {
    format!(
        "  {:?}: median {:.3} (IQR {:.3}-{:.3}) over {}{}",
        group.executor,
        group.median,
        group.p25,
        group.p75,
        group.samples,
        if group.sufficient {
            ""
        } else {
            ", too few to apply"
        }
    )
}

fn excluded(exclusion: &Exclusion) -> String {
    format!("  excluded {:?}: {}", exclusion.reason, exclusion.count)
}

fn waits(label: &str, report: &WaitReport) -> Vec<String> {
    let mut lines = vec![format!("{label}:")];
    lines.extend(report.by_kind.iter().map(|g| {
        format!(
            "  {:?}: median {:.1}h, p80 {:.1}h over {}",
            g.kind, g.median_hours, g.p80_hours, g.count
        )
    }));
    lines.extend(report.excluded.iter().map(excluded));
    lines
}
