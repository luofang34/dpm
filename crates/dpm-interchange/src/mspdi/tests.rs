use super::*;
use dpm_model::{DependencyKind, Key, Project, WorkKind, WorkStatus};

pub(crate) const PROJECT_GUID: &str = "11111111-1111-4111-8111-111111111111";

pub(crate) fn workspace() -> Plan {
    let mut plan = Plan::empty("Interchange");
    let project = Project {
        id: ProjectId::new(),
        key: Key::new("REL"),
        parent: None,
        title: "Release".into(),
        objective: "Ship the release".into(),
    };
    plan.projects.insert(project.id, project);
    plan
}

pub(crate) fn guid(uid: i64) -> String {
    format!("22222222-2222-4222-8222-{uid:012}")
}

pub(crate) fn task(uid: i64, level: u32, name: &str, body: &str) -> String {
    format!(
        "<Task><UID>{uid}</UID><GUID>{}</GUID><Name>{name}</Name><OutlineLevel>{level}</OutlineLevel>{body}</Task>",
        guid(uid)
    )
}

pub(crate) fn link(predecessor: i64, kind: u32, lag: i64, format: u32) -> String {
    format!(
        "<PredecessorLink><PredecessorUID>{predecessor}</PredecessorUID><Type>{kind}</Type><LinkLag>{lag}</LinkLag><LagFormat>{format}</LagFormat></PredecessorLink>"
    )
}

pub(crate) fn document(tasks: &[String]) -> String {
    format!(
        "<?xml version=\"1.0\"?><Project xmlns=\"http://schemas.microsoft.com/project\"><Name>Source</Name><GUID>{PROJECT_GUID}</GUID><Tasks>{}</Tasks></Project>",
        tasks.concat()
    )
}

pub(crate) fn options() -> ImportOptions {
    ImportOptions {
        project_key: "REL".into(),
        key_prefix: Some("MSP".into()),
        match_existing_by: None,
        keep_existing_priority: false,
    }
}

pub(crate) fn import(plan: &Plan, xml: &str) -> ImportResult {
    import_mspdi(plan, xml, &options()).expect("import")
}

pub(crate) fn work<'a>(plan: &'a Plan, key: &str) -> &'a dpm_model::WorkItem {
    plan.find_work_by_key(key).expect("work")
}

/// A summary with two tasks and a milestone, followed by a top-level task.
pub(crate) fn outline_document(extra_links: &str) -> String {
    document(&[
        task(1, 1, "Design", "<Summary>1</Summary>"),
        task(
            2,
            2,
            "Spec",
            "<Duration>PT16H0M0S</Duration><DurationFormat>5</DurationFormat><Notes>Write it</Notes>",
        ),
        task(
            3,
            2,
            "Review",
            &format!(
                "<Duration>PT8H0M0S</Duration><DurationFormat>6</DurationFormat><Priority>700</Priority>{}",
                link(2, 1, 0, 7)
            ),
        ),
        task(
            4,
            2,
            "Approved",
            &format!(
                "<Milestone>1</Milestone><Duration>PT0H0M0S</Duration>{}",
                link(3, 1, 1200, 6)
            ),
        ),
        task(
            5,
            1,
            "Build",
            &format!(
                "<Duration>PT4H0M0S</Duration><DurationFormat>6</DurationFormat>{extra_links}"
            ),
        ),
    ])
}

