use crate::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

fn fixture() -> Plan {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture");
    plan.validate().expect("valid fixture");
    plan
}

fn t(hours: i64) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 0, 0, 0)
        .single()
        .expect("fixed time")
        + TimeDelta::hours(hours)
}

fn id(plan: &Plan, key: &str) -> WorkItemId {
    plan.find_work_by_key(key).expect("work").id
}

fn decide(plan: &mut Plan, option: &str, at: Option<DateTime<Utc>>) {
    let decision = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "DEC-SUPPLIER")
        .expect("decision");
    decision.status = DecisionStatus::Decided;
    decision.outcome = Some(option.into());
    decision.resolved_at = at;
    plan.validate().expect("valid decision");
}

fn verify(plan: &mut Plan, key: &str, at: DateTime<Utc>) {
    let work = plan.find_work_by_key_mut(key).expect("work");
    work.status = WorkStatus::Verified;
    work.owner = Some(ActorId::agent("worker"));
    work.events.started_at = Some(at);
    work.events.submitted_at = Some(at);
    work.events.verified_at = Some(at);
}

fn state(plan: &Plan, key: &str) -> Applicability {
    plan.applicability()[&id(plan, key)].clone()
}

#[test]
fn an_open_choice_leaves_both_branches_undecided_and_their_join_uncommitted() {
    let plan = fixture();
    assert_eq!(state(&plan, "SUP-DESIGN"), Applicability::Applicable);
    for key in [
        "SUP-PKG-A",
        "SUP-A-QUOTE",
        "SUP-A-TEST",
        "SUP-A-QUAL",
        "SUP-B-QUAL",
    ] {
        assert!(
            matches!(state(&plan, key), Applicability::Undecided { .. }),
            "{key}"
        );
    }
    for key in ["SUP-MERGE", "SUP-BUILD", "SUP-A-AUDIT"] {
        assert!(
            matches!(state(&plan, key), Applicability::AwaitingChoice { .. }),
            "{key}"
        );
    }
    let timeline = Timeline::at(&plan, t(10));
    assert!(
        timeline.completed().is_empty(),
        "an unknown choice is not completion"
    );
}

#[test]
fn a_choice_selects_one_subtree_through_nested_packages() {
    let mut plan = fixture();
    decide(&mut plan, "B", Some(t(1)));
    for key in ["SUP-PKG-A", "SUP-A-QUOTE", "SUP-A-TEST", "SUP-A-QUAL"] {
        assert_eq!(
            state(&plan, key),
            Applicability::NotSelected {
                decision: Key::new("DEC-SUPPLIER"),
                option: "A".into(),
                selected: "B".into(),
            },
            "{key} inherits the package condition"
        );
    }
    for key in [
        "SUP-PKG-B",
        "SUP-B-QUOTE",
        "SUP-B-QUAL",
        "SUP-MERGE",
        "SUP-BUILD",
    ] {
        assert_eq!(state(&plan, key), Applicability::Applicable, "{key}");
    }
}

#[test]
fn waiving_the_soft_edge_from_skipped_work_releases_its_stranded_successor() {
    let mut plan = fixture();
    decide(&mut plan, "B", Some(t(1)));
    let audit = id(&plan, "SUP-A-AUDIT");
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.successor == audit)
        .expect("edge");
    edge.policy = DependencyPolicy::Soft;
    edge.waiver = Some(DependencyWaiver {
        actor: ActorId::human("lead"),
        at: t(2),
        reason: "audit covers supplier B instead".into(),
    });
    plan.validate().expect("valid waiver");
    assert_eq!(state(&plan, "SUP-A-AUDIT"), Applicability::Applicable);
}

#[test]
fn an_ordinary_dependency_on_skipped_work_strands_its_successor() {
    let mut plan = fixture();
    decide(&mut plan, "B", Some(t(1)));
    let audit = state(&plan, "SUP-A-AUDIT");
    assert!(
        matches!(&audit, Applicability::Stranded { predecessor, .. } if predecessor.0 == "SUP-A-QUOTE"),
        "{audit:?}"
    );
    let timeline = Timeline::at(&plan, t(2));
    let edge = plan
        .dependencies
        .iter()
        .find(|d| d.successor == id(&plan, "SUP-A-AUDIT"))
        .expect("edge");
    assert_eq!(timeline.edge(&plan, edge), Release::NotSelected);

    // Without an explicit branch join the merge point is stranded too, and so is its successor.
    plan.find_work_by_key_mut("SUP-MERGE").expect("merge").join = JoinPolicy::AllPredecessors;
    assert!(matches!(
        state(&plan, "SUP-MERGE"),
        Applicability::Stranded { .. }
    ));
    assert!(matches!(
        state(&plan, "SUP-BUILD"),
        Applicability::Stranded { .. }
    ));
}

