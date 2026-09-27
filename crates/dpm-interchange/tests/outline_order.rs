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
