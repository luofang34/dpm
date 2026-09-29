use crate::error::{CliError, io_error};
use dpm_app::{Envelope, QueryResponse, RecordedOperation};
use dpm_engine::NextWorkResult;
use serde::Serialize;
use std::io::{self, Write};

pub(crate) fn json_blocking(value: &impl Serialize) -> Result<(), CliError> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out).map_err(io_error("write output", "stdout"))?;
    Ok(())
}

/// Write a `--json` success in the envelope agent tools return; `revision` is `None` when the
/// command reads no workspace.
pub(crate) fn success_blocking(
    revision: Option<u64>,
    data: &impl Serialize,
) -> Result<(), CliError> {
    json_blocking(&Envelope::new(revision, data))
}

pub(crate) fn value_blocking(
    value: &(impl Serialize + std::fmt::Debug),
    revision: Option<u64>,
    json: bool,
) -> Result<(), CliError> {
    if json {
        success_blocking(revision, value)
    } else {
        text_blocking(&format!("{value:#?}"))
    }
}

/// Present a query result: the envelope for `--json`, the pretty data otherwise.
pub(crate) fn response_blocking(response: QueryResponse, json: bool) -> Result<(), CliError> {
    if json {
        json_blocking(&Envelope::from(response))
    } else {
        text_blocking(&serde_json::to_string_pretty(&response.data)?)
    }
}

/// A mutation's recorded operation, enveloped at its resulting revision.
pub(crate) fn operation_blocking(operation: RecordedOperation, json: bool) -> Result<(), CliError> {
    if json {
        json_blocking(&Envelope::from(operation))
    } else {
        text_blocking(&format!("{operation:#?}"))
    }
}

pub(crate) fn text_blocking(text: &str) -> Result<(), CliError> {
    writeln!(io::stdout().lock(), "{text}").map_err(io_error("write output", "stdout"))?;
    Ok(())
}

/// Write a complete document exactly, without an added line break.
pub(crate) fn raw_blocking(text: &str) -> Result<(), CliError> {
    io::stdout()
        .lock()
        .write_all(text.as_bytes())
        .map_err(io_error("write output", "stdout"))
}

/// Keys shown before the remainder is summarized as a count.
const UNESTIMATED_SHOWN: usize = 5;

/// One line naming the tasks the forecast counts as 0 h, or `None` when every task is estimated.
pub(crate) fn unestimated_line(keys: &[dpm_model::Key]) -> Option<String> {
    if keys.is_empty() {
        return None;
    }
    let shown: Vec<_> = keys
        .iter()
        .take(UNESTIMATED_SHOWN)
        .map(|k| k.0.as_str())
        .collect();
    let more = keys.len().saturating_sub(UNESTIMATED_SHOWN);
    Some(format!(
        "Unestimated: {} task(s) count as 0 h, so the forecast is optimistic: {}{}",
        keys.len(),
        shown.join(", "),
        if more > 0 {
            format!(" (+{more} more)")
        } else {
            String::new()
        }
    ))
}

pub(crate) fn next_text_blocking(result: &NextWorkResult) -> Result<(), CliError> {
    if !result.scope.is_unscoped() {
        let keys = |members: Vec<String>| {
            if members.is_empty() {
                "any".to_string()
            } else {
                members.join(", ")
            }
        };
        text_blocking(&format!(
            "Scope: projects {}; assets {} ({} of {} eligible in scope)\n",
            keys(
                result
                    .scope
                    .projects
                    .iter()
                    .map(|m| m.key.0.clone())
                    .collect()
            ),
            keys(
                result
                    .scope
                    .assets
                    .iter()
                    .map(|m| m.key.0.clone())
                    .collect()
            ),
            result.in_scope_count,
            result.eligible_count,
        ))?;
    }
    for scoped in &result.candidates {
        let candidate = &scoped.candidate;
        text_blocking(&format!(
            "#{} {}  {}\n  score {:.1}, float {:.1}h, downstream {}\n  {}\n",
            scoped.global_rank,
            candidate.work.key,
            candidate.work.title,
            candidate.score,
            candidate.total_float_hours,
            candidate.downstream_count,
            candidate.reasons.join("; ")
        ))?;
    }
    let outside = &result.outside_scope;
    if outside.count > 0 {
        let higher = outside
            .keys
            .iter()
            .take(outside.higher_ranked_count)
            .map(|k| k.0.as_str())
            .collect::<Vec<_>>();
        text_blocking(&format!(
            "Outside scope: {} eligible, {} ranked higher{}",
            outside.count,
            outside.higher_ranked_count,
            if higher.is_empty() {
                String::new()
            } else {
                format!(": {}", higher.join(", "))
            }
        ))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
