use crate::gantt::dependencies;
use dpm_engine::WorkExplanation;
use dpm_model::Plan;

pub(crate) fn text(plan: &Plan, detail: &WorkExplanation) -> String {
    let work = &detail.work;
    let mut lines = vec![
        format!("{} — {}", work.key, work.title),
        format!(
            "Status: {:?} · {:.0}% · verified={}",
            work.status, detail.progress.percent_complete, detail.progress.verified
        ),
        format!("Objective: {}", work.objective),
    ];
    if let Some(owner) = &work.owner {
        lines.push(format!("Owner: {owner}"));
    }
    section(
        &mut lines,
        "Acceptance",
        work.acceptance.iter().map(|a| a.text.clone()),
    );
    section(
        &mut lines,
        "Execution gates",
        detail.why_now.iter().cloned(),
    );
    if let Some(review) = &work.last_rejection {
        section(
            &mut lines,
            "Latest review rejection",
            [format!(
                "{} at {}: {}",
                review.actor, review.at, review.reason
            )],
        );
    }
    if let Some(instructions) = &work.instructions {
        section(
            &mut lines,
            "Steps",
            instructions.steps.iter().enumerate().map(|(index, step)| {
                format!(
                    "{}. {}\n   Expected: {}",
                    index.saturating_add(1),
                    step.action,
                    step.expected_result
                )
            }),
        );
        section(
            &mut lines,
            "In scope",
            instructions.in_scope.iter().cloned(),
        );
        section(
            &mut lines,
            "Out of scope",
            instructions.out_of_scope.iter().cloned(),
        );
        section(
            &mut lines,
            "Verification",
            instructions.verification.iter().cloned(),
        );
    }
    append_context(&mut lines, detail);
    section(
        &mut lines,
        "Dependencies",
        dependencies::lines(plan, Some(work.id)),
    );
    lines.push(dependencies::LEGEND.into());
    lines.join("\n")
}

fn append_context(lines: &mut Vec<String>, detail: &WorkExplanation) {
    let context = &detail.context;
    section(
        lines,
        "Requirements",
        context
            .requirements
            .iter()
            .map(|r| format!("{}: {} — {}", r.key, r.title, r.statement)),
    );
    section(
        lines,
        "Resources",
        context
            .resources
            .iter()
            .map(|r| format!("{}: {} ({:?})", r.key, r.label, r.kind)),
    );
    section(
        lines,
        "Decisions",
        context.decisions.iter().map(|d| {
            format!(
                "{} ({:?}): {}\n   {}\n   {}",
                d.key,
                d.status,
                d.question,
                d.outcome.as_deref().unwrap_or("unresolved"),
                d.rationale.as_deref().unwrap_or("")
            )
        }),
    );
    section(
        lines,
        "Risks",
        context.risks.iter().map(|r| {
            format!(
                "{} ({:?}, {:.0}%): {}\n   {}",
                r.key,
                r.impact,
                r.probability * 100.0,
                r.description,
                r.mitigation.as_deref().unwrap_or("no mitigation recorded")
            )
        }),
    );
    section(
        lines,
        "Artifacts / sources",
        context
            .artifacts
            .iter()
            .map(|a| format!("{}: {}", a.label, a.uri)),
    );
}

fn section(lines: &mut Vec<String>, title: &str, values: impl IntoIterator<Item = String>) {
    let values: Vec<_> = values.into_iter().collect();
    if !values.is_empty() {
        lines.push(format!("\n{title}:"));
        lines.extend(values);
    }
}
