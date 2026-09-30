use super::*;
use crate::{Command, apply_command, apply_plan_change, status, status_calibrated};
use chrono::{TimeDelta, TimeZone};
use dpm_model::{
    ActorId, Calendars, Decision, DecisionId, DecisionStatus, OperationId, WorkItemId, WorkStatus,
};

/// A plan and the operations that produced it, built through real commands.
pub(crate) struct Log {
    pub(crate) plan: Plan,
    pub(crate) history: Vec<Operation>,
}

pub(crate) fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
}

/// `hours` after [`t0`], to the millisecond.
pub(crate) fn at(hours: f64) -> DateTime<Utc> {
    t0() + TimeDelta::milliseconds((hours * 3_600_000.0).round() as i64)
}

fn actor(spelling: &str) -> ActorId {
    spelling.parse().expect("actor")
}

impl Log {
    pub(crate) fn new() -> Self {
        Self {
            plan: serde_json::from_str(include_str!(
                "../../../../../tests/support/execution-plan.json"
            ))
            .expect("fixture"),
            history: Vec::new(),
        }
    }

    pub(crate) fn id(&self, key: &str) -> WorkItemId {
        self.plan.find_work_by_key(key).expect("work").id
    }

    pub(crate) fn run(&mut self, who: &str, command: Command, hours: f64) {
        let operation = apply_command(
            &mut self.plan,
            actor(who),
            command,
            at(hours),
            OperationId::new(),
        )
        .expect("command");
        self.history.push(operation);
    }

    pub(crate) fn claim(&mut self, who: &str, key: &str, hours: f64) {
        let work = self.id(key);
        self.run(who, Command::Claim { work }, hours);
    }

    /// Start committed at `hours`, optionally backfilled to an earlier occurrence.
    pub(crate) fn start(&mut self, who: &str, key: &str, hours: f64, occurred: Option<f64>) {
        let work = self.id(key);
        let occurred_at = occurred.map(at);
        self.run(who, Command::Start { work, occurred_at }, hours);
    }

    pub(crate) fn submit(&mut self, who: &str, key: &str, hours: f64, occurred: Option<f64>) {
        let work = self.id(key);
        let occurred_at = occurred.map(at);
        let command = Command::Submit {
            work,
            note: None,
            occurred_at,
        };
        self.run(who, command, hours);
    }

    pub(crate) fn verify(&mut self, who: &str, key: &str, hours: f64) {
        let work = self.id(key);
        let command = Command::Verify {
            work,
            note: None,
            occurred_at: None,
        };
        self.run(who, command, hours);
    }

    pub(crate) fn reject(&mut self, who: &str, key: &str, hours: f64) {
        let work = self.id(key);
        let reason = "acceptance unmet".into();
        self.run(who, Command::Reject { work, reason }, hours);
    }

    pub(crate) fn decide_gate(&mut self, hours: f64) {
        let decision = self
            .plan
            .find_decision_by_key("TEST-GATE")
            .expect("gate")
            .id;
        let outcome = "Proceed".into();
        self.run("human:lead", Command::Decide { decision, outcome }, hours);
    }

