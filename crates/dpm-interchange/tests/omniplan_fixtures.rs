//! MSPDI documents saved by OmniPlan 4.10.3, through the public import API and the reviewed
//! plan-change path.
#![allow(clippy::expect_used, clippy::panic)]

use chrono::{TimeZone, Utc};
use dpm_engine::propose_change;
use dpm_interchange::{
    ExistingMatch, Finding, ImportOptions, ImportResult, ItemOutcome, ItemReport, LinkOutcome,
    import_mspdi,
};
use dpm_model::{
    ActorId, ActorKind, DependencyKind, Key, Plan, Priority, Project, ProjectId, WorkKind,
    WorkStatus,
};
use std::collections::BTreeSet;
use uuid::Uuid;

const NATIVE: &str = include_str!("mspdi/omniplan-native.xml");
const DPM_EXPORT: &str = include_str!("mspdi/dpm-export-cjk.xml");
const OMNIPLAN_ROUND_TRIP: &str = include_str!("mspdi/omniplan-export-of-dpm.xml");

fn workspace() -> Plan {
    let mut plan = Plan::empty("OmniPlan fixtures");
    let project = Project {
        id: ProjectId(Uuid::from_u128(0x3c1f_9a2e_0d4b_4e6a_8f17_5b2c_6d9e_0a41)),
        key: Key::new("REL"),
        parent: None,
        title: "Release".into(),
        objective: "Ship the release".into(),
    };
    plan.projects.insert(project.id, project);
    plan
}

fn options(project: &str, prefix: &str, matching: Option<ExistingMatch>) -> ImportOptions {
    ImportOptions {
        project_key: project.into(),
        key_prefix: Some(prefix.into()),
        match_existing_by: matching,
        keep_existing_priority: false,
    }
}

fn import(plan: &Plan, xml: &str, options: &ImportOptions) -> ImportResult {
    import_mspdi(plan, xml, options).expect("import")
}

/// Apply a candidate as an independent human reviewer, exactly as `plan apply` does.
fn apply(plan: &mut Plan, candidate: Plan) {
    let reviewer = ActorId {
        kind: ActorKind::Human,
        name: "reviewer".into(),
    };
    let at = Utc
        .with_ymd_and_hms(2026, 9, 27, 12, 0, 0)
        .single()
        .expect("time");
    dpm_engine::apply_plan_change(
        plan,
        reviewer,
        &candidate,
        "Import reviewed OmniPlan schedule",
        at,
        dpm_model::OperationId::new(),
    )
    .expect("apply");
}

fn item(result: &ImportResult, uid: i64) -> &ItemReport {
    result
        .report
        .items
        .iter()
        .find(|i| i.uid == uid)
        .expect("item")
}

fn fields(findings: &[Finding]) -> Vec<&str> {
    findings.iter().map(|f| f.field.as_str()).collect()
}

fn edge(plan: &Plan, from: &str, to: &str) -> Vec<(DependencyKind, f64)> {
    let id = |key: &str| plan.find_work_by_key(key).expect(key).id;
    plan.dependencies
        .iter()
        .filter(|d| d.predecessor == id(from) && d.successor == id(to))
        .map(|d| (d.kind, d.lag_hours))
        .collect()
}

#[test]
fn native_file_maps_outline_kinds_names_and_priorities() {
    let plan = workspace();
    let result = import(&plan, NATIVE, &options("REL", "OMNI", None));
    let report = &result.report;
    assert_eq!(report.source.name.as_deref(), Some("DPM OmniPlan 原生夹具"));
    assert_eq!(report.source.project_guid, None);
    let root = item(&result, 0);
    assert_eq!(
        (root.outcome, fields(&root.rejected)),
        (ItemOutcome::Skipped, vec!["project_summary"])
    );
    use {WorkKind::*, WorkStatus::*};
    let mapped: Vec<_> = report.items[1..]
        .iter()
        .map(|i| {
            let work = i.work.as_ref().expect("work");
            (i.name.as_str(), work.key.0.as_str(), work.kind, work.status)
        })
        .collect();
    assert_eq!(
        mapped,
        [
            ("设计 Design", "OMNI-2", WorkPackage, Planned),
            ("编写规格说明", "OMNI-3", Task, Proposed),
            ("审查规格", "OMNI-4", Task, Proposed),
            ("设计批准", "OMNI-5", Milestone, Planned),
            ("构建 Build", "OMNI-6", WorkPackage, Planned),
            ("实现功能", "OMNI-7", Task, Proposed),
            ("编写文档", "OMNI-8", Task, Proposed),
            ("集成测试", "OMNI-9", Task, Proposed),
            ("发布 Release", "OMNI-10", Milestone, Planned),
        ]
    );
    let work = |key: &str| result.candidate.find_work_by_key(key).expect(key);
    assert_eq!(work("OMNI-3").parent, Some(work("OMNI-2").id));
    assert_eq!(work("OMNI-9").parent, Some(work("OMNI-6").id));
    assert_eq!(work("OMNI-10").parent, None);
    assert_eq!(work("OMNI-3").contract.objective, "明确发布范围与接口。");
    assert_eq!(
        work("OMNI-7").schedule.estimate.map(|e| e.likely_hours),
        Some(40.0)
    );
    // OmniPlan's 0..9 priority is written as n * 1000 / 9; DPM's bands recover it.
    use Priority::*;
    assert_eq!(
        ["OMNI-3", "OMNI-4", "OMNI-7", "OMNI-9", "OMNI-5"].map(|key| work(key).schedule.priority),
        [P3, P2, P0, P1, P4]
    );
    for item in &report.items[1..] {
        assert_eq!(fields(&item.approximated)[..2], ["identity", "priority"]);
        assert!(item.approximated[0].detail.contains("key prefix OMNI"));
    }
    propose_change(&plan, &result.candidate).expect("reviewable");
}

