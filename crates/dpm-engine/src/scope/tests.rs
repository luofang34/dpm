use super::*;
use crate::{Command, apply_command};
use chrono::Utc;
use dpm_model::{
    ActorId, AssetKind, AssetRequirement, Priority, Project, WorkItemId, WorkspaceAsset,
};

const ROOT: &str = "TEST";
const SUB: &str = "SUB";
const OTHER: &str = "OTHER";
const REPO_A: &str = "TEST-REPO";
const REPO_B: &str = "REPO-B";

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn project_id(plan: &Plan, key: &str) -> ProjectId {
    plan.projects
        .values()
        .find(|p| p.key.0 == key)
        .expect("project")
        .id
}

fn asset_id(plan: &Plan, key: &str) -> AssetId {
    plan.assets
        .values()
        .find(|r| r.key.0 == key)
        .expect("asset")
        .id
}

/// Ready tasks across nested projects and two repositories, plus a non-code task.
fn scoped_plan() -> Plan {
    let mut plan = fixture();
    let template = plan.find_work_by_key("TEST-A").expect("task").clone();
    let dependency = plan.dependencies[0].clone();
    plan.work_items.clear();
    plan.dependencies.clear();
    plan.decisions.clear();
    plan.risks.clear();
    let root = project_id(&plan, ROOT);
    for (key, parent) in [(SUB, Some(root)), (OTHER, None)] {
        let id = ProjectId::new();
        plan.projects.insert(
            id,
            Project {
                id,
                key: Key::new(key),
                parent,
                title: format!("{key} project"),
                objective: "Scope membership".into(),
            },
        );
    }
    let b = AssetId::new();
    plan.assets.insert(
        b,
        WorkspaceAsset {
            id: b,
            key: Key::new(REPO_B),
            label: "Second repository".into(),
            kind: AssetKind::GitRepository {
                remotes: Vec::new(),
            },
        },
    );
    let a = asset_id(&plan, REPO_A);
    let read = |asset| AssetRequirement {
        asset,
        access: AssetAccess::Read,
    };
    let write = |asset| AssetRequirement {
        asset,
        access: AssetAccess::Write,
    };
    let tasks = [
        ("W-A", ROOT, vec![write(a)], Priority::P2),
        ("W-B", OTHER, vec![write(b)], Priority::P0),
        ("W-AB", SUB, vec![write(a), write(b)], Priority::P2),
        ("R-A", SUB, vec![read(a)], Priority::P3),
        ("R-A-W-B", ROOT, vec![read(a), write(b)], Priority::P2),
        ("NOCODE", OTHER, Vec::new(), Priority::P1),
        ("DEP", ROOT, vec![write(a)], Priority::P0),
        ("CAP", ROOT, vec![write(a)], Priority::P0),
    ];
    for (key, project, assets, priority) in tasks {
        let mut work = template.clone();
        work.id = WorkItemId::new();
        work.key = Key::new(key);
        work.project = project_id(&plan, project);
        work.contract.assets = assets;
        work.schedule.priority = priority;
        if key == "CAP" {
            work.contract.capabilities = BTreeSet::from(["design".to_string()]);
        }
        plan.work_items.insert(work.id, work);
    }
    let mut edge = dependency;
    edge.predecessor = plan.find_work_by_key("W-B").expect("W-B").id;
    edge.successor = plan.find_work_by_key("DEP").expect("DEP").id;
    plan.dependencies.push(edge);
    plan.validate().expect("valid scoped plan");
    plan
}

fn query() -> NextWorkQuery {
    NextWorkQuery {
        capabilities: BTreeSet::from(["rust".to_string(), "testing".to_string()]),
        use_probabilistic_criticality: false,
    }
}

fn run(plan: &Plan, projects: &[&str], assets: &[&str], limit: usize) -> NextWorkResult {
    let scope =
        WorkScope::resolve(plan, projects.iter().copied(), assets.iter().copied()).expect("scope");
    next_in_scope(plan, &query(), &scope, limit, chrono::Utc::now()).expect("next")
}

fn keys(result: &NextWorkResult) -> Vec<String> {
    result
        .candidates
        .iter()
        .map(|c| c.candidate.work.key.0.clone())
        .collect()
}

