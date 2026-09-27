use crate::mspdi::tests::{document, task, workspace};
use crate::{
    ImportOptions, ImportResult, InterchangeError, ItemOutcome, LinkOutcome, import_mspdi,
};
use dpm_model::Plan;

/// A document as OmniPlan writes it: no project GUID, no task GUIDs, a UID 0 project summary.
fn anonymous(tasks: &[String]) -> String {
    format!(
        "<Project xmlns=\"http://schemas.microsoft.com/project\"><Title>Tool plan</Title><Tasks><Task><UID>0</UID><OutlineLevel>0</OutlineLevel><Summary>1</Summary></Task>{}</Tasks></Project>",
        tasks.concat()
    )
}

fn bare(uid: i64, level: u32, name: &str, body: &str) -> String {
    format!(
        "<Task><UID>{uid}</UID><Name>{name}</Name><OutlineLevel>{level}</OutlineLevel>{body}</Task>"
    )
}

fn release() -> String {
    anonymous(&[
        bare(1, 1, "Design", "<Summary>1</Summary>"),
        bare(2, 2, "Spec", "<Duration>PT8H0M0S</Duration>"),
        bare(
            3,
            1,
            "Ship",
            "<Milestone>1</Milestone><PredecessorLink><PredecessorUID>2</PredecessorUID><Type>1</Type></PredecessorLink>",
        ),
    ])
}

fn options(prefix: Option<&str>) -> ImportOptions {
    ImportOptions {
        project_key: "REL".into(),
        key_prefix: prefix.map(Into::into),
    }
}

fn import(plan: &Plan, xml: &str, prefix: &str) -> ImportResult {
    import_mspdi(plan, xml, &options(Some(prefix))).expect("import")
}

fn keys(result: &ImportResult) -> Vec<String> {
    result
        .report
        .items
        .iter()
        .filter_map(|i| i.work.as_ref().map(|w| w.key.0.clone()))
        .collect()
}

#[test]
fn guidless_documents_import_under_the_source_prefix_and_reimport_unchanged() {
    let first = import(&workspace(), &release(), "OP");
    let root = &first.report.items[0];
    assert_eq!(root.outcome, ItemOutcome::Skipped);
    assert_eq!(root.rejected[0].field, "project_summary");
    assert_eq!(keys(&first), ["OP-1", "OP-2", "OP-3"]);
    for item in &first.report.items[1..] {
        assert_eq!(item.outcome, ItemOutcome::Created);
        let identity = &item.approximated[0];
        assert_eq!(identity.field, "identity");
        assert!(
            identity.detail.contains("key prefix OP") && identity.detail.contains("REL"),
            "{identity:?}"
        );
    }
    assert_eq!(first.report.links[0].outcome, LinkOutcome::Preserved);
    assert_eq!(first.candidate.dependencies.len(), 1);
    let applied = first.candidate;
    let again = import(&applied, &release(), "OP");
    assert_eq!(again.candidate, applied);
    assert!(
        again.report.items[1..]
            .iter()
            .all(|i| i.outcome == ItemOutcome::Unchanged)
    );
    // Another prefix names another source: separate work, never a merge.
    let other = import(&applied, &release(), "OQ");
    assert_eq!(keys(&other), ["OQ-1", "OQ-2", "OQ-3"]);
    assert_eq!(
        other.candidate.work_items.len(),
        applied.work_items.len() + 3
    );
}

#[test]
fn guidless_documents_require_an_explicit_source_prefix() {
    let error = import_mspdi(&workspace(), &release(), &options(None)).expect_err("refused");
    assert!(
        matches!(
            error,
            InterchangeError::SourceScopeRequired {
                count: 3,
                first_uid: 1
            }
        ),
        "{error}"
    );
    assert_eq!(error.code(), "invalid_request");
    assert!(error.to_string().contains("key prefix"));
    // A project GUID or task GUIDs already scope the identity.
    let guided = document(&[task(1, 1, "Guided", "")]);
    assert!(import_mspdi(&workspace(), &guided, &options(None)).is_ok());
}

#[test]
fn a_guidless_source_never_takes_over_work_another_source_keyed() {
    let guided = import(&workspace(), &document(&[task(1, 1, "Guided", "")]), "OP").candidate;
    let error = import_mspdi(
        &guided,
        &anonymous(&[bare(1, 1, "Other", "")]),
        &options(Some("OP")),
    )
    .expect_err("collision");
    assert!(
        matches!(&error, InterchangeError::KeyCollision { key, uid: 1 } if key == "OP-1"),
        "{error}"
    );
}