#[test]
fn outline_maps_to_packages_tasks_and_milestones_without_execution() {
    let plan = workspace();
    let result = import(&plan, &outline_document(""));
    let candidate = &result.candidate;
    let design = work(candidate, "MSP-1");
    assert_eq!(design.kind, WorkKind::WorkPackage);
    assert_eq!(design.status, WorkStatus::Planned);
    let spec = work(candidate, "MSP-2");
    assert_eq!(
        (spec.kind, spec.status, spec.parent),
        (WorkKind::Task, WorkStatus::Proposed, Some(design.id))
    );
    assert!(spec.acceptance.is_empty() && spec.owner.is_none());
    assert_eq!(spec.objective, "Write it");
    assert_eq!(spec.estimate.map(|e| e.likely_hours), Some(16.0));
    assert_eq!(spec.id.0.to_string(), guid(2));
    assert_eq!(work(candidate, "MSP-3").priority, dpm_model::Priority::P1);
    assert_eq!(work(candidate, "MSP-4").kind, WorkKind::Milestone);
    assert_eq!(work(candidate, "MSP-5").parent, None);
    let review = &result.report.items[2];
    assert_eq!(review.outcome, ItemOutcome::Created);
    assert!(review.preserved.contains(&"priority".to_string()));
    assert!(
        review
            .approximated
            .iter()
            .any(|f| f.field == "duration" && f.detail.contains("elapsed"))
    );
    assert!(
        result.report.items[1]
            .approximated
            .iter()
            .any(|f| f.field == "duration" && f.detail.contains("working-time"))
    );
    let edges: Vec<_> = candidate
        .dependencies
        .iter()
        .map(|d| (d.kind, d.lag_hours))
        .collect();
    assert_eq!(
        edges,
        [
            (DependencyKind::FinishStart, 0.0),
            (DependencyKind::FinishStart, 2.0)
        ]
    );
    assert!(dpm_engine::propose_change(&plan, candidate).is_ok());
}

#[test]
fn reimporting_the_same_document_changes_nothing() {
    let plan = workspace();
    let first = import(&plan, &outline_document(&link(1, 1, 0, 7))).candidate;
    let again = import(&first, &outline_document(&link(1, 1, 0, 7)));
    assert_eq!(again.candidate, first);
    assert!(
        again
            .report
            .items
            .iter()
            .all(|i| i.outcome == ItemOutcome::Unchanged)
    );
    let preview = dpm_engine::propose_change(&first, &again.candidate).expect("preview");
    assert!(preview.changes.is_empty());
}

#[test]
fn reimport_updates_the_same_work_and_keeps_local_additions() {
    let plan = workspace();
    let mut current = import(&plan, &outline_document("")).candidate;
    let spec = work(&current, "MSP-2").id;
    let build = work(&current, "MSP-5").id;
    let mut local = work(&current, "MSP-5").clone();
    local.id = dpm_model::WorkItemId::new();
    local.key = Key::new("LOCAL-1");
    current.work_items.insert(local.id, local.clone());
    current.dependencies.push(dpm_model::Dependency::new(
        build,
        local.id,
        DependencyKind::FinishStart,
        1.0,
    ));
    let changed = outline_document("")
        .replace(">Spec<", ">Specification<")
        .replace(&link(3, 1, 1200, 6), "");
    let result = import(&current, &changed);
    assert_eq!(result.candidate.work_items[&spec].title, "Specification");
    assert_eq!(result.candidate.work_items.len(), current.work_items.len());
    assert_eq!(result.report.retained, [Key::new("LOCAL-1")]);
    assert_eq!(result.report.items[1].outcome, ItemOutcome::Updated);
    // The dropped source link disappears; the local edge from imported work stays.
    assert_eq!(result.candidate.dependencies.len(), 2);
    assert!(
        result
            .candidate
            .dependencies
            .iter()
            .any(|d| d.successor == local.id)
    );
}

#[test]
fn unchanged_source_values_keep_richer_local_estimates_and_edges() {
    let plan = workspace();
    let mut current = import(&plan, &outline_document("")).candidate;
    let spec = work(&current, "MSP-2").id;
    // 12/15/24 h has a PERT expectation of 16 h, the value the source still carries.
    current.work_items.get_mut(&spec).expect("spec").estimate =
        Some(dpm_model::ThreePointEstimate {
            optimistic_hours: 12.0,
            likely_hours: 15.0,
            pessimistic_hours: 24.0,
        });
    let edge = current
        .dependencies
        .iter_mut()
        .find(|d| d.lag_hours == 2.0)
        .expect("edge");
    edge.policy = dpm_model::DependencyPolicy::Soft;
    edge.rationale = Some("review can overlap".into());
    let result = import(&current, &outline_document(""));
    assert_eq!(result.candidate, current);
    assert!(
        result.report.items[1]
            .preserved
            .contains(&"duration".to_string())
    );
}

