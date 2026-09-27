//! MSPDI documents written by an independent tool, through the public import/export API and the
//! reviewed plan-change path.
#![allow(clippy::expect_used, clippy::panic)]

use chrono::{TimeZone, Utc};
use dpm_engine::{Command, apply_command, propose_change};
use dpm_interchange::{
    ImportOptions, ImportResult, ItemOutcome, LinkOutcome, export_mspdi, import_mspdi,
};
use dpm_model::{
    ActorId, ActorKind, DependencyKind, Key, Plan, Project, ProjectId, WorkKind, WorkStatus,
};
use std::collections::BTreeSet;
use uuid::Uuid;

const RELEASE: &str = include_str!("mspdi/mpxj-release-plan.xml");
const UNSUPPORTED: &str = include_str!("mspdi/mpxj-unsupported-features.xml");
const EXPORTED_RELEASE: &str = include_str!("mspdi/dpm-export-release-plan.xml");
const MPXJ_REWRITE: &str = include_str!("mspdi/mpxj-rewrite-of-dpm-export.xml");

fn workspace() -> Plan {
    let mut plan = Plan::empty("Interchange fixtures");
    let project = Project {
        id: ProjectId(Uuid::from_u128(0x7d6a_0c1e_5b0f_4f53_9d51_2f0b_3c7c_9a01)),
        key: Key::new("REL"),
        parent: None,
        title: "Release".into(),
        objective: "Ship the release".into(),
    };
    plan.projects.insert(project.id, project);
    plan
}

fn import(plan: &Plan, xml: &str) -> ImportResult {
    let options = ImportOptions {
        project_key: "REL".into(),
        key_prefix: None,
        match_existing_by: None,
    };
    import_mspdi(plan, xml, &options).expect("import")
}

/// Apply a candidate as an independent human reviewer, exactly as `plan apply` does.
fn apply(plan: &mut Plan, candidate: Plan) {
    let reviewer = ActorId {
        kind: ActorKind::Human,
        name: "reviewer".into(),
    };
    let at = Utc
        .with_ymd_and_hms(2026, 9, 26, 12, 0, 0)
        .single()
        .expect("time");
    let command = Command::ApplyChange {
        plan: Box::new(candidate),
        reason: "Import reviewed schedule".into(),
    };
    apply_command(plan, reviewer, command, at).expect("apply");
}

fn edge_lags(plan: &Plan, from: &str, to: &str) -> Vec<(DependencyKind, f64)> {
    let id = |key: &str| plan.find_work_by_key(key).expect(key).id;
    plan.dependencies
        .iter()
        .filter(|d| d.predecessor == id(from) && d.successor == id(to))
        .map(|d| (d.kind, d.lag_hours))
        .collect()
}