#[test]
fn native_file_reports_links_constraint_resource_and_calendars() {
    let result = import(&workspace(), NATIVE, &options("REL", "OMNI", None));
    let report = &result.report;
    // OmniPlan writes a locked start as Must Start On; start-after and end-before constraints
    // set on other tasks are not in the file at all.
    assert_eq!(fields(&item(&result, 8).rejected), ["constraint"]);
    assert!(
        item(&result, 8).rejected[0]
            .detail
            .contains("Must Start On (2026-10-12T09:00:00)")
    );
    assert_eq!(fields(&item(&result, 7).rejected), ["assignments"]);
    assert!(item(&result, 7).rejected[0].detail.contains("李工程师"));
    let document: Vec<_> = report.rejected.iter().map(|f| f.detail.as_str()).collect();
    assert_eq!(
        document,
        [
            "2 calendar(s) not imported; DPM schedules elapsed hours",
            "1 resource(s) not imported; DPM workspace assets are repositories and tools",
        ]
    );
    use LinkOutcome::*;
    let links: Vec<_> = report
        .links
        .iter()
        .map(|l| (l.relation.as_deref(), l.link_lag, l.outcome))
        .collect();
    assert_eq!(
        links,
        [
            (Some("FS"), 1200, Approximated),
            (Some("FS"), 0, Preserved),
            (Some("FS"), 4800, Approximated),
            (Some("SS"), 2400, Approximated),
            (Some("FF"), -1800, Approximated),
            (Some("SF"), 3600, Approximated),
            (Some("FS"), -1200, Approximated),
        ]
    );
    assert!(report.links[0].notes[0].starts_with("no LagFormat, so working-time lag 2 h"));
    let candidate = &result.candidate;
    use DependencyKind::*;
    assert_eq!(edge(candidate, "OMNI-3", "OMNI-4"), [(FinishStart, 2.0)]);
    assert_eq!(edge(candidate, "OMNI-7", "OMNI-8"), [(StartStart, 4.0)]);
    assert_eq!(edge(candidate, "OMNI-7", "OMNI-9"), [(FinishFinish, -3.0)]);
    assert_eq!(edge(candidate, "OMNI-8", "OMNI-9"), [(StartFinish, 6.0)]);
    assert_eq!(edge(candidate, "OMNI-9", "OMNI-10"), [(FinishStart, -2.0)]);
    assert_eq!(candidate.dependencies.len(), 7);
}

#[test]
fn importing_the_native_file_again_changes_nothing_and_never_duplicates() {
    let mut plan = workspace();
    let first = import(&plan, NATIVE, &options("REL", "OMNI", None)).candidate;
    apply(&mut plan, first);
    let again = import(&plan, NATIVE, &options("REL", "OMNI", None));
    assert_eq!(again.candidate, plan);
    let preview = propose_change(&plan, &again.candidate).expect("preview");
    assert!(preview.changes.is_empty());
    assert!(
        again.report.items[1..]
            .iter()
            .all(|i| i.outcome == ItemOutcome::Unchanged)
    );
    let keys: BTreeSet<_> = plan.work_items.values().map(|w| w.key.0.clone()).collect();
    assert_eq!((keys.len(), plan.work_items.len()), (9, 9));
    // Without a prefix naming the source, a GUID-less file is refused rather than guessed.
    let unscoped = ImportOptions {
        key_prefix: None,
        ..options("REL", "OMNI", None)
    };
    let error = import_mspdi(&plan, NATIVE, &unscoped).expect_err("scope");
    assert_eq!(error.code(), "invalid_request");
}