#[test]
fn the_join_completes_only_with_the_selected_verified_branch_and_the_choice_time() {
    let mut plan = fixture();
    decide(&mut plan, "A", Some(t(1)));
    let merge = id(&plan, "SUP-MERGE");
    assert!(!Timeline::at(&plan, t(9)).completed().contains(&merge));
    for key in ["SUP-DESIGN", "SUP-A-QUOTE"] {
        verify(&mut plan, key, t(2));
    }
    verify(&mut plan, "SUP-A-QUAL", t(3));
    let timeline = Timeline::at(&plan, t(9));
    assert_eq!(
        timeline.completed_at(merge),
        Some(EventTime::Recorded(t(3)))
    );
    let skipped = plan
        .dependencies
        .iter()
        .find(|d| d.predecessor == id(&plan, "SUP-B-QUAL"))
        .expect("edge");
    assert_eq!(
        timeline.edge(&plan, skipped),
        Release::SkippedBranch {
            at: EventTime::Recorded(t(1))
        }
    );
    let package = id(&plan, "SUP-PKG-A");
    assert_eq!(
        timeline.completed_at(package),
        Some(EventTime::Recorded(t(3)))
    );
    assert_eq!(timeline.completed_at(id(&plan, "SUP-PKG-B")), None);
}

#[test]
fn an_all_skipped_join_completes_only_when_it_permits_an_empty_result() {
    let mut plan = fixture();
    let b = plan.find_work_by_key_mut("SUP-PKG-B").expect("package");
    b.condition.as_mut().expect("condition").option = "A".into();
    decide(&mut plan, "B", Some(t(4)));
    assert_eq!(state(&plan, "SUP-MERGE"), Applicability::EmptyJoin);
    assert!(matches!(
        state(&plan, "SUP-BUILD"),
        Applicability::Stranded { .. }
    ));
    let merge = id(&plan, "SUP-MERGE");
    assert!(!Timeline::at(&plan, t(9)).completed().contains(&merge));

    plan.find_work_by_key_mut("SUP-MERGE").expect("merge").join =
        JoinPolicy::ActiveBranches { allow_empty: true };
    assert_eq!(state(&plan, "SUP-MERGE"), Applicability::Applicable);
    assert_eq!(
        Timeline::at(&plan, t(9)).completed_at(merge),
        Some(EventTime::Recorded(t(4))),
        "the choice that skipped every branch sets the join time"
    );
}

#[test]
fn verified_work_counts_only_while_its_option_is_selected() {
    let mut plan = fixture();
    decide(&mut plan, "A", Some(t(1)));
    verify(&mut plan, "SUP-DESIGN", t(2));
    verify(&mut plan, "SUP-A-QUOTE", t(2));
    let quote = id(&plan, "SUP-A-QUOTE");
    assert!(Timeline::at(&plan, t(3)).completed().contains(&quote));
    // A reviewed replacement switches the choice; the verified work stays verified but is excluded.
    let old = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "DEC-SUPPLIER")
        .expect("decision");
    old.status = DecisionStatus::Superseded;
    let mut replacement = old.clone();
    replacement.id = DecisionId::new();
    replacement.key = Key::new("DEC-SUPPLIER-2");
    replacement.status = DecisionStatus::Decided;
    replacement.outcome = Some("B".into());
    replacement.resolved_at = None;
    replacement.supersedes = Some(old.id);
    plan.decisions.insert(replacement.id, replacement);
    plan.validate().expect("valid replacement");
    assert!(state(&plan, "SUP-A-QUOTE").is_not_selected());
    assert_eq!(
        plan.work_items[&quote].status,
        WorkStatus::Verified,
        "lifecycle is kept"
    );
    assert!(!Timeline::at(&plan, t(3)).completed().contains(&quote));
    verify(&mut plan, "SUP-A-AUDIT", t(2));
    assert_eq!(
        state(&plan, "SUP-A-AUDIT"),
        Applicability::Applicable,
        "verified unconditional work is not re-labelled stranded by a later choice"
    );
}