#[test]
fn release_fixture_maps_outline_durations_and_links() {
    let plan = workspace();
    let result = import(&plan, RELEASE);
    let candidate = &result.candidate;
    assert!(result.report.source.scope.contains("not calendar dates"));
    assert_eq!(result.report.items[0].outcome, ItemOutcome::Skipped);
    let kinds: Vec<_> = result.report.items[1..]
        .iter()
        .map(|i| {
            let work = i.work.as_ref().expect("work");
            (work.key.0.as_str(), work.kind, work.status)
        })
        .collect();
    use {WorkKind::*, WorkStatus::*};
    assert_eq!(
        kinds,
        [
            ("REL-2", WorkPackage, Planned),
            ("REL-3", Task, Proposed),
            ("REL-4", Task, Proposed),
            ("REL-5", Milestone, Planned),
            ("REL-6", WorkPackage, Planned),
            ("REL-7", Task, Proposed),
            ("REL-8", Task, Proposed),
            ("REL-9", Task, Proposed),
            ("REL-10", Milestone, Planned),
        ]
    );
    let work = |key: &str| candidate.find_work_by_key(key).expect(key);
    assert_eq!(work("REL-3").parent, Some(work("REL-2").id));
    assert_eq!(work("REL-10").parent, None);
    assert_eq!(
        work("REL-3").objective,
        "Specify the release scope and interfaces."
    );
    assert_eq!(work("REL-7").priority, dpm_model::Priority::P1);
    let hours = |key: &str| work(key).estimate.map(|e| e.likely_hours);
    assert_eq!(
        [
            hours("REL-3"),
            hours("REL-4"),
            hours("REL-7"),
            hours("REL-8"),
            hours("REL-9")
        ],
        [Some(16.0), Some(8.0), Some(40.0), Some(12.0), Some(16.0)]
    );
    assert!(work("REL-3").acceptance.is_empty());
    assert_eq!(
        edge_lags(candidate, "REL-4", "REL-5"),
        [(DependencyKind::FinishStart, 2.0)]
    );
    assert_eq!(
        edge_lags(candidate, "REL-7", "REL-8"),
        [(DependencyKind::StartStart, 4.0)]
    );
    assert_eq!(
        edge_lags(candidate, "REL-7", "REL-9"),
        [(DependencyKind::FinishFinish, -3.0)]
    );
    assert_eq!(
        edge_lags(candidate, "REL-8", "REL-10"),
        [(DependencyKind::StartFinish, 24.0)]
    );
    // Design -> Build (FS, 1 working day) binds every Design leaf to every Build leaf.
    for from in ["REL-3", "REL-4", "REL-5"] {
        for to in ["REL-7", "REL-8", "REL-9"] {
            assert_eq!(
                edge_lags(candidate, from, to),
                [(DependencyKind::FinishStart, 8.0)]
            );
        }
    }
    assert_eq!(candidate.dependencies.len(), 15);
    let summary = &result.report.links[2];
    assert_eq!(summary.outcome, LinkOutcome::Approximated);
    assert_eq!(summary.dependencies.len(), 9);
    assert_eq!(result.report.rejected[0].field, "calendars");
    let preview = propose_change(&plan, candidate).expect("preview");
    assert_eq!(preview.changes.len(), 9 + 15);
}

#[test]
fn unsupported_fixture_reports_every_category_and_completes_nothing() {
    let mut plan = workspace();
    let result = import(&plan, UNSUPPORTED);
    let report = &result.report;
    let mut fields: BTreeSet<&str> = report.rejected.iter().map(|f| f.field.as_str()).collect();
    for item in &report.items {
        fields.extend(item.rejected.iter().map(|f| f.field.as_str()));
        fields.extend(item.approximated.iter().map(|f| f.field.as_str()));
    }
    let expected = [
        "PercentComplete",
        "actuals",
        "assignments",
        "baselines",
        "calendars",
        "constraint",
        "custom_fields",
        "deadline",
        "duration",
        "identity",
        "resources",
        "task",
    ];
    assert_eq!(fields, expected.into_iter().collect());
    let by_name = |name: &str| report.items.iter().find(|i| i.name == name).expect(name);
    assert_eq!(by_name("Inactive task").outcome, ItemOutcome::Skipped);
    assert!(
        by_name("Task without GUID")
            .approximated
            .iter()
            .any(|f| f.field == "identity")
    );
    assert_eq!(
        by_name("Milestone with duration")
            .work
            .as_ref()
            .expect("work")
            .kind,
        WorkKind::Milestone
    );
    assert_eq!(
        by_name("Zero duration task")
            .work
            .as_ref()
            .expect("work")
            .kind,
        WorkKind::Task
    );
    let outcomes: Vec<_> = report.links.iter().map(|l| l.outcome).collect();
    use LinkOutcome::*;
    assert_eq!(outcomes, [Preserved, Rejected, Rejected, Approximated]);
    apply(&mut plan, result.candidate);
    let finished = plan.find_work_by_key("REL-4").expect("finished");
    assert_eq!(
        (finished.title.as_str(), finished.status),
        ("Finished task", WorkStatus::Proposed)
    );
    let progress = dpm_engine::progress(&plan, chrono::Utc::now()).expect("progress");
    assert!(!progress.work[&finished.id].verified);
    assert_eq!(finished.reported_progress_percent, 0);
}

