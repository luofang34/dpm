use crate::mspdi::tests::{document, import, outline_document, task, work, workspace};
use crate::{
    ExistingMatch, ImportOptions, ImportResult, InterchangeError, ItemOutcome, import_mspdi,
};
use dpm_model::Plan;

/// The document as a GUID-dropping tool writes it back: no project or task GUIDs.
fn without_guids(xml: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    while let Some(start) = rest.find("<GUID>") {
        out.push_str(&rest[..start]);
        let end = rest[start..].find("</GUID>").expect("closed") + start + "</GUID>".len();
        rest = &rest[end..];
    }
    out + rest
}

fn options(matching: Option<ExistingMatch>) -> ImportOptions {
    ImportOptions {
        project_key: "REL".into(),
        key_prefix: Some("OP".into()),
        match_existing_by: matching,
        keep_existing_priority: false,
    }
}

fn matched(plan: &Plan, xml: &str) -> Result<ImportResult, InterchangeError> {
    import_mspdi(plan, xml, &options(Some(ExistingMatch::TitlePath)))
}

#[test]
fn a_guidless_round_trip_maps_back_onto_the_exported_work_only_when_asked() {
    let plan = import(&workspace(), &outline_document("")).candidate;
    let returned = without_guids(&outline_document(""));
    let result = matched(&plan, &returned).expect("import");
    assert_eq!(result.candidate, plan);
    for item in &result.report.items {
        assert_eq!(item.outcome, ItemOutcome::Unchanged, "{item:?}");
        let identity = &item.approximated[0];
        assert!(
            identity.detail.contains("matched existing work MSP-"),
            "{identity:?}"
        );
    }
    assert!(
        result.report.items[1].approximated[0]
            .detail
            .contains("\"Design / Spec\"")
    );
    // Without the opt-in, the same file is another source: new work, nothing merged.
    let separate = import_mspdi(&plan, &returned, &options(None)).expect("import");
    assert!(
        separate
            .report
            .items
            .iter()
            .all(|i| i.outcome == ItemOutcome::Created)
    );
    assert_eq!(
        separate.candidate.work_items.len(),
        2 * plan.work_items.len()
    );
    assert!(work(&separate.candidate, "OP-2").id != work(&plan, "MSP-2").id);
}

#[test]
fn unmatched_tasks_keep_their_derived_identity_and_say_why() {
    let plan = import(&workspace(), &outline_document("")).candidate;
    let returned = without_guids(&outline_document("")).replace(">Spec<", ">Draft<");
    let result = matched(&plan, &returned).expect("import");
    let draft = &result.report.items[1];
    assert_eq!(draft.outcome, ItemOutcome::Created);
    assert_eq!(draft.work.as_ref().expect("work").key.0, "OP-2");
    assert!(
        draft.approximated[0]
            .detail
            .contains("no existing work has the title path \"Design / Draft\""),
        "{draft:?}"
    );
    // Spec is not in the source, so it stays; Review still matches.
    assert_eq!(result.report.retained[0].0, "MSP-2");
    assert_eq!(result.report.items[2].outcome, ItemOutcome::Unchanged);
}

#[test]
fn ambiguous_title_paths_refuse_the_whole_import() {
    let twins = document(&[
        task(1, 1, "Design", "<Summary>1</Summary>"),
        task(2, 2, "Spec", ""),
        task(3, 2, "Spec", ""),
    ]);
    let plan = import(&workspace(), &twins).candidate;
    let error = matched(&plan, &without_guids(&twins)).expect_err("ambiguous");
    let InterchangeError::AmbiguousMatch { ambiguities } = &error else {
        panic!("{error}");
    };
    assert_eq!(
        ambiguities,
        &["tasks UID 2, 3 share title path \"Design / Spec\" with existing work MSP-2, MSP-3"]
    );
    assert_eq!(error.code(), "invalid_command");
    let single = import(&workspace(), &outline_document("")).candidate;
    let error = matched(&single, &without_guids(&twins)).expect_err("ambiguous");
    assert!(error.to_string().contains("tasks UID 2, 3"), "{error}");
    let local_twins = document(&[
        task(1, 1, "Design", "<Summary>1</Summary>"),
        task(2, 2, "Spec", ""),
    ]);
    let error = matched(&plan, &without_guids(&local_twins)).expect_err("ambiguous");
    assert!(
        error.to_string().contains(
            "task UID 2 has title path \"Design / Spec\", shared by existing work MSP-2, MSP-3"
        ),
        "{error}"
    );
}

#[test]
fn a_match_never_overrides_work_this_source_already_created() {
    let plan = import(&workspace(), &document(&[task(9, 1, "Other", "")])).candidate;
    let bare = |name: &str| {
        format!(
            "<Project xmlns=\"http://schemas.microsoft.com/project\"><Tasks><Task><UID>1</UID><Name>{name}</Name><OutlineLevel>1</OutlineLevel></Task></Tasks></Project>"
        )
    };
    let plan = import_mspdi(&plan, &bare("First"), &options(None))
        .expect("import")
        .candidate;
    assert_eq!(work(&plan, "OP-1").title, "First");
    // UID 1 is renamed to the title of other local work: its derived identity still names OP-1.
    let error = matched(&plan, &bare("Other")).expect_err("conflict");
    assert!(
        error
            .to_string()
            .contains("task UID 1 already maps to OP-1 through its derived identity, but its title path \"Other\" matches MSP-9"),
        "{error}"
    );
    // Renamed to a title nobody else has, the source keeps updating its own work.
    let renamed = matched(&plan, &bare("Renamed")).expect("import");
    assert_eq!(work(&renamed.candidate, "OP-1").title, "Renamed");
}
