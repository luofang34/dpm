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
                .objective
                .clear()
        },
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
                .acceptance
                .clear()
        },
        |p| p.find_work_by_key_mut("TEST-A").expect("task").acceptance[0].text = " ".into(),
        |p| p.find_work_by_key_mut("TEST-A").expect("task").owner = Some(ActorId::agent("owner")),
        |p| p.find_work_by_key_mut("TEST-A").expect("task").status = WorkStatus::Claimed,
        |p| {
            p.find_work_by_key_mut("TEST-A")
                .expect("task")
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
        plan.find_work_by_key_mut("TEST-A").expect("task").estimate = Some(estimate);
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
        format!("/work_items/{work}/acceptance/0"),
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
