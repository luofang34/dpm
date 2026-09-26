use crate::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};

fn fixture() -> Plan {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../../tests/support/conditional-plan.json"
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

/// Adds work cloned from a fixture item of the same kind, with no condition unless `option`
/// names a supplier option, and no dependencies.
fn add(plan: &mut Plan, template: &str, key: &str, parent: Option<&str>, option: Option<&str>) {
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let mut work = plan.find_work_by_key(template).expect("template").clone();
    work.id = WorkItemId::new();
    work.key = Key::new(key);
    work.parent = parent.map(|p| id(plan, p));
    work.condition = option.map(|option| WorkCondition {
        decision,
        option: option.into(),
    });
    plan.work_items.insert(work.id, work);
    plan.validate().expect("valid addition");
}

fn package(plan: &mut Plan, key: &str, parent: Option<&str>) {
    add(plan, "SUP-PKG-A", key, parent, None);
}

fn task(plan: &mut Plan, key: &str, parent: &str, option: &str) {
    add(plan, "SUP-A-QUOTE", key, Some(parent), Some(option));
}

fn decide(plan: &mut Plan, option: &str, at: DateTime<Utc>) {
    let decision = plan
        .decisions
        .values_mut()
        .find(|d| d.key.0 == "DEC-SUPPLIER")
        .expect("decision");
    decision.status = DecisionStatus::Decided;
    decision.outcome = Some(option.into());
    decision.resolved_at = Some(at);
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

fn excluded_by_supplier() -> Applicability {
    Applicability::AllChildrenExcluded {
        decisions: vec![Key::new("DEC-SUPPLIER")],
    }
}

#[test]
fn an_unconditional_package_whose_only_children_were_not_selected_is_excluded() {
    let mut plan = fixture();
    package(&mut plan, "X-PKG", None);
    task(&mut plan, "X-A1", "X-PKG", "A");
    decide(&mut plan, "B", t(1));
    assert_eq!(state(&plan, "X-PKG"), excluded_by_supplier());
    let timeline = Timeline::at(&plan, t(9));
    assert_eq!(timeline.completed_at(id(&plan, "X-PKG")), None);
    assert!(timeline.applicability(id(&plan, "X-PKG")).is_not_selected());
}

#[test]
fn a_nested_excluded_package_is_a_skipped_branch_of_a_container_with_selected_work() {
    let mut plan = fixture();
    package(&mut plan, "OUTER", None);
    package(&mut plan, "INNER", Some("OUTER"));
    task(&mut plan, "X-A1", "INNER", "A");
    task(&mut plan, "X-B1", "OUTER", "B");
    package(&mut plan, "ONLY-EXCLUDED", None);
    package(&mut plan, "DEEP", Some("ONLY-EXCLUDED"));
    task(&mut plan, "X-A2", "DEEP", "A");
    decide(&mut plan, "B", t(1));
    assert_eq!(state(&plan, "INNER"), excluded_by_supplier());
    assert_eq!(state(&plan, "DEEP"), excluded_by_supplier());
    assert_eq!(state(&plan, "ONLY-EXCLUDED"), excluded_by_supplier());
    assert_eq!(state(&plan, "OUTER"), Applicability::Applicable);

    let outer = id(&plan, "OUTER");
    assert_eq!(Timeline::at(&plan, t(2)).completed_at(outer), None);
    verify(&mut plan, "X-B1", t(3));
    assert_eq!(
        Timeline::at(&plan, t(4)).completed_at(outer),
        Some(EventTime::Recorded(t(3))),
        "the selected child completes the container; the excluded package is skipped at the choice"
    );
}

#[test]
fn an_undecided_choice_leaves_a_package_of_conditional_children_awaiting_it() {
    let mut plan = fixture();
    package(&mut plan, "X-PKG", None);
    task(&mut plan, "X-A1", "X-PKG", "A");
    task(&mut plan, "X-B1", "X-PKG", "B");
    assert_eq!(
        state(&plan, "X-PKG"),
        Applicability::AwaitingChoice {
            predecessor: Key::new("X-A1")
        }
    );
    assert_eq!(
        Timeline::at(&plan, t(9)).completed_at(id(&plan, "X-PKG")),
        None
    );

    decide(&mut plan, "A", t(1));
    assert_eq!(state(&plan, "X-PKG"), Applicability::Applicable);
    verify(&mut plan, "X-A1", t(2));
    assert_eq!(
        Timeline::at(&plan, t(3)).completed_at(id(&plan, "X-PKG")),
        Some(EventTime::Recorded(t(2)))
    );
}

#[test]
fn a_package_left_only_with_stranded_children_reports_that_it_cannot_complete() {
    let mut plan = fixture();
    package(&mut plan, "S-PKG", None);
    let container = id(&plan, "S-PKG");
    plan.find_work_by_key_mut("SUP-A-AUDIT")
        .expect("audit")
        .parent = Some(container);
    task(&mut plan, "X-A1", "S-PKG", "A");
    plan.validate().expect("valid move");
    decide(&mut plan, "B", t(1));
    assert!(matches!(
        state(&plan, "SUP-A-AUDIT"),
        Applicability::Stranded { .. }
    ));
    assert_eq!(
        state(&plan, "S-PKG"),
        Applicability::ChildrenStranded {
            child: Key::new("SUP-A-AUDIT")
        }
    );
}
