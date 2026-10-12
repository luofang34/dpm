use super::*;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

/// Before anything starts the remaining graph is the baseline, so both report the same drag, and
/// every activity's drag is between zero and its own duration on this finish-to-start plan.
#[test]
fn remaining_drag_of_an_unstarted_plan_equals_the_baseline() {
    let plan = fixture();
    let all: BTreeSet<WorkItemId> = plan.work_items.keys().copied().collect();
    let baseline = drag(&plan, &all).expect("drag");
    let timeline = dpm_model::Timeline::at(&plan, chrono::DateTime::UNIX_EPOCH);
    let remaining = drag_remaining_at(&plan, &timeline, &all).expect("drag");
    assert_eq!(baseline, remaining);
    let schedule = crate::deterministic(&plan).expect("schedule");
    for (id, hours) in &baseline {
        let work = &plan.work_items[id];
        assert!(*hours >= 0.0 && *hours <= work.expected_duration_hours() + EPSILON);
        if !schedule.activities[id].critical {
            assert_eq!(*hours, 0.0, "{} floats, so it has no drag", work.key);
        }
    }
    assert!(baseline.values().any(|hours| *hours > 0.0));
}

#[test]
fn unknown_work_is_absent() {
    let plan = fixture();
    let unknown = BTreeSet::from([WorkItemId::new()]);
    assert!(drag(&plan, &unknown).expect("drag").is_empty());
}