#[test]
fn omniplan_round_trip_maps_back_only_when_asked_and_reports_what_omniplan_changed() {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("plan");
    let exported = import(&plan, DPM_EXPORT, &options("TEST", "DPM", None)).candidate;
    apply(&mut plan, exported);
    // By default OmniPlan's file is another source: all new work beside the original.
    let separate = import(&plan, OMNIPLAN_ROUND_TRIP, &options("TEST", "OPR", None));
    assert!(
        separate.report.items[1..]
            .iter()
            .all(|i| i.outcome == ItemOutcome::Created)
    );
    assert_eq!(separate.report.retained.len(), plan.work_items.len());
    let matching = options("TEST", "OPR", Some(ExistingMatch::TitlePath));
    let matched = import(&plan, OMNIPLAN_ROUND_TRIP, &matching);
    let report = &matched.report;
    assert!(report.retained.is_empty());
    for item in &report.items[1..] {
        assert!(
            item.approximated[0]
                .detail
                .contains("matched existing work")
        );
    }
    let updated: Vec<_> = report.items[1..]
        .iter()
        .filter(|i| i.outcome != ItemOutcome::Unchanged)
        .map(|i| {
            let change = &i.changes[0];
            let values = (change.before.as_str(), change.after.as_str());
            (
                i.name.as_str(),
                change.field.as_str(),
                values,
                i.changes.len(),
            )
        })
        .collect();
    // OmniPlan writes priority 0 for every group.
    assert_eq!(
        updated,
        [
            ("Design", "priority", ("P2", "P4"), 1),
            ("Build", "priority", ("P2", "P4"), 1)
        ]
    );
    // OmniPlan rewrote the SF link into the zero-duration Release milestone as SS: the same bound
    // for a milestone, but a different relation, so the report shows it instead of hiding it.
    let removed: Vec<_> = report
        .removed_dependencies
        .iter()
        .map(|d| (d.predecessor.0.as_str(), d.successor.0.as_str(), d.kind))
        .collect();
    use DependencyKind::*;
    assert_eq!(removed, [("DPM-8", "DPM-1", StartFinish)]);
    assert_eq!(
        edge(&matched.candidate, "DPM-8", "DPM-1"),
        [(StartStart, 24.0)]
    );
    assert_eq!(
        edge(&matched.candidate, "DPM-7", "DPM-9"),
        [(FinishFinish, -3.0)]
    );
    let preview = propose_change(&plan, &matched.candidate).expect("preview");
    assert_eq!(preview.changes.len(), 4);
    apply(&mut plan, matched.candidate);
    let again = import(&plan, OMNIPLAN_ROUND_TRIP, &matching);
    let preview = propose_change(&plan, &again.candidate).expect("preview");
    assert!(preview.changes.is_empty());
}

const DPM_EXPORT_NO_P0: &str = include_str!("mspdi/dpm-export-no-p0.xml");
const OMNIPLAN_NO_P0: &str = include_str!("mspdi/omniplan-export-no-p0.xml");

/// The execution plan with priorities P1, P2, P3 in key order and no P0: the pre-image of
/// `dpm-export-no-p0.xml`.
fn plan_without_p0() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("plan");
    let mut work: Vec<_> = plan.work_items.values_mut().collect();
    work.sort_by(|a, b| a.key.cmp(&b.key));
    for (item, priority) in work.into_iter().zip(
        [Priority::P1, Priority::P2, Priority::P3]
            .into_iter()
            .cycle(),
    ) {
        item.schedule.priority = priority;
    }
    plan
}

#[test]
fn omniplan_rescales_priorities_by_the_highest_one_so_existing_priority_can_be_kept() {
    let plan = plan_without_p0();
    assert_eq!(
        import(&plan, DPM_EXPORT_NO_P0, &options("TEST", "DPM", None)).candidate,
        plan,
        "the pinned pre-image describes the same priorities and structure"
    );
    let matching = options("TEST", "OPR", Some(ExistingMatch::TitlePath));
    // OmniPlan writes ⌊level·1000 ÷ highest level⌋: 700/500/300 come back as 1000/714/428.
    let raised = import(&plan, OMNIPLAN_NO_P0, &matching);
    let mut bands: Vec<_> = raised.report.items[1..]
        .iter()
        .flat_map(|i| &i.changes)
        .filter(|c| c.field == "priority")
        .map(|c| (c.before.as_str(), c.after.as_str()))
        .collect();
    bands.sort_unstable();
    bands.dedup();
    assert_eq!(bands, [("P1", "P0"), ("P2", "P1"), ("P3", "P2")]);
    let kept = import(
        &plan,
        OMNIPLAN_NO_P0,
        &ImportOptions {
            keep_existing_priority: true,
            ..matching
        },
    );
    let preview = propose_change(&plan, &kept.candidate).expect("preview");
    assert!(preview.changes.is_empty(), "{:?}", preview.changes);
    for item in &kept.report.items[1..] {
        assert!(item.kept.contains(&"priority".to_string()), "{}", item.name);
        let finding = item
            .approximated
            .iter()
            .find(|f| f.field == "priority")
            .expect("the source value stays visible");
        assert!(finding.detail.contains("not applied"), "{}", finding.detail);
    }
}