fn outside(result: &NextWorkResult) -> Vec<String> {
    result
        .outside_scope
        .keys
        .iter()
        .map(|k| k.0.clone())
        .collect()
}

fn global_order(plan: &Plan) -> Vec<String> {
    next_work(plan, &query(), chrono::Utc::now())
        .expect("global")
        .into_iter()
        .map(|c| c.work.key.0)
        .collect()
}

#[test]
fn unscoped_result_equals_the_global_ranking_and_hides_nothing() {
    let plan = scoped_plan();
    let result = run(&plan, &[], &[], 100);
    assert_eq!(result.result_version, NEXT_RESULT_VERSION);
    assert!(result.scope.is_unscoped());
    assert_eq!(keys(&result), global_order(&plan));
    assert_eq!(result.outside_scope, OutsideScope::default());
    let ranks = result.candidates.iter().map(|c| c.global_rank);
    assert!(ranks.eq(1..=result.eligible_count));
}

#[test]
fn asset_scope_requires_a_positive_match_and_every_write_to_fit() {
    let plan = scoped_plan();
    let only_a = run(&plan, &[], &[REPO_A], 100);
    let mut inside = keys(&only_a);
    inside.sort();
    // Read-only work fits; multi-write work and a write outside the set do not; no-code never matches.
    assert_eq!(inside, ["R-A", "W-A"]);
    for key in ["W-B", "W-AB", "R-A-W-B", "NOCODE"] {
        assert!(outside(&only_a).contains(&key.to_string()), "{key}");
    }
    let both = run(&plan, &[], &[REPO_A, REPO_B], 100);
    let mut inside = keys(&both);
    inside.sort();
    assert_eq!(inside, ["R-A", "R-A-W-B", "W-A", "W-AB", "W-B"]);
    assert_eq!(outside(&both), ["NOCODE"]);
}

#[test]
fn asset_less_work_is_outside_a_asset_scope_but_inside_a_project_scope() {
    let plan = scoped_plan();
    let by_asset = run(&plan, &[], &[REPO_B], 100);
    assert!(!keys(&by_asset).contains(&"NOCODE".to_string()));
    assert!(outside(&by_asset).contains(&"NOCODE".to_string()));
    let by_project = run(&plan, &[OTHER], &[], 100);
    let mut inside = keys(&by_project);
    inside.sort();
    assert_eq!(inside, ["NOCODE", "W-B"]);
}

#[test]
fn project_scope_includes_descendants_and_intersects_with_assets() {
    let plan = scoped_plan();
    let mut subtree = keys(&run(&plan, &[ROOT], &[], 100));
    subtree.sort();
    assert_eq!(subtree, ["R-A", "R-A-W-B", "W-A", "W-AB"]);
    let mut intersect = keys(&run(&plan, &[ROOT], &[REPO_A], 100));
    intersect.sort();
    assert_eq!(intersect, ["R-A", "W-A"]);
    assert!(keys(&run(&plan, &[OTHER], &[REPO_A], 100)).is_empty());
}

#[test]
fn filtered_candidates_retain_global_order_and_rank() {
    let plan = scoped_plan();
    let global = global_order(&plan);
    for (projects, assets) in [
        (vec![ROOT], vec![]),
        (vec![], vec![REPO_A]),
        (vec![SUB], vec![REPO_A, REPO_B]),
    ] {
        let result = run(&plan, &projects, &assets, 100);
        for candidate in &result.candidates {
            assert_eq!(
                global[candidate.global_rank - 1],
                candidate.candidate.work.key.0
            );
        }
        let ranks = result.candidates.iter().map(|c| c.global_rank);
        assert!(ranks.clone().zip(ranks.skip(1)).all(|(a, b)| a < b));
        let mut merged = keys(&result);
        merged.extend(outside(&result));
        merged.sort_by_key(|key| global.iter().position(|g| g == key));
        assert_eq!(merged, global);
    }
}

