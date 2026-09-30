//! External saves preserve identity and uncertainty without importing execution state.
#![cfg(test)]
use dpm_interchange::{ImportOptions, export_mspdi, import_mspdi};
use dpm_model::{Plan, WorkKind, WorkStatus};

fn fixture() -> Plan {
    serde_json::from_str(include_str!("../../../tests/support/execution-plan.json")).expect("plan")
}
fn options() -> ImportOptions {
    ImportOptions {
        project_key: "TEST".into(),
        key_prefix: None,
        match_existing_by: None,
        keep_existing_priority: true,
        time_zone: None,
    }
}
fn without_guids(xml: &str) -> String {
    xml.lines()
        .filter(|line| !line.trim().starts_with("<GUID>"))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn names_and_uids_are_not_identity_and_fresh_work_restores_keys_and_ranges() {
    let plan = fixture();
    let xml = export_mspdi(&plan, "TEST").expect("export").xml;
    let returned = without_guids(&xml).replace("Contract A", "外部重命名任务");
    let result = import_mspdi(&plan, &returned, &options()).expect("rename");
    assert_eq!(result.candidate.work_items.len(), plan.work_items.len());
    let a = result.candidate.find_work_by_key("TEST-A").expect("a");
    assert_eq!(a.title, "外部重命名任务");
    let mut empty = plan.clone();
    empty.work_items.clear();
    empty.dependencies.clear();
    empty.decisions.clear();
    let fresh = import_mspdi(&empty, &returned, &options())
        .expect("fresh")
        .candidate;
    for (id, original) in &plan.work_items {
        let work = &fresh.work_items[id];
        assert_eq!(work.key, original.key);
        assert_eq!(work.schedule.estimate, original.schedule.estimate);
        if work.kind == WorkKind::Task {
            assert_eq!(work.execution.status, WorkStatus::Proposed);
        }
    }
}

#[test]
fn external_duration_wins_and_wbs_deviation_is_reported() {
    let plan = fixture();
    let xml = export_mspdi(&plan, "TEST").expect("export").xml;
    let start = xml.find("<Duration>").expect("duration");
    let end = xml
        .match_indices("</Duration>")
        .map(|(at, tag)| at + tag.len())
        .find(|end| *end > start)
        .expect("end");
    let mut edited = xml.clone();
    edited.replace_range(
        start..end,
        "<Duration>PT99H0M0S</Duration><WBS>custom-code</WBS>",
    );
    let result = import_mspdi(&plan, &edited, &options()).expect("edit");
    let a = result.candidate.find_work_by_key("TEST-A").expect("a");
    let estimate = a.schedule.estimate.expect("estimate");
    assert_eq!(
        (
            estimate.optimistic_hours,
            estimate.likely_hours,
            estimate.pessimistic_hours
        ),
        (99.0, 99.0, 99.0)
    );
    assert!(
        result.report.items[0]
            .approximated
            .iter()
            .any(|f| f.field == "estimate" && f.detail.contains("external duration wins"))
    );
    assert!(
        result.report.items[0]
            .rejected
            .iter()
            .any(|f| f.field == "wbs")
    );
}

#[test]
fn malformed_or_duplicated_metadata_is_refused_without_fallback_matching() {
    let plan = fixture();
    let xml = export_mspdi(&plan, "TEST").expect("export").xml;
    for bad in [
        xml.replace("\"version\":1", "\"version\":999"),
        xml.replace("\"version\":1", "\"version\":null"),
    ] {
        assert!(import_mspdi(&plan, &bad, &options()).is_err());
    }
    let start = xml.find("<Task>").expect("task");
    let end = xml
        .match_indices("</Task>")
        .map(|(at, tag)| at + tag.len())
        .find(|end| *end > start)
        .expect("end");
    let duplicate = xml
        .get(start..end)
        .expect("task element")
        .replace("<UID>1</UID>", "<UID>99</UID>");
    assert!(
        import_mspdi(
            &plan,
            &xml.replace("</Tasks>", &format!("{duplicate}</Tasks>")),
            &options()
        )
        .is_err()
    );
}

#[test]
fn genuine_omniplan_and_mpxj_saves_keep_identity_keys_and_estimates() {
    let plan = fixture();
    assert_eq!(
        export_mspdi(&plan, "TEST").expect("export").xml,
        include_str!("mspdi/dpm-metadata-roundtrip.xml")
    );
    for (name, xml) in [
        (
            "OmniPlan",
            include_str!("mspdi/omniplan-metadata-roundtrip.xml"),
        ),
        ("MPXJ", include_str!("mspdi/mpxj-metadata-roundtrip.xml")),
    ] {
        let imported = import_mspdi(&plan, xml, &options()).expect(name);
        assert_eq!(
            imported.candidate.work_items.len(),
            plan.work_items.len(),
            "{name}"
        );
        for (id, work) in &plan.work_items {
            let returned = &imported.candidate.work_items[id];
            assert_eq!(returned.key, work.key, "{name}");
            assert_eq!(returned.schedule.estimate, work.schedule.estimate, "{name}");
            assert_eq!(returned.execution, work.execution, "{name}");
        }
        assert!(
            imported
                .report
                .items
                .iter()
                .filter(|i| i.work.is_some())
                .all(|i| i.preserved.iter().any(|f| f == "dpm_metadata"))
        );
        if name == "OmniPlan" {
            assert_eq!(
                imported
                    .candidate
                    .find_work_by_key("TEST-A")
                    .expect("renamed")
                    .title,
                "Contract A renamed in OmniPlan"
            );
        }
    }
}
