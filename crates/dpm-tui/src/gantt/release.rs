//! Release state of one relation at the snapshot clock, read from the shared [`Timeline`]
//! evaluator so the terminal never restates a gate rule the engine does not apply.

use dpm_engine::Transition;
use dpm_model::{
    Dependency, Endpoint, Plan, Release, StartBasis, StartRelease, Timeline, WorkItemId,
};

/// Which transitions the relation gates and whether each is released now.
pub(crate) fn state(plan: &Plan, timeline: &Timeline, edge: &Dependency) -> String {
    if edge.is_waived() {
        return "waived, not enforced".into();
    }
    let key = plan
        .work_items
        .get(&edge.predecessor)
        .map_or_else(String::new, |w| w.key.to_string());
    let event = edge.kind.predecessor_endpoint();
    if !Transition::Start.governs(edge.kind) {
        let release = timeline.edge(plan, edge);
        return format!(
            "gates submit/verify: {}",
            describe(release, event, &key, None)
        );
    }
    let start = timeline.start_edge(plan, edge);
    let mut text = format!("gates start: {}", start_state(&start, edge, event, &key));
    if edge.start_basis == StartBasis::Provisional {
        // Verification re-reads the edge without the pending attempt.
        let finish = timeline.edge(plan, edge);
        text.push_str(&format!(
            " · gates verify: {}",
            describe(finish, event, &key, None)
        ));
    }
    text
}

/// Decisions gating the work or a containing package, each open or resolved at the clock.
pub(crate) fn decision_gates(plan: &Plan, timeline: &Timeline, work: WorkItemId) -> Option<String> {
    let gates: Vec<_> = timeline
        .decisions(plan, work)
        .into_iter()
        .map(|(decision, release)| {
            let state = if release.released_at().is_some() {
                "resolved"
            } else {
                "open"
            };
            format!("{} {state}", decision.key)
        })
        .collect();
    (!gates.is_empty()).then(|| format!("Decision gates: {}", gates.join(", ")))
}

fn start_state(start: &StartRelease, edge: &Dependency, event: Endpoint, key: &str) -> String {
    match (start.release, start.attempt) {
        (Release::Released { .. }, Some(n)) if start.provisional => {
            format!("released provisionally on attempt #{n}, pending review")
        }
        (Release::AwaitingEvent, _) if edge.start_basis == StartBasis::Provisional => {
            format!("awaiting a submitted attempt or verified finish of {key}")
        }
        (release, attempt) => describe(release, event, key, attempt),
    }
}

fn describe(release: Release, event: Endpoint, key: &str, attempt: Option<u32>) -> String {
    let event = match event {
        Endpoint::Start => "start",
        Endpoint::Finish => "verified finish",
    };
    let basis = attempt.map_or_else(String::new, |n| format!("attempt #{n} submitted; "));
    match release {
        Release::AwaitingEvent => format!("awaiting {event} of {key}"),
        Release::Elapsing { opens_at, .. } => format!(
            "{basis}lag elapses at {}",
            opens_at.format("%Y-%m-%d %H:%M UTC")
        ),
        Release::UnrecordedEventTime => {
            format!("{key} {event} time unrecorded, so positive lag cannot elapse")
        }
        Release::LagOutOfRange { .. } => format!("{basis}lag exceeds the supported time range"),
        Release::Released { .. } => "released".into(),
        Release::SkippedBranch { .. } => {
            format!("{key} not selected; an absent branch of this join")
        }
        Release::NotSelected => format!("{key} not selected; never releases"),
    }
}