#[test]
fn out_of_scope_dependency_never_satisfies_in_scope_work() {
    let mut plan = scoped_plan();
    let result = run(&plan, &[], &[REPO_A], 100);
    assert!(!keys(&result).contains(&"DEP".to_string()));
    assert!(!outside(&result).contains(&"DEP".to_string()));
    assert!(outside(&result).contains(&"W-B".to_string()));
    let predecessor = plan.find_work_by_key("W-B").expect("W-B").id;
    for (actor, command) in [
        (
            ActorId::agent("worker"),
            Command::Claim { work: predecessor },
        ),
        (
            ActorId::agent("worker"),
            Command::Start { work: predecessor },
        ),
        (
            ActorId::agent("worker"),
            Command::Submit {
                work: predecessor,
                note: None,
            },
        ),
        (
            ActorId::human("reviewer"),
            Command::Verify {
                work: predecessor,
                note: None,
            },
        ),
    ] {
        apply_command(
            &mut plan,
            actor,
            command,
            Utc::now(),
            dpm_model::OperationId::new(),
        )
        .expect("complete W-B");
    }
    assert!(keys(&run(&plan, &[], &[REPO_A], 100)).contains(&"DEP".to_string()));
}

#[test]
fn higher_ranked_count_is_relative_to_the_best_in_scope_work() {
    let plan = scoped_plan();
    let global = global_order(&plan);
    let result = run(&plan, &[], &[REPO_A], 1);
    let best = result.candidates[0].global_rank;
    assert_eq!(result.outside_scope.higher_ranked_count, best - 1);
    assert!(result.outside_scope.higher_ranked_count >= 1);
    let higher = &outside(&result)[..result.outside_scope.higher_ranked_count];
    assert_eq!(higher, &global[..best - 1]);
    // The P0 blocker in another repository stays visible above the scoped result.
    assert_eq!(higher[0], "W-B");
}

#[test]
fn empty_scope_reports_all_eligible_outside_work_as_higher_ranked() {
    let mut plan = scoped_plan();
    let docs = AssetId::new();
    plan.assets.insert(
        docs,
        WorkspaceAsset {
            id: docs,
            key: Key::new("DOCS"),
            label: "Documents".into(),
            kind: AssetKind::DocumentCollection,
        },
    );
    let result = run(&plan, &[], &["DOCS"], 5);
    assert!(result.candidates.is_empty());
    assert_eq!(result.in_scope_count, 0);
    assert_eq!(result.outside_scope.count, result.eligible_count);
    assert_eq!(
        result.outside_scope.higher_ranked_count,
        result.eligible_count
    );
    assert_eq!(outside(&result), global_order(&plan));
}

#[test]
fn limit_applies_after_filtering_without_changing_outside_context() {
    let plan = scoped_plan();
    let full = run(&plan, &[ROOT], &[], 100);
    assert!(full.in_scope_count > 2);
    let limited = run(&plan, &[ROOT], &[], 2);
    assert_eq!(keys(&limited), keys(&full)[..2]);
    assert_eq!(limited.in_scope_count, full.in_scope_count);
    assert_eq!(limited.outside_scope, full.outside_scope);
    let none = run(&plan, &[ROOT], &[], 0);
    assert!(none.candidates.is_empty());
    assert_eq!(none.in_scope_count, full.in_scope_count);
    assert_eq!(none.outside_scope, full.outside_scope);
}

#[test]
fn capability_eligibility_is_distinct_from_scope() {
    let plan = scoped_plan();
    let result = run(&plan, &[ROOT], &[], 100);
    assert!(!keys(&result).contains(&"CAP".to_string()));
    assert!(!outside(&result).contains(&"CAP".to_string()));
    let all = next_in_scope(
        &plan,
        &NextWorkQuery::default(),
        &WorkScope::default(),
        100,
        chrono::Utc::now(),
    )
    .expect("next");
    assert!(keys(&all).contains(&"CAP".to_string()));
}

#[test]
fn unknown_scope_keys_are_rejected_instead_of_matching_nothing() {
    let plan = scoped_plan();
    assert!(matches!(
        WorkScope::resolve(&plan, ["MISSING"], []),
        Err(ScopeError::UnknownProject(key)) if key == "MISSING"
    ));
    assert!(matches!(
        WorkScope::resolve(&plan, [], ["MISSING"]),
        Err(ScopeError::UnknownResource(key)) if key == "MISSING"
    ));
}

#[test]
fn scoped_queries_do_not_change_the_plan() {
    let plan = scoped_plan();
    let before = serde_json::to_value(&plan).expect("before");
    run(&plan, &[ROOT], &[REPO_A], 1);
    assert_eq!(serde_json::to_value(&plan).expect("after"), before);
}