#[test]
fn summary_links_expand_only_where_the_bound_is_exact() {
    let plan = workspace();
    // FS from a summary bounds its latest child finish: exact for every descendant.
    let result = import(&plan, &outline_document(&link(1, 1, 60, 6)));
    let report = &result.report.links[2];
    assert_eq!(report.outcome, LinkOutcome::Preserved);
    assert_eq!(report.dependencies.len(), 3);
    assert!(report.notes[0].contains("expanded"));
    // SS from a summary bounds its earliest child start: no exact anchors.
    let result = import(&plan, &outline_document(&link(1, 3, 0, 7)));
    assert_eq!(result.report.links[2].outcome, LinkOutcome::Rejected);
    assert!(result.report.links[2].dependencies.is_empty());
    // A link from a summary into its own child would be a cycle.
    let inner = document(&[
        task(1, 1, "Design", ""),
        task(2, 2, "Spec", &link(1, 1, 0, 7)),
    ]);
    let result = import(&plan, &inner);
    assert_eq!(result.report.links[0].outcome, LinkOutcome::Rejected);
}

#[test]
fn lag_units_are_converted_or_reported_never_guessed() {
    let plan = workspace();
    let result = import(
        &plan,
        &outline_document(&[link(2, 1, 4800, 7), link(3, 0, 50, 19), link(4, 3, 30, 3)].concat()),
    );
    let links = &result.report.links[2..];
    assert_eq!(links[0].outcome, LinkOutcome::Approximated);
    assert!(links[0].notes[0].contains("8 h"));
    assert_eq!(links[1].outcome, LinkOutcome::Rejected);
    assert!(links[1].notes[0].contains("lag format 19"));
    let no_format = document(&[
        task(1, 1, "A", ""),
        task(
            2,
            1,
            "B",
            "<PredecessorLink><PredecessorUID>1</PredecessorUID><Type>1</Type><LinkLag>600</LinkLag></PredecessorLink>",
        ),
    ]);
    // OmniPlan writes lags this way; the schema fixes the unit (tenths of a minute) and the
    // missing format means MS Project's default, working time.
    let unformatted = &import(&plan, &no_format).report.links[0];
    assert_eq!(
        unformatted.outcome,
        LinkOutcome::Approximated,
        "{unformatted:?}"
    );
    assert!(
        unformatted.notes[0].contains("no LagFormat") && unformatted.notes[0].contains("1 h"),
        "{unformatted:?}"
    );
}

#[test]
fn source_progress_is_reported_and_never_completes_work() {
    let plan = workspace();
    let xml = document(&[task(
        1,
        1,
        "Done elsewhere",
        "<PercentComplete>100</PercentComplete><ActualFinish>2026-01-05T12:00:00</ActualFinish><ConstraintType>4</ConstraintType>",
    )]);
    let result = import(&plan, &xml);
    let item = &result.report.items[0];
    assert_eq!(
        item.work.as_ref().map(|w| w.status),
        Some(WorkStatus::Proposed)
    );
    let fields: Vec<_> = item.rejected.iter().map(|f| f.field.as_str()).collect();
    assert_eq!(fields, ["constraint", "PercentComplete", "actuals"]);
}

#[test]
fn identity_falls_back_to_project_guid_and_uid() {
    let plan = workspace();
    let bare = "<Task><UID>7</UID><Name>No GUID</Name><OutlineLevel>1</OutlineLevel></Task>";
    let result = import(&plan, &document(&[bare.into()]));
    let id = result.report.items[0].work.as_ref().expect("work").id;
    assert_eq!(
        id,
        super::encoding::derived_work_id(PROJECT_GUID.parse().expect("guid"), 7)
    );
    let anonymous = format!(
        "<Project xmlns=\"http://schemas.microsoft.com/project\"><Tasks>{bare}</Tasks></Project>"
    );
    let result = import(&plan, &anonymous);
    let project = plan.projects.values().next().expect("project").id;
    assert_eq!(
        result.report.items[0].work.as_ref().expect("work").id,
        super::encoding::scoped_work_id(project, "MSP", 7)
    );
}

