//! Explicit outline order survives exchange independently of task names and keys.
#![allow(clippy::expect_used, clippy::panic)]

use dpm_interchange::{ImportOptions, export_mspdi, import_mspdi};
use dpm_model::{Plan, SiblingOrder};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../tests/support/execution-plan.json")).expect("plan")
}

fn options() -> ImportOptions {
    ImportOptions {
        project_key: "TEST".into(),
        key_prefix: Some("NEW".into()),
        match_existing_by: None,
        keep_existing_priority: false,
    }
}

#[test]
fn reordered_siblings_and_renamed_titles_roundtrip_without_renumbering() {
    let mut plan = fixture();
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    let last = SiblingOrder::between(Some(&plan.work_items[&b].order), None).expect("position");
    let work = plan.work_items.get_mut(&a).expect("a");
    work.order = last;
    work.title = "改名后的任务".into();
    let export = export_mspdi(&plan, "TEST").expect("export");
    assert!(
        export.xml.find("Contract B").expect("B") < export.xml.find("改名后的任务").expect("A")
    );
    let imported = import_mspdi(&plan, &export.xml, &options()).expect("import");
    assert_eq!(imported.candidate, plan);
    assert!(imported.report.items.iter().all(|i| i.changes.is_empty()));
}

#[test]
fn inserting_a_source_task_between_existing_siblings_keeps_their_positions() {
    let plan = fixture();
    let export = export_mspdi(&plan, "TEST").expect("export");
    let insertion = "\n<Task><UID>99</UID><GUID>aaaaaaaa-0000-4000-8000-000000000099</GUID><Name>Inserted</Name><OutlineLevel>1</OutlineLevel><Milestone>0</Milestone><Duration>PT1H0M0S</Duration><DurationFormat>6</DurationFormat></Task>";
    let at = export.xml.find("</Task>").expect("first task") + "</Task>".len();
    let mut xml = export.xml;
    xml.insert_str(at, insertion);
    let imported = import_mspdi(&plan, &xml, &options()).expect("import");
    for (id, work) in &plan.work_items {
        assert_eq!(imported.candidate.work_items[id].order, work.order);
    }
    let new = imported
        .candidate
        .find_work_by_key("NEW-99")
        .expect("inserted");
    let a = plan.find_work_by_key("TEST-A").expect("a");
    let b = plan.find_work_by_key("TEST-B").expect("b");
    assert!(a.order < new.order && new.order < b.order);
    assert_eq!(new.execution.status, dpm_model::WorkStatus::Proposed);
}

fn claim(plan: &mut Plan, key: &str) {
    let work = plan.find_work_by_key(key).expect("work").id;
    dpm_engine::apply_command(
        plan,
        dpm_model::ActorId::agent("worker"),
        dpm_engine::Command::Claim { work },
        chrono::Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("claim");
}

#[test]
fn external_move_keeps_protected_neighbours_fixed_in_either_direction() {
    for moving in ["TEST-B", "TEST-C"] {
        let mut plan = fixture();
        claim(&mut plan, "TEST-A");
        let a = plan.find_work_by_key("TEST-A").expect("a").clone();
        let id = plan.find_work_by_key(moving).expect("moving").id;
        let mut external = plan.clone();
        external.work_items.get_mut(&id).expect("moving").order = SiblingOrder(vec![50]);
        let xml = export_mspdi(&external, "TEST").expect("export").xml;
        let imported = import_mspdi(&plan, &xml, &options()).expect("import");
        dpm_engine::propose_change(&plan, &imported.candidate)
            .expect("legal move before claimed work");
        for (id, work) in &plan.work_items {
            if work.key.0 != moving {
                assert_eq!(imported.candidate.work_items[id], *work);
            }
        }
        assert!(imported.candidate.work_items[&id].order < a.order);
        let return_xml = export_mspdi(&plan, "TEST").expect("return export").xml;
        let returned =
            import_mspdi(&imported.candidate, &return_xml, &options()).expect("return import");
        dpm_engine::propose_change(&imported.candidate, &returned.candidate)
            .expect("legal move after claimed work");
        assert_eq!(returned.candidate.work_items[&a.id], a);
    }
}

#[test]
fn unclaimed_prerequisite_is_also_a_protected_order_anchor() {
    let mut plan = fixture();
    plan.decisions.clear();
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    let c = plan.find_work_by_key("TEST-C").expect("c").id;
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|d| d.predecessor == a && d.successor == b)
        .expect("edge");
    edge.kind = dpm_model::DependencyKind::FinishFinish;
    claim(&mut plan, "TEST-B");
    assert!(dpm_engine::protected_work(&plan).contains(&a));
    let mut external = plan.clone();
    external.work_items.get_mut(&c).expect("c").order = SiblingOrder(vec![50]);
    let xml = export_mspdi(&external, "TEST").expect("export").xml;
    let imported = import_mspdi(&plan, &xml, &options()).expect("import");
    dpm_engine::propose_change(&plan, &imported.candidate)
        .expect("preserves full prerequisite basis");
    for id in [a, b] {
        assert_eq!(imported.candidate.work_items[&id], plan.work_items[&id]);
    }
}

#[test]
fn reversing_protected_anchors_is_an_explicit_conflict() {
    let mut plan = fixture();
    plan.dependencies.clear();
    plan.decisions.clear();
    claim(&mut plan, "TEST-A");
    claim(&mut plan, "TEST-B");
    let mut external = plan.clone();
    external.find_work_by_key_mut("TEST-B").expect("b").order = SiblingOrder(vec![50]);
    let xml = export_mspdi(&external, "TEST").expect("export").xml;
    assert!(matches!(
        import_mspdi(&plan, &xml, &options()),
        Err(dpm_interchange::InterchangeError::OutlineOrder { .. })
    ));
}

#[test]
fn concurrent_equal_positions_roundtrip_but_unrepresentable_insertion_is_refused() {
    let mut plan = fixture();
    plan.dependencies.clear();
    plan.decisions.clear();
    plan.find_work_by_key_mut("TEST-B").expect("b").order = SiblingOrder(vec![100]);
    claim(&mut plan, "TEST-A");
    claim(&mut plan, "TEST-B");
    let mut xml = export_mspdi(&plan, "TEST").expect("export").xml;
    assert_eq!(
        import_mspdi(&plan, &xml, &options())
            .expect("same order")
            .candidate,
        plan
    );
    let at = xml.find("</Task>").expect("first") + "</Task>".len();
    xml.insert_str(at, "<Task><UID>99</UID><GUID>ffffffff-0000-4000-8000-000000000099</GUID><Name>Inserted</Name><OutlineLevel>1</OutlineLevel><Duration>PT1H0M0S</Duration></Task>");
    assert!(matches!(
        import_mspdi(&plan, &xml, &options()),
        Err(dpm_interchange::InterchangeError::OutlineOrder { uid: 99, .. })
    ));
}
