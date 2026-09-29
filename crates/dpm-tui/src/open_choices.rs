use dpm_engine::{ProgressScope, StatusSummary};
use dpm_model::{Applicability, Key};

/// Short name of a derived applicability state, shared by every TUI surface that shows it.
pub(crate) fn applicability_label(applicability: &Applicability) -> &'static str {
    match applicability {
        Applicability::Applicable => "applicable",
        Applicability::Undecided { .. } => "undecided",
        Applicability::NotSelected { .. } => "not selected",
        Applicability::AwaitingChoice { .. } => "awaiting a choice",
        Applicability::Stranded { .. } => "stranded",
        Applicability::EmptyJoin => "empty join",
        Applicability::AllChildrenExcluded { .. } => "all children excluded",
        Applicability::ChildrenStranded { .. } => "children stranded",
    }
}

/// List-row tag for work a choice keeps out of progress, or that is otherwise outside the
/// active graph although progress still counts it.
pub(crate) fn scope_tag(scope: ProgressScope, applicability: &Applicability) -> String {
    match scope {
        ProgressScope::NotSelected => " [not selected]".into(),
        ProgressScope::Undecided => " [undecided]".into(),
        ProgressScope::Counted if applicability.is_applicable() => String::new(),
        ProgressScope::Counted => format!(" [{}]", applicability_label(applicability)),
    }
}

/// Label and engine explanation of why work is outside the active graph.
pub(crate) fn applicability_text(applicability: &Applicability) -> String {
    format!(
        "{}: {}",
        applicability_label(applicability),
        dpm_engine::describe_applicability(applicability)
    )
}

/// Keys named before the rest of a list is summarized as a count, so one line stays readable.
const KEYS_SHOWN: usize = 5;

/// Comma-separated keys, truncated with the count of those left out.
pub(crate) fn key_list(keys: &[Key]) -> String {
    let shown: Vec<_> = keys.iter().take(KEYS_SHOWN).map(|k| k.0.as_str()).collect();
    match keys.len().saturating_sub(KEYS_SHOWN) {
        0 => shown.join(", "),
        more => format!("{} (+{more} more)", shown.join(", ")),
    }
}

/// Excluded work and one forecast per open-choice scenario, never a blended percentile.
pub(crate) fn summary_text(summary: &StatusSummary) -> String {
    let mut text = String::new();
    if let Some(choices) = &summary.open_choices {
        let keys: Vec<_> = choices.decisions.iter().map(|k| k.0.as_str()).collect();
        text.push_str(&format!(
            "\nOpen choices {}: expected remaining covers committed work only; {} scenario(s)\n",
            keys.join(", "),
            choices.scenario_count
        ));
        for scenario in &choices.scenarios {
            let picks: Vec<_> = scenario
                .choices
                .iter()
                .map(|(decision, option)| format!("{decision}={option}"))
                .collect();
            text.push_str(&format!(
                "  {}: {:.1}h",
                picks.join(" "),
                scenario.expected_finish_hours
            ));
            for (label, value) in [
                ("P50", scenario.p50_finish_hours),
                ("P80", scenario.p80_finish_hours),
                ("P95", scenario.p95_finish_hours),
            ] {
                if let Some(value) = value {
                    text.push_str(&format!(" · {label} {value:.1}h"));
                }
            }
            if !scenario.stranded.is_empty() {
                let stranded: Vec<_> = scenario.stranded.iter().map(|k| k.0.as_str()).collect();
                text.push_str(&format!(" · stranded {}", stranded.join(", ")));
            }
            if !scenario.unestimated.is_empty() {
                text.push_str(&format!(
                    " · unestimated (0 h) {}",
                    key_list(&scenario.unestimated)
                ));
            }
            text.push('\n');
        }
    }
    if !summary.not_applicable.is_empty() {
        text.push_str("\nOutside the active graph:\n");
        for work in &summary.not_applicable {
            text.push_str(&format!(
                "{} — {}\n",
                work.key,
                applicability_text(&work.applicability)
            ));
        }
    }
    text
}

#[cfg(test)]
mod tests;
