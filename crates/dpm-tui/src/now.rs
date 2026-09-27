use dpm_engine::{
    NextWorkCandidate, StatusSummary, describe_applicability, excluded_in_flight, in_status_scope,
};
use dpm_model::{DecisionStatus, Plan, Timeline, WorkStatus};

/// The Now page: headline counts, decisions to make, work needing attention and ready work.
///
/// Every list uses the scope of the count above it, so a list never shows work its count leaves
/// out; work a reviewed choice change kept in flight outside that scope is listed on its own.
pub(crate) fn text(
    plan: &Plan,
    summary: &StatusSummary,
    timeline: &Timeline,
    candidates: Vec<NextWorkCandidate>,
) -> String {
    let mut now = headline(summary);
    now.push_str(&crate::open_choices::summary_text(summary));
    now.push_str("\n\nNeeds decision:\n");
    for decision in plan
        .decisions
        .values()
        .filter(|d| d.status == DecisionStatus::Open)
    {
        now.push_str(&format!("{}: {}\n", decision.key, decision.question));
    }
    for (title, state) in [
        ("Blocked work", WorkStatus::Blocked),
        ("Needs review", WorkStatus::Submitted),
    ] {
        now.push_str(&format!("\n{title}:\n"));
        for work in plan
            .work_items
            .values()
            .filter(|w| w.execution.status == state && in_status_scope(timeline, w.id))
        {
            now.push_str(&format!(
                "{} — {} {}\n",
                work.key,
                work.title,
                work.execution.block_reason.as_deref().unwrap_or("")
            ));
        }
    }
    let excluded = excluded_in_flight(plan, timeline);
    if !excluded.is_empty() {
        now.push_str("\nExcluded in flight:\n");
        for work in excluded {
            now.push_str(&format!(
                "{} — {} [{:?}] {}\n",
                work.key,
                work.title,
                work.execution.status,
                describe_applicability(timeline.applicability(work.id))
            ));
        }
    }
    now.push_str("\nRisks:\n");
    for risk in plan.risks.values() {
        now.push_str(&format!(
            "{} ({:?}): {}\n",
            risk.key, risk.impact, risk.description
        ));
    }
    now.push_str(&ready(candidates));
    now
}

fn headline(summary: &StatusSummary) -> String {
    let mut now = format!(
        "Ready {} · Blocked {} · In flight {} · Awaiting review {} · Complete {} / {} · Decisions {}",
        summary.ready,
        summary.blocked,
        summary.in_flight,
        summary.awaiting_verification,
        summary.complete,
        summary.total_work,
        summary.open_decisions,
    );
    if summary.excluded_in_flight > 0 {
        now.push_str(&format!(
            " · Excluded in flight {}",
            summary.excluded_in_flight
        ));
    }
    now.push_str(&format!(
        "\nExpected remaining: {:.1}h\nExecution: {:.1}% · verified={}",
        summary.expected_finish_hours, summary.progress.percent_complete, summary.progress.verified
    ));
    if let (Some(p50), Some(p80)) = (summary.p50_finish_hours, summary.p80_finish_hours) {
        now.push_str(&format!(" · P50 {p50:.1}h · P80 {p80:.1}h"));
    }
    if let Some(p95) = summary.p95_finish_hours {
        now.push_str(&format!(" · P95 {p95:.1}h"));
    }
    if !summary.unestimated.is_empty() {
        now.push_str(&format!(
            "\nUnestimated {}: counted as 0 h, forecast optimistic: {}",
            summary.unestimated.len(),
            crate::open_choices::key_list(&summary.unestimated)
        ));
    }
    now
}

fn ready(candidates: Vec<NextWorkCandidate>) -> String {
    let mut now = String::from("\nRecommended ready work:\n");
    if candidates.is_empty() {
        now.push_str("No ready work. Inspect Work or Detail for blockers and gates.\n");
    }
    for candidate in candidates {
        now.push_str(&format!(
            "{}  {}  score {:.1}{}\n",
            candidate.work.key,
            candidate.work.title,
            candidate.score,
            if candidate.critical {
                " [critical]"
            } else {
                ""
            }
        ));
    }
    now
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
