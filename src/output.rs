use crate::error::{CliError, io_error};
use dpm_engine::NextWorkCandidate;
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

pub(crate) fn candidates_blocking(
    candidates: &[NextWorkCandidate],
    json: bool,
) -> Result<(), CliError> {
    if json {
        return json_blocking(&candidates);
    }
    for candidate in candidates {
        text_blocking(&format!(
            "{}  {}\n  score {:.1}, float {:.1}h, downstream {}\n  {}\n",
            candidate.work.key,
            candidate.work.title,
            candidate.score,
            candidate.total_float_hours,
            candidate.downstream_count,
            candidate.reasons.join("; ")
        ))?;
    }
    Ok(())
}
