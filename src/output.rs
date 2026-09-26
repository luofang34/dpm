use crate::error::{CliError, io_error};
use dpm_engine::NextWorkResult;
use serde::Serialize;
use std::io::{self, Write};

pub(crate) fn json_blocking(value: &impl Serialize) -> Result<(), CliError> {
    let mut out = io::stdout().lock();
    serde_json::to_writer_pretty(&mut out, value)?;
    writeln!(out).map_err(io_error("write output", "stdout"))?;
    Ok(())
}

pub(crate) fn value_blocking(
    value: &(impl Serialize + std::fmt::Debug),
    json: bool,
) -> Result<(), CliError> {
    if json {
        json_blocking(value)
    } else {
        text_blocking(&format!("{value:#?}"))
    }
}

pub(crate) fn text_blocking(text: &str) -> Result<(), CliError> {
    writeln!(io::stdout().lock(), "{text}").map_err(io_error("write output", "stdout"))?;
    Ok(())
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
            "Scope: projects {}; resources {} ({} of {} eligible in scope)\n",
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
                    .resources
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
