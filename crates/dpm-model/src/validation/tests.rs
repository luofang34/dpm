use crate::*;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

#[test]
fn fixture_and_empty_workspace_are_valid() {
    fixture().validate().expect("valid fixture");
    Plan::empty("empty")
        .validate()
        .expect("valid empty workspace");
}

#[test]
fn decision_context_defaults_preserve_existing_serialized_gates() {
    let source: serde_json::Value = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("source");
    let plan: Plan = serde_json::from_value(source.clone()).expect("compatible plan");
    for (id, decision) in &plan.decisions {
        assert!(decision.related_work.is_empty() && decision.artifact_ids.is_empty());
        assert!(decision.rationale.is_none());
        assert_eq!(
            serde_json::to_value(decision).expect("serialize"),
            source["decisions"][id.to_string()]
        );
    }
}

#[test]
fn decision_context_rejects_dangling_references_and_empty_reasons() {
    let cases: Vec<fn(&mut Decision)> = vec![
        |d| {
            d.related_work.insert(WorkItemId::new());
        },
        |d| {
            d.artifact_ids.insert(ArtifactId::new());
        },
        |d| {
            d.rationale = Some("  ".into());
        },
    ];
    for corrupt in cases {
        let mut plan = fixture();
        corrupt(plan.decisions.values_mut().next().expect("decision"));
        assert!(plan.validate().is_err());
    }
}

#[test]
fn malformed_graphs_and_contracts_are_rejected() {
    let cases: Vec<fn(&mut Plan)> = vec![
        |p| p.workspace.name.clear(),
        |p| p.work_items.values_mut().next().expect("work").id = WorkItemId::new(),
        |p| p.work_items.values_mut().next().expect("work").project = ProjectId::new(),
        |p| {
            p.work_items
                .values_mut()
                .for_each(|w| w.key = Key::new("duplicate"))
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .contract
                .objective
                .clear()
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .contract
                .acceptance
                .clear()
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .contract
                .acceptance[0]
                .text = " ".into()
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .execution
                .owner = Some(ActorId::agent("owner"))
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .execution
                .status = WorkStatus::Claimed
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .execution
                .artifact_ids
                .insert(ArtifactId::new());
        },
        |p| {
            p.decisions
                .values_mut()
                .next()
                .expect("gate")
                .blocks
                .insert(WorkItemId::new());
        },
        |p| p.dependencies[0].predecessor = WorkItemId::new(),
        |p| p.dependencies[0].lag_hours = f64::NAN,
        |p| p.dependencies[0].successor = p.dependencies[0].predecessor,
        |p| {
            let project = p.projects.values_mut().next().expect("project");
            project.parent = Some(project.id);
        },
        |p| {
            let task = p.find_work_by_key_mut("TEST-A").expect("task");
            task.parent = Some(task.id);
        },
    ];
    for (index, mutate) in cases.into_iter().enumerate() {
        let mut plan = fixture();
        mutate(&mut plan);
        assert!(plan.validate().is_err(), "invalid case {index} accepted");
    }
}

#[test]
fn estimates_reject_nonfinite_negative_and_unordered_bounds() {
    for estimate in [
        ThreePointEstimate {
            optimistic_hours: f64::NAN,
            likely_hours: 1.0,
            pessimistic_hours: 2.0,
        },
        ThreePointEstimate {
            optimistic_hours: -1.0,
            likely_hours: 1.0,
            pessimistic_hours: 2.0,
        },
        ThreePointEstimate {
            optimistic_hours: 2.0,
            likely_hours: 1.0,
            pessimistic_hours: 3.0,
        },
        ThreePointEstimate {
            optimistic_hours: 1.0,
            likely_hours: 2.0,
            pessimistic_hours: f64::INFINITY,
        },
    ] {
        let mut plan = fixture();
        plan.find_work_by_key_mut("TEST-A")
            .expect("task")
            .schedule
            .estimate = Some(estimate);
        assert!(matches!(
            plan.validate(),
            Err(ValidationError::Estimate { .. })
        ));
    }
}

#[test]
fn large_finite_estimates_do_not_overflow_the_weighted_mean() {
    let estimate = ThreePointEstimate {
        optimistic_hours: f64::MAX,
        likely_hours: f64::MAX,
        pessimistic_hours: f64::MAX,
    };
    estimate.validate().expect("finite ordered bounds");
    assert!(estimate.pert_expected_hours().is_finite());
}

#[test]
fn unknown_plan_fields_are_rejected_instead_of_dropping_proposed_edits() {
    let source = serde_json::to_value(fixture()).expect("serialize");
    let work = fixture().find_work_by_key("TEST-A").expect("task").id;
    for path in [
        String::new(),
        "/workspace".into(),
        format!("/work_items/{work}"),
        format!("/work_items/{work}/contract"),
        format!("/work_items/{work}/execution"),
        format!("/work_items/{work}/schedule"),
        format!("/work_items/{work}/contract/acceptance/0"),
    ] {
        let mut value = source.clone();
        value
            .pointer_mut(&path)
            .expect("object")
            .as_object_mut()
            .expect("map")
            .insert("unrecognized_edit".into(), serde_json::json!(true));
        let error =
            serde_json::from_value::<Plan>(value).expect_err("unknown edit must not disappear");
        assert!(error.to_string().contains("unknown field"));
    }
}

