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
    if !detail.applicability.is_applicable() {
        lines.push(format!(
            "Applicability: {} — {}",
            crate::open_choices::applicability_label(&detail.applicability),
            dpm_engine::describe_applicability(&detail.applicability)
        ));
    }
    if let Some(owner) = &work.owner {
        lines.push(format!("Owner: {owner}"));
    }
    section(
        &mut lines,
        "Acceptance",
        work.acceptance.iter().map(|a| a.text.clone()),
    );
    execution(&mut lines, detail);
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
    append_schedule(&mut lines, detail);
    append_context(&mut lines, detail);
    section(
        &mut lines,
        "Dependencies",
        dependencies::lines(plan, Some(work.id)),
    );
    lines.push(dependencies::LEGEND.into());
    lines.join("\n")
}

fn append_schedule(lines: &mut Vec<String>, detail: &WorkExplanation) {
    if detail.work.kind == dpm_model::WorkKind::WorkPackage {
        return;
    }
    if let Some(schedule) = &detail.schedule {
        section(
            lines,
            "Remaining schedule (elapsed hours)",
            [
                format!(
                    "Earliest start {:.1} / finish {:.1}; latest start {:.1} / finish {:.1}",
                    schedule.earliest_start_hours,
                    schedule.earliest_finish_hours,
                    schedule.latest_start_hours,
                    schedule.latest_finish_hours
                ),
                format!(
                    "Free float {:.1} / total float {:.1}; critical={}",
                    schedule.free_float_hours, schedule.total_float_hours, schedule.critical
                ),
            ],
        );
    }
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

/// Recorded events, completion time, gate explanations and every transition's eligibility.
fn execution(lines: &mut Vec<String>, detail: &WorkExplanation) {
    let events = [
        ("started", detail.work.events.started_at),
        ("submitted", detail.work.events.submitted_at),
        ("verified", detail.work.events.verified_at),
    ];
    section(
        lines,
        "Events",
        events
            .iter()
            .filter_map(|(name, at)| at.map(|at| format!("{name} at {at}")))
            .chain(detail.progress.completed_at.map(|at| match at {
                dpm_model::EventTime::Recorded(at) => format!("completed at {at}"),
                dpm_model::EventTime::Unrecorded => "completed at an unrecorded time".into(),
            })),
    );
    section(lines, "Execution gates", detail.why_now.iter().cloned());
    section(
        lines,
        "Transitions",
        detail.transitions.iter().map(|(transition, report)| {
            if report.ready {
                format!("{transition}: permitted now")
            } else {
                format!("{transition}: {}", report.reasons().join("; "))
            }
        }),
    );
    section(
        lines,
        "Provisional basis",
        detail
            .basis
            .relies_on
            .iter()
            .map(|s| {
                format!(
                    "relies on {} attempt #{}: {}",
                    s.predecessor_key,
                    s.basis.attempt,
                    basis_state(s)
                )
            })
            .chain(detail.basis.relied_on_by.iter().map(|s| {
                format!(
                    "{} relies on attempt #{}: {}",
                    s.successor_key,
                    s.basis.attempt,
                    basis_state(s)
                )
            })),
    );
}

/// The same derived basis state `explain` returns, including who rejected the attempt.
fn basis_state(status: &dpm_model::BasisStatus) -> String {
    let state = match &status.state {
        dpm_model::BasisState::Pending => "pending review".to_string(),
        dpm_model::BasisState::Verified => "verified".to_string(),
        dpm_model::BasisState::Invalidated {
            rejected_by,
            reason,
            ..
        } => format!("invalidated, rejected by {rejected_by}: {reason}"),
    };
    if status.enforced {
        state
    } else {
        format!("{state} (edge waived)")
    }
}

fn section(lines: &mut Vec<String>, title: &str, values: impl IntoIterator<Item = String>) {
    let values: Vec<_> = values.into_iter().collect();
    if !values.is_empty() {
        lines.push(format!("\n{title}:"));
        lines.extend(values);
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
