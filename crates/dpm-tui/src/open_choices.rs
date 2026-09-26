use dpm_engine::{ProgressScope, StatusSummary};

/// Short list-row tag for work a choice keeps out of progress.
pub(crate) fn scope_tag(scope: ProgressScope) -> &'static str {
    match scope {
        ProgressScope::Counted => "",
        ProgressScope::NotSelected => " [not selected]",
        ProgressScope::Undecided => " [undecided]",
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
            if let Some(p80) = scenario.p80_finish_hours {
                text.push_str(&format!(" · P80 {p80:.1}h"));
            }
            if !scenario.stranded.is_empty() {
                let stranded: Vec<_> = scenario.stranded.iter().map(|k| k.0.as_str()).collect();
                text.push_str(&format!(" · stranded {}", stranded.join(", ")));
            }
            text.push('\n');
        }
    }
    if !summary.not_applicable.is_empty() {
        text.push_str("\nOutside the active graph:\n");
        for work in &summary.not_applicable {
            text.push_str(&format!("{} — {:?}\n", work.key, work.applicability));
        }
    }
    text
}