#[test]
fn applied_fixture_reimports_and_exports_without_semantic_changes() {
    let mut plan = workspace();
    let first = import(&plan, RELEASE).candidate;
    apply(&mut plan, first);
    let again = import(&plan, RELEASE);
    assert!(
        propose_change(&plan, &again.candidate)
            .expect("preview")
            .changes
            .is_empty()
    );
    assert!(
        again.report.items[1..]
            .iter()
            .all(|i| i.outcome == ItemOutcome::Unchanged)
    );
    let exported = export_mspdi(&plan, "REL").expect("export");
    assert_eq!(exported.xml, EXPORTED_RELEASE);
    let round_trip = import(&plan, &exported.xml);
    assert_eq!(round_trip.candidate, plan);
    // MPXJ read that export and wrote its own encoding; it must mean the same plan.
    let rewritten = import(&plan, MPXJ_REWRITE);
    assert_eq!(rewritten.candidate, plan);
    assert!(
        rewritten
            .report
            .links
            .iter()
            .all(|l| l.outcome == LinkOutcome::Preserved)
    );
}

#[test]
fn cjk_names_survive_export_and_repeated_imports_never_duplicate() {
    let source = RELEASE
        .replace(
            "<Name>Write specification</Name>",
            "<Name>编写规格说明</Name>",
        )
        .replace("<Name>Design approved</Name>", "<Name>设计批准</Name>");
    let mut plan = workspace();
    let first = import(&plan, &source).candidate;
    apply(&mut plan, first);
    let count = plan.work_items.len();
    for xml in [
        source.clone(),
        export_mspdi(&plan, "REL").expect("export").xml,
    ] {
        assert!(xml.contains("编写规格说明") && xml.contains("设计批准"));
        let again = import(&plan, &xml);
        assert_eq!(again.candidate.work_items.len(), count);
        assert!(
            propose_change(&plan, &again.candidate)
                .expect("preview")
                .changes
                .is_empty()
        );
    }
    assert_eq!(
        plan.find_work_by_key("REL-3").expect("spec").title,
        "编写规格说明"
    );
}

#[test]
fn dpm_plans_export_and_reimport_unchanged() {
    for source in [
        include_str!("../../../tests/support/execution-plan.json"),
        include_str!("../../../tests/support/conditional-plan.json"),
        include_str!("../../../examples/self-host/dpm-alpha.json"),
    ] {
        let plan: Plan = serde_json::from_str(source).expect("plan");
        for project in plan.projects.values() {
            let exported = export_mspdi(&plan, &project.key.0).expect("export");
            let options = ImportOptions {
                project_key: project.key.0.clone(),
                key_prefix: None,
                match_existing_by: None,
            };
            let result = import_mspdi(&plan, &exported.xml, &options).expect("import");
            assert_eq!(result.candidate, plan, "{}", project.key);
            let preview = propose_change(&plan, &result.candidate).expect("preview");
            assert!(preview.changes.is_empty(), "{}", project.key);
        }
    }
}

#[test]
fn conditional_work_is_written_unconditionally_and_reported_never_dropped() {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/conditional-plan.json"))
            .expect("plan");
    let decision = plan
        .find_decision_by_key("DEC-SUPPLIER")
        .expect("decision")
        .id;
    let command = Command::Decide {
        decision,
        outcome: "B".into(),
    };
    apply_command(&mut plan, ActorId::human("lead"), command, Utc::now()).expect("decide");
    let exported = export_mspdi(&plan, "SUP").expect("export");
    assert_eq!(exported.report.items.len(), plan.work_items.len());
    let item = |key: &str| {
        exported
            .report
            .items
            .iter()
            .find(|i| i.key.0 == key)
            .expect("reported")
    };
    let fields = |key: &str| -> BTreeSet<String> {
        item(key).omitted.iter().map(|f| f.field.clone()).collect()
    };
    assert!(fields("SUP-PKG-A").contains("condition"));
    assert!(fields("SUP-A-QUOTE").contains("applicability"));
    assert!(fields("SUP-MERGE").contains("join"));
    assert!(fields("SUP-A-AUDIT").contains("applicability"));
    assert!(exported.xml.contains("Obtain supplier A quote"));
    assert!(
        exported
            .report
            .dependencies
            .iter()
            .any(|d| d.notes.iter().any(|n| n.contains("not selected")))
    );
}