    /// Human A (2 h of a 4 h estimate, reviewed by an agent), agent B (rejected once, 2 h 50 min
    /// of 20 h), agent D (backfilled, 16 h of 32 h), bulk-recorded C, unestimated E, and F
    /// released, claimed again and handed off.
    pub(crate) fn scenario() -> Self {
        let mut log = Self::new();
        let e = log.id("TEST-E");
        if let Some(work) = log.plan.work_items.get_mut(&e) {
            work.schedule.estimate = None;
        }
        log.claim("human:alice", "TEST-A", 0.0);
        log.start("human:alice", "TEST-A", 1.0, None);
        log.submit("human:alice", "TEST-A", 3.0, None);
        log.verify("agent:reviewer", "TEST-A", 5.0);
        log.decide_gate(6.0);
        log.claim("agent:coder", "TEST-B", 6.0);
        log.start("agent:coder", "TEST-B", 6.0 + 10.0 / 60.0, None);
        log.submit("agent:coder", "TEST-B", 7.0, None);
        log.reject("human:bob", "TEST-B", 8.0);
        log.submit("agent:coder", "TEST-B", 9.0, None);
        log.verify("human:bob", "TEST-B", 20.0);
        log.claim("agent:coder", "TEST-D", 6.0 + 20.0 / 60.0);
        log.start("agent:coder", "TEST-D", 30.0, Some(10.0));
        log.submit("agent:coder", "TEST-D", 30.0 + 30.0 / 3600.0, Some(26.0));
        log.verify("human:bob", "TEST-D", 40.0);
        log.claim("agent:coder", "TEST-C", 20.5);
        log.start("agent:coder", "TEST-C", 21.0, None);
        log.submit("agent:coder", "TEST-C", 21.0 + 60.0 / 3600.0, None);
        log.verify("human:bob", "TEST-C", 22.0);
        log.claim("human:alice", "TEST-E", 41.0);
        log.start("human:alice", "TEST-E", 42.0, None);
        log.submit("human:alice", "TEST-E", 43.0, None);
        log.verify("agent:reviewer", "TEST-E", 44.0);
        let f = log.id("TEST-F");
        log.claim("agent:coder", "TEST-F", 45.0);
        let reason = "wrong task".to_string();
        log.run("agent:coder", Command::Release { work: f, reason }, 46.0);
        log.claim("agent:other", "TEST-F", 47.0);
        let handoff = Command::Handoff {
            work: f,
            from: actor("agent:other"),
            to: actor("agent:third"),
            reason: "reassigned".into(),
        };
        log.run("human:lead", handoff, 48.0);
        log
    }

    pub(crate) fn report(&self, hours: f64) -> CalibrationReport {
        calibration(&self.plan, &self.history, at(hours)).expect("calibration")
    }
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-9, "{actual} != {expected}");
}

fn keys(exclusion: &Exclusion) -> Vec<&str> {
    exclusion.keys.iter().map(|k| k.0.as_str()).collect()
}

#[test]
fn verified_work_is_measured_per_executor_kind_and_capability() {
    let log = Log::scenario();
    let report = log.report(50.0);
    let samples: Vec<_> = report
        .estimates
        .samples
        .iter()
        .map(|s| (s.key.0.as_str(), s.executor, s.actual_hours, s.ratio))
        .collect();
    assert_eq!(samples.len(), 3, "{samples:?}");
    let b_actual = 3.0 - 10.0 / 60.0;
    for ((key, executor, actual, ratio), expected) in samples.iter().zip([
        ("TEST-A", ActorKind::Human, 2.0, 0.5),
        ("TEST-B", ActorKind::Agent, b_actual, b_actual / 20.0),
        ("TEST-D", ActorKind::Agent, 16.0, 0.5),
    ]) {
        assert_eq!((*key, *executor), (expected.0, expected.1));
        close(*actual, expected.2);
        close(*ratio, expected.3);
    }
    let agent = &report.estimates.by_executor;
    assert_eq!(agent.len(), 2);
    let agents = agent
        .iter()
        .find(|g| g.executor == ActorKind::Agent)
        .expect("agent");
    assert_eq!((agents.samples, agents.sufficient), (2, false));
    close(agents.median, (b_actual / 20.0 + 0.5) / 2.0);
    let rust = report
        .estimates
        .by_capability
        .iter()
        .find(|g| g.executor == ActorKind::Agent && g.capability.as_deref() == Some("rust"))
        .expect("agent rust");
    assert_eq!(rust.samples, 2);
    assert_eq!(report.rules.actual_hours, HourBasis::Elapsed);
    assert_eq!(report.history.operations, log.history.len());
    assert_eq!(report.history.first_at, Some(at(0.0)));
}