#[test]
fn decision_replacement_links_must_reach_a_superseded_decision_without_loops() {
    let mut plan = fixture();
    let old = plan.decisions.values().next().expect("decision").clone();
    let mut replacement = old.clone();
    replacement.id = DecisionId::new();
    replacement.key = Key::new("TEST-GATE-2");
    replacement.status = DecisionStatus::Decided;
    replacement.outcome = Some("Use the revised input".into());
    replacement.blocks.clear();
    replacement.supersedes = Some(old.id);
    plan.decisions.insert(replacement.id, replacement.clone());
    assert!(plan.validate().is_err(), "target is still Open");
    plan.decisions.get_mut(&old.id).expect("old").status = DecisionStatus::Superseded;
    plan.validate().expect("linked replacement");
    let mut duplicate = replacement.clone();
    duplicate.id = DecisionId::new();
    duplicate.key = Key::new("TEST-GATE-3");
    plan.decisions.insert(duplicate.id, duplicate.clone());
    assert!(plan.validate().is_err(), "one replacement per decision");
    plan.decisions.remove(&duplicate.id);
    let (first, second) = (old.id, replacement.id);
    let edited = plan.decisions.get_mut(&second).expect("new");
    edited.supersedes = Some(DecisionId::new());
    assert!(plan.validate().is_err(), "dangling replacement");
    let edited = plan.decisions.get_mut(&second).expect("new");
    edited.supersedes = Some(second);
    assert!(plan.validate().is_err(), "self replacement");
    let edited = plan.decisions.get_mut(&second).expect("new");
    edited.status = DecisionStatus::Superseded;
    edited.supersedes = Some(first);
    plan.decisions.get_mut(&first).expect("old").supersedes = Some(second);
    assert!(plan.validate().is_err(), "replacement cycle");
}

#[test]
fn an_unrecorded_start_marker_needs_started_work_without_a_start_time() {
    let mut plan = fixture();
    let id = plan.find_work_by_key("TEST-A").expect("task").id;
    let work = plan.work_items.get_mut(&id).expect("task");
    work.execution.owner = Some(ActorId::agent("legacy"));
    work.execution.status = WorkStatus::Blocked;
    work.execution.block_reason = Some("vendor".into());
    work.execution.events.start_unrecorded = true;
    plan.validate().expect("a blocked legacy start is valid");
    let serialized = serde_json::to_value(plan.work_items[&id].execution.events).expect("events");
    assert_eq!(serialized, serde_json::json!({"start_unrecorded": true}));
    let mut timed = plan.clone();
    timed
        .work_items
        .get_mut(&id)
        .expect("task")
        .execution
        .events
        .started_at = Some(chrono::Utc::now());
    assert!(
        timed.validate().is_err(),
        "a recorded start is not unrecorded"
    );
    let work = plan.work_items.get_mut(&id).expect("task");
    work.execution.status = WorkStatus::Claimed;
    work.execution.block_reason = None;
    assert!(plan.validate().is_err(), "claimed work has not started");
    assert_eq!(
        serde_json::to_value(ExecutionEvents::default()).expect("events"),
        serde_json::json!({}),
        "the marker is additive and absent by default"
    );
}

#[test]
fn handoffs_name_two_owners_a_reason_and_keep_time_order() {
    let base = fixture();
    let task = base.find_work_by_key("TEST-A").expect("task").id;
    let milestone = base.find_work_by_key("TEST-M1").expect("milestone").id;
    let at = chrono::DateTime::<chrono::Utc>::UNIX_EPOCH;
    let handoff = |from: &str, to: &str, reason: &str, hours: i64| Handoff {
        from: ActorId::agent(from),
        to: ActorId::agent(to),
        actor: ActorId::human("lead"),
        at: at + chrono::TimeDelta::hours(hours),
        reason: reason.into(),
    };
    let mut valid = base.clone();
    let item = valid.work_items.get_mut(&task).expect("task");
    item.execution.handoffs = vec![handoff("a", "b", "moved", 1), handoff("b", "a", "back", 1)];
    valid.validate().expect("ordered handoffs");
    let invalid = [
        (task, vec![handoff("a", "a", "same", 1)]),
        (task, vec![handoff("a", "b", " ", 1)]),
        (
            task,
            vec![handoff("a", "b", "x", 2), handoff("b", "c", "y", 1)],
        ),
        (milestone, vec![handoff("a", "b", "not a task", 1)]),
    ];
    for (work, handoffs) in invalid {
        let mut plan = base.clone();
        plan.work_items
            .get_mut(&work)
            .expect("work")
            .execution
            .handoffs = handoffs;
        assert!(plan.validate().is_err());
    }
}

#[test]
fn actors_parse_from_the_adapter_spelling_and_round_trip() {
    for spelling in ["human:ann", "agent:coder", "service:ci"] {
        let actor: ActorId = spelling.parse().expect("actor");
        assert_eq!(actor.to_string(), spelling);
    }
    assert_eq!(
        "coder".parse::<ActorId>(),
        Err(ActorParseError::MissingKind("coder".into()))
    );
    assert_eq!(
        "robot:x".parse::<ActorId>(),
        Err(ActorParseError::UnknownKind("robot".into()))
    );
    assert_eq!(
        "agent: ".parse::<ActorId>(),
        Err(ActorParseError::EmptyName)
    );
}