#[test]
fn work_in_another_project_is_never_moved() {
    let mut plan = workspace();
    let other = Project {
        id: ProjectId::new(),
        key: Key::new("OTHER"),
        parent: None,
        title: "Other".into(),
        objective: "Other work".into(),
    };
    plan.projects.insert(other.id, other);
    let xml = document(&[task(1, 1, "Shared", "")]);
    let elsewhere = import_mspdi(
        &plan,
        &xml,
        &ImportOptions {
            project_key: "OTHER".into(),
            key_prefix: None,
            match_existing_by: None,
            keep_existing_priority: false,
        },
    )
    .expect("import")
    .candidate;
    assert!(elsewhere.find_work_by_key("OTHER-1").is_some());
    let result = import(&elsewhere, &xml);
    assert_eq!(result.report.items[0].outcome, ItemOutcome::Skipped);
    assert_eq!(result.candidate, elsewhere);
}

#[test]
fn malformed_documents_fail_with_context() {
    let plan = workspace();
    let cases = [
        ("not xml", "invalid_request"),
        ("<Project/>", "invalid_request"),
        (
            &document(&[task(1, 2, "Orphan", "")]) as &str,
            "invalid_request",
        ),
        (
            &document(&[task(1, 1, "A", ""), task(1, 1, "B", "")]),
            "invalid_request",
        ),
    ];
    for (xml, code) in cases {
        let error = import_mspdi(&plan, xml, &options()).expect_err(xml);
        assert_eq!(error.code(), code, "{error}");
    }
    let unknown = ImportOptions {
        project_key: "NOPE".into(),
        key_prefix: None,
        match_existing_by: None,
        keep_existing_priority: false,
    };
    let error = import_mspdi(&plan, &outline_document(""), &unknown).expect_err("project");
    assert_eq!(error.code(), "not_found");
    let taken = import(&plan, &outline_document("")).candidate;
    let other = outline_document("").replace("22222222-2222-4222-8222", "33333333-3333-4333-8333");
    let error = import_mspdi(&taken, &other, &options()).expect_err("key");
    assert!(
        matches!(error, InterchangeError::KeyCollision { .. }),
        "{error}"
    );
}

#[test]
fn export_is_deterministic_and_reimports_without_changes() {
    let plan = workspace();
    let current = import(&plan, &outline_document(&link(1, 1, 60, 6))).candidate;
    let first = export_mspdi(&current, "REL").expect("export");
    assert_eq!(first, export_mspdi(&current, "REL").expect("export"));
    assert!(
        first
            .xml
            .contains(&format!("<GUID>{}</GUID>", guid(2).to_uppercase()))
    );
    let again = import(&current, &first.xml);
    assert_eq!(again.candidate, current);
    let error = {
        let mut broken = current.clone();
        broken.work_items.values_mut().next().expect("work").title = "bell\u{7}".into();
        export_mspdi(&broken, "REL").expect_err("control character")
    };
    assert_eq!(error.code(), "invalid_command");
}

#[test]
fn rejected_links_carry_only_the_rejection_and_zero_durations_are_reported() {
    let plan = workspace();
    // SS from a summary with a working-time lag: rejected, with no approximation note.
    let result = import(&plan, &outline_document(&link(1, 3, 4800, 7)));
    let rejected = &result.report.links[2];
    assert_eq!(rejected.outcome, LinkOutcome::Rejected);
    assert_eq!(rejected.notes.len(), 1, "{:?}", rejected.notes);
    let current = result.candidate;
    let zeroed = outline_document("").replace("PT16H0M0S", "PT0H0M0S");
    let result = import(&current, &zeroed);
    let spec = &result.report.items[1];
    assert_eq!(spec.outcome, ItemOutcome::Updated);
    assert!(!spec.preserved.contains(&"duration".to_string()));
    assert!(spec.approximated.iter().any(|f| f.field == "duration"));
    assert_eq!(work(&result.candidate, "MSP-2").estimate, None);
}

#[test]
fn export_orders_siblings_by_natural_key_order() {
    let tasks: Vec<String> = (1..=12)
        .map(|uid| {
            task(
                uid,
                1,
                &format!("Step {uid}"),
                "<Duration>PT1H0M0S</Duration>",
            )
        })
        .collect();
    let current = import(&workspace(), &document(&tasks)).candidate;
    let xml = export_mspdi(&current, "REL").expect("export").xml;
    let names: Vec<_> = xml
        .split("<Name>")
        .skip(2)
        .filter_map(|rest| rest.split('<').next())
        .collect();
    let expected: Vec<_> = (1..=12).map(|uid| format!("Step {uid}")).collect();
    assert_eq!(names, expected, "MSP-2 precedes MSP-10");
}