#[test]
fn bulk_recorded_and_unestimated_work_is_excluded_with_its_reason() {
    let report = Log::scenario().report(50.0);
    let excluded: Vec<_> = report
        .estimates
        .excluded
        .iter()
        .map(|e| (e.reason, e.count, keys(e)))
        .collect();
    assert_eq!(
        excluded,
        [
            (ExclusionReason::Unestimated, 1, vec!["TEST-E"]),
            (ExclusionReason::BulkRecorded, 1, vec!["TEST-C"]),
        ]
    );
    let json = serde_json::to_value(&report.estimates.excluded).expect("json");
    assert_eq!(json[1]["reason"], "bulk_recorded");
}

#[test]
fn work_started_before_the_history_is_not_in_history() {
    let mut log = Log::scenario();
    // The log a store holds may begin after the work started, as for an imported snapshot.
    let start = log
        .history
        .iter()
        .position(|op| matches!(op.command, Command::Start { .. }))
        .expect("first start");
    log.history.drain(..=start);
    let report = log.report(50.0);
    let not_in_history = report
        .estimates
        .excluded
        .iter()
        .find(|e| e.reason == ExclusionReason::NotInHistory)
        .expect("excluded");
    assert_eq!(keys(not_in_history), ["TEST-A"]);
}

#[test]
fn review_waits_count_rejections_and_verifications_per_reviewer_kind() {
    let report = Log::scenario().report(50.0);
    let groups: Vec<_> = report
        .reviews
        .by_kind
        .iter()
        .map(|g| (g.kind, g.count, g.median_hours))
        .collect();
    // Agent reviews: A after 2 h, E after 1 h. Human: B rejected after 1 h and verified after
    // 11 h, D verified 14 h after its backfilled submission; bulk-recorded C is not measured.
    assert_eq!(
        groups,
        [(ActorKind::Human, 3, 11.0), (ActorKind::Agent, 2, 1.5)]
    );
    let bulk = report.reviews.excluded.first().expect("excluded");
    assert_eq!(
        (bulk.reason, keys(bulk)),
        (ExclusionReason::BulkRecorded, vec!["TEST-C"])
    );
}

#[test]
fn decision_waits_run_from_the_reviewed_change_that_opened_them() {
    let mut log = Log::scenario();
    let current = log.plan.clone();
    let mut proposed = current.clone();
    let gate = current
        .find_decision_by_key("TEST-GATE")
        .expect("gate")
        .clone();
    let question = Decision {
        id: DecisionId::new(),
        key: dpm_model::Key::new("TEST-Q"),
        question: "Which follow-up?".into(),
        status: DecisionStatus::Open,
        outcome: None,
        resolved_at: None,
        blocks: Default::default(),
        supersedes: None,
        ..gate.clone()
    };
    let replacement = Decision {
        id: DecisionId::new(),
        key: dpm_model::Key::new("TEST-GATE-2"),
        status: DecisionStatus::Decided,
        outcome: Some("Proceed with review".into()),
        resolved_at: None,
        rationale: Some("clarified".into()),
        blocks: Default::default(),
        supersedes: Some(gate.id),
        ..gate.clone()
    };
    if let Some(old) = proposed.decisions.get_mut(&gate.id) {
        old.status = DecisionStatus::Superseded;
    }
    proposed.decisions.insert(question.id, question.clone());
    proposed.decisions.insert(replacement.id, replacement);
    let operation = apply_plan_change(
        &mut log.plan,
        ActorId::human("lead"),
        &proposed,
        "follow-up question",
        at(50.0),
        OperationId::new(),
    )
    .expect("change");
    log.history.push(operation);
    let decide = Command::Decide {
        decision: question.id,
        outcome: "Ship".into(),
    };
    log.run("service:ci", decide, 55.0);

    let report = log.report(60.0);
    let groups: Vec<_> = report
        .decisions
        .by_kind
        .iter()
        .map(|g| (g.kind, g.count, g.median_hours))
        .collect();
    assert_eq!(groups, [(ActorKind::Service, 1, 5.0)]);
    let excluded: Vec<_> = report
        .decisions
        .excluded
        .iter()
        .map(|e| (e.reason, keys(e)))
        .collect();
    assert_eq!(
        excluded,
        [
            (ExclusionReason::OpenedBeforeHistory, vec!["TEST-GATE"]),
            (ExclusionReason::Replacement, vec!["TEST-GATE-2"]),
        ]
    );
}

