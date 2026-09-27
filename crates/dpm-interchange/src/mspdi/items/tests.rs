use crate::mspdi::tests::{document, import, outline_document, task, work, workspace};
use crate::{FieldChange, ImportResult, ItemOutcome};
use dpm_model::{Plan, Priority, WorkKind};

fn reimport(edit: impl Fn(String) -> String) -> (Plan, ImportResult) {
    let current = import(&workspace(), &outline_document("")).candidate;
    let result = import(&current, &edit(outline_document("")));
    (current, result)
}

fn preserved(result: &ImportResult, uid: i64) -> Vec<String> {
    result
        .report
        .items
        .iter()
        .find(|i| i.uid == uid)
        .map(|i| i.preserved.clone())
        .unwrap_or_default()
}

#[test]
fn absent_priority_keeps_the_local_priority() {
    let (current, result) = reimport(|xml| xml.replace("<Priority>700</Priority>", ""));
    assert_eq!(work(&current, "MSP-3").priority, Priority::P1);
    assert_eq!(work(&result.candidate, "MSP-3").priority, Priority::P1);
    assert_eq!(result.candidate, current);
    assert_eq!(result.report.items[2].outcome, ItemOutcome::Unchanged);
    assert!(!preserved(&result, 3).contains(&"priority".to_string()));
}

#[test]
fn empty_name_keeps_the_local_title() {
    let (_, result) = reimport(|xml| xml.replace("<Name>Spec</Name>", "<Name></Name>"));
    assert_eq!(work(&result.candidate, "MSP-2").title, "Spec");
    assert_eq!(result.report.items[1].outcome, ItemOutcome::Unchanged);
    assert!(!preserved(&result, 2).contains(&"title".to_string()));
}

#[test]
fn absent_milestone_flag_keeps_the_local_kind() {
    let (_, result) = reimport(|xml| xml.replace("<Milestone>1</Milestone>", ""));
    assert_eq!(work(&result.candidate, "MSP-4").kind, WorkKind::Milestone);
    assert_eq!(result.report.items[3].outcome, ItemOutcome::Unchanged);
}

#[test]
fn omitted_fields_are_reported_as_kept_and_new_work_reports_the_default_priority() {
    let (_, result) = reimport(|xml| {
        xml.replace("<Priority>700</Priority>", "")
            .replace("<Notes>Write it</Notes>", "")
            .replace("<Duration>PT16H0M0S</Duration>", "")
    });
    assert_eq!(result.report.items[2].kept, ["notes", "priority"]);
    assert_eq!(
        result.report.items[1].kept,
        ["notes", "priority", "duration"]
    );
    assert_eq!(result.report.items[3].kept, ["notes", "priority"]);
    assert!(result.report.items.iter().all(|i| i.changes.is_empty()));
    let created = import(&workspace(), &outline_document(""));
    let spec = &created.report.items[1];
    assert!(spec.kept.is_empty());
    assert!(
        spec.approximated
            .iter()
            .any(|f| f.field == "priority" && f.detail.contains("default 500"))
    );
}

#[test]
fn updated_items_list_exactly_the_changed_local_fields() {
    let (_, result) = reimport(|xml| {
        xml.replace(">Spec<", ">Specification<")
            .replace("<Priority>700</Priority>", "<Priority>300</Priority>")
    });
    let spec = &result.report.items[1];
    assert_eq!(spec.outcome, ItemOutcome::Updated);
    assert_eq!(
        spec.changes,
        [FieldChange {
            field: "title".into(),
            before: "Spec".into(),
            after: "Specification".into(),
        }]
    );
    let review = &result.report.items[2];
    assert_eq!(review.changes[0].field, "priority");
    assert_eq!(
        (
            review.changes[0].before.as_str(),
            review.changes[0].after.as_str()
        ),
        ("P1", "P3")
    );
}

#[test]
fn a_top_level_source_task_reports_its_kept_local_parent() {
    let mut current = import(&workspace(), &outline_document("")).candidate;
    let design = work(&current, "MSP-1").id;
    let build = work(&current, "MSP-5").id;
    current.work_items.get_mut(&build).expect("build").parent = Some(design);
    let only_build = document(&[task(
        5,
        1,
        "Build",
        "<Duration>PT4H0M0S</Duration><DurationFormat>6</DurationFormat><Priority>500</Priority>",
    )]);
    let result = import(&current, &only_build);
    let item = &result.report.items[0];
    assert_eq!(work(&result.candidate, "MSP-5").parent, Some(design));
    assert_eq!(item.kept, ["notes", "outline"]);
    assert!(!item.preserved.contains(&"outline".to_string()));
    assert_eq!(item.outcome, ItemOutcome::Unchanged);
}

/// Source field whose value a changed DPM field comes from.
fn source_field(changed: &str) -> &str {
    match changed {
        "objective" => "notes",
        "parent" => "outline",
        "estimate" => "duration",
        other => other,
    }
}

/// Every changed field comes from source data the report names, no kept field changes, and the
/// outcome agrees with the change list.
fn assert_report_matches_mutation(current: &Plan, result: &ImportResult) {
    for item in &result.report.items {
        let Some(reference) = &item.work else {
            continue;
        };
        let Some(before) = current.work_items.get(&reference.id) else {
            assert!(item.changes.is_empty(), "{item:?}");
            continue;
        };
        let after = &result.candidate.work_items[&reference.id];
        assert_eq!(before == after, item.changes.is_empty(), "{item:?}");
        assert_eq!(
            item.outcome == ItemOutcome::Unchanged,
            item.changes.is_empty(),
            "{item:?}"
        );
        for change in &item.changes {
            let field = source_field(&change.field);
            let reported = item.preserved.iter().any(|p| p == field)
                || item.approximated.iter().any(|f| f.field == field);
            assert!(reported, "{field} changed without a finding: {item:?}");
            assert!(!item.kept.iter().any(|k| k == field), "{item:?}");
        }
    }
}

#[test]
fn every_report_describes_its_mutation() {
    let edits: [fn(String) -> String; 7] = [
        |xml| xml.replace("<Priority>700</Priority>", ""),
        |xml| xml.replace("<Priority>700</Priority>", "<Priority>650</Priority>"),
        |xml| xml.replace("<Name>Spec</Name>", "<Name></Name>"),
        |xml| xml.replace("<Milestone>1</Milestone>", ""),
        |xml| xml.replace("<Notes>Write it</Notes>", "<Notes>Rewrite it</Notes>"),
        |xml| xml.replace("PT16H0M0S", "PT0H0M0S").replace("PT8H", "PT9H"),
        // Approved moves out of Design to the top level.
        |xml| {
            xml.replace(
                "Approved</Name><OutlineLevel>2",
                "Approved</Name><OutlineLevel>1",
            )
        },
    ];
    for edit in edits {
        let (current, result) = reimport(edit);
        assert_report_matches_mutation(&current, &result);
    }
    let fixtures = [
        include_str!("../../../tests/mspdi/mpxj-release-plan.xml"),
        include_str!("../../../tests/mspdi/mpxj-unsupported-features.xml"),
    ];
    for xml in fixtures {
        let current = import(&workspace(), xml).candidate;
        assert_report_matches_mutation(&current, &import(&current, xml));
        let edited = xml
            .replace("<Priority>", "<Unused>")
            .replace("</Priority>", "</Unused>");
        assert_report_matches_mutation(&current, &import(&current, &edited));
    }
}