#[test]
fn malformed_options_conditions_and_joins_are_rejected() {
    let mut plan = fixture();
    decide(&mut plan, "A", None);
    let decision = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "DEC-SUPPLIER")
        .expect("decision");
    decision.outcome = Some("Supplier A".into());
    assert!(plan.validate().is_err(), "outcome must be an option key");

    let mut plan = fixture();
    let work = plan.find_work_by_key_mut("SUP-PKG-A").expect("package");
    work.condition.as_mut().expect("condition").option = "C".into();
    assert!(plan.validate().is_err(), "condition must name an option");

    let mut plan = fixture();
    plan.find_work_by_key_mut("SUP-PKG-A")
        .expect("package")
        .join = JoinPolicy::ActiveBranches { allow_empty: false };
    assert!(plan.validate().is_err(), "packages are not merge points");

    for options in [1, 3] {
        let mut plan = fixture();
        let decision = plan.decisions.values_mut().next().expect("decision");
        let first = decision.options[0].clone();
        decision.options.truncate(1);
        decision.options.resize(options, first);
        assert!(
            plan.validate().is_err(),
            "a choice needs two options with unique keys"
        );
    }
}

#[test]
fn plans_without_conditions_serialize_without_the_new_fields() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let text = serde_json::to_string(&plan).expect("serialize");
    for field in ["\"condition\"", "\"join\"", "\"options\""] {
        assert!(!text.contains(field), "{field} is additive");
    }
    assert!(
        plan.applicability()
            .values()
            .all(Applicability::is_applicable),
        "unconditional plans stay fully applicable"
    );
}

/// Supersede the standing supplier choice with a replacement resolved at `at`.
fn supersede(plan: &mut Plan, option: &str, at: DateTime<Utc>) {
    let standing = plan
        .decisions
        .values()
        .find(|d| d.key.0.starts_with("DEC-SUPPLIER") && d.status == DecisionStatus::Decided)
        .expect("standing choice")
        .clone();
    let mut replacement = standing.clone();
    replacement.id = DecisionId::new();
    replacement.key = Key::new(format!("DEC-SUPPLIER-{}", plan.decisions.len()));
    replacement.outcome = Some(option.into());
    replacement.resolved_at = Some(at);
    replacement.rationale = Some("revisited".into());
    replacement.supersedes = Some(standing.id);
    if let Some(old) = plan.decisions.get_mut(&standing.id) {
        old.status = DecisionStatus::Superseded;
    }
    plan.decisions.insert(replacement.id, replacement);
    plan.validate().expect("valid replacement");
}

fn skipped_at(plan: &Plan) -> Release {
    let (b, merge) = (id(plan, "SUP-B-QUAL"), id(plan, "SUP-MERGE"));
    let edge = plan
        .dependencies
        .iter()
        .find(|d| d.predecessor == b && d.successor == merge)
        .expect("branch edge");
    Timeline::at(plan, t(20)).edge(plan, edge)
}

#[test]
fn a_reaffirmed_outcome_keeps_its_first_resolution_and_a_changed_one_starts_anew() {
    let mut plan = fixture();
    decide(&mut plan, "A", Some(t(1)));
    supersede(&mut plan, "A", t(5));
    assert_eq!(
        skipped_at(&plan),
        Release::SkippedBranch {
            at: EventTime::Recorded(t(1))
        }
    );
    supersede(&mut plan, "B", t(6));
    supersede(&mut plan, "A", t(7));
    assert_eq!(
        skipped_at(&plan),
        Release::SkippedBranch {
            at: EventTime::Recorded(t(7))
        },
        "the run of A restarted when B interrupted it"
    );
    let mut legacy = fixture();
    decide(&mut legacy, "A", None);
    supersede(&mut legacy, "A", t(5));
    assert_eq!(
        skipped_at(&legacy),
        Release::SkippedBranch {
            at: EventTime::Unrecorded
        },
        "a reaffirmed legacy choice was made before times were recorded"
    );
}