#[test]
fn working_hours_on_the_tasks_calendar_are_the_actual_time() {
    let mut log = Log::new();
    log.plan.calendars = Some(Calendars::in_zone("Europe/Berlin"));
    // Friday 4 September 2026, 16:00 in Berlin, to Monday 10:00: 66 elapsed hours, of which
    // the standard calendar works 17:00-16:00 on Friday and 08:00-10:00 on Monday.
    let friday = 3.0 * 24.0 + 14.0;
    log.claim("human:alice", "TEST-A", friday - 1.0);
    log.start("human:alice", "TEST-A", friday, None);
    log.submit("human:alice", "TEST-A", friday + 66.0, None);
    log.verify("human:bob", "TEST-A", friday + 67.0);
    let report = log.report(friday + 70.0);
    assert_eq!(report.rules.actual_hours, HourBasis::Working);
    let sample = report.estimates.samples.first().expect("sample");
    close(sample.actual_hours, 3.0);
    close(sample.ratio, 0.75);
}

#[test]
fn a_calibrated_status_applies_sufficient_factors_to_a_copy_only() {
    let log = Log::scenario();
    let plain = status(&log.plan, false, at(50.0)).expect("status");
    let plain_json = serde_json::to_value(&plain).expect("json");
    assert!(plain_json.get("calibration").is_none());

    let mut report = log.report(50.0);
    let insufficient = status_calibrated(&log.plan, false, at(50.0), &report).expect("status");
    let applied = insufficient.calibration.as_ref().expect("calibration");
    assert!(
        applied
            .factors
            .iter()
            .all(|f| !f.applied && f.factor == 1.0)
    );
    assert!(!applied.review_delay.applied);
    close(
        insufficient.expected_finish_hours,
        plain.expected_finish_hours,
    );

    for group in &mut report.estimates.by_executor {
        group.sufficient = true;
        group.median = 0.1;
    }
    for group in &mut report.reviews.by_kind {
        group.sufficient = true;
    }
    let before = log.plan.clone();
    let calibrated = status_calibrated(&log.plan, false, at(50.0), &report).expect("status");
    // Only TEST-F (4 h, held by an agent) remains: 0.4 h of work, then the 11 h human review.
    close(plain.expected_finish_hours, 4.0);
    close(calibrated.expected_finish_hours, 0.4 + 11.0);
    let applied = calibrated.calibration.expect("calibration");
    let agent = applied.factors.first().expect("factor");
    assert_eq!(
        (agent.executor, agent.tasks, agent.applied),
        (ActorKind::Agent, 1, true)
    );
    close(applied.review_delay.hours, 11.0);
    assert_eq!(log.plan, before, "the stored plan is never rewritten");
    let status = log.plan.work_items.get(&log.id("TEST-F")).expect("F");
    assert_eq!(status.execution.status, WorkStatus::Claimed);
}

#[test]
fn a_review_recorded_with_its_submission_is_no_wait() {
    let mut log = Log::new();
    log.claim("agent:coder", "TEST-A", 0.0);
    log.start("agent:coder", "TEST-A", 1.0, None);
    log.submit("agent:coder", "TEST-A", 3.0, None);
    log.verify("human:bob", "TEST-A", 3.0 + 5.0 / 3600.0);
    let report = log.report(4.0);
    assert!(report.reviews.by_kind.is_empty());
    let excluded = report.reviews.excluded.first().expect("excluded");
    assert_eq!(
        (excluded.reason, excluded.count, keys(excluded)),
        (ExclusionReason::ReviewedOnSubmission, 1, vec!["TEST-A"])
    );
    // The execution itself is still measured: 2 h of a 4 h estimate.
    close(report.estimates.samples.first().expect("sample").ratio, 0.5);
}
