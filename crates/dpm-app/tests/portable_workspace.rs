//! A portable execution graph coordinates repositories and non-code work across nested projects.
#![allow(clippy::expect_used, clippy::panic)]

use dpm_app::{Application, CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::{ActorId, AssetId, Key, Plan, ProjectId};

fn mixed_plan() -> Plan {
    let mut plan: Plan =
        serde_json::from_str(include_str!("../../../tests/support/execution-plan.json"))
            .expect("fixture");
    plan.decisions.clear();
    let root = plan.projects.values().next().expect("project").clone();
    let repository = plan.assets.values().next().expect("asset").clone();
    let second_asset = AssetId::new();
    let mut second = repository.clone();
    second.id = second_asset;
    second.key = Key::new("SECOND-REPOSITORY");
    plan.assets.insert(second_asset, second);
    for (key, project_key, asset) in [
        ("TEST-A", "SOFTWARE", Some(repository.id)),
        ("TEST-B", "FIRMWARE", Some(second_asset)),
        ("TEST-C", "PROCUREMENT", None),
    ] {
        let project = ProjectId::new();
        plan.projects.insert(
            project,
            dpm_model::Project {
                id: project,
                key: Key::new(project_key),
                parent: Some(root.id),
                title: project_key.into(),
                objective: "Deliver one part of a shared outcome".into(),
            },
        );
        let work = plan.find_work_by_key_mut(key).expect("work");
        work.project = project;
        work.contract.assets = asset
            .map(|asset| dpm_model::AssetRequirement {
                asset,
                access: dpm_model::AssetAccess::Write,
            })
            .into_iter()
            .collect();
    }
    plan.validate().expect("mixed graph");
    plan
}

fn ready(app: &Application, key: &str) -> bool {
    app.query_blocking(Query::Explain { key: key.into() })
        .expect("explain")
        .data["ready"]
        .as_bool()
        .expect("readiness")
}

#[test]
fn portable_nested_workspace_keeps_cross_repository_execution_and_history() {
    let source = mixed_plan();
    let json = serde_json::to_string(&source).expect("JSON");
    let portable: Plan = serde_json::from_str(&json).expect("JSON import");
    let toml = toml::to_string(&portable).expect("TOML");
    let portable: Plan = toml::from_str(&toml).expect("TOML import");
    assert_eq!(portable, source);
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("workspace.sqlite");
    let mut app = Application::initialize_blocking(&path, &portable).expect("store");
    let a = portable.find_work_by_key("TEST-A").expect("a").id;
    assert!(ready(&app, "TEST-A"));
    assert!(!ready(&app, "TEST-B"));
    assert_eq!(app.plan_blocking().expect("read only queries"), portable);
    for command in [
        Command::Claim { work: a },
        Command::Start { work: a },
        Command::Submit {
            work: a,
            note: None,
        },
        Command::Verify {
            work: a,
            note: None,
        },
    ] {
        let actor = if matches!(command, Command::Verify { .. }) {
            ActorId::human("independent-reviewer")
        } else {
            ActorId::agent("worker")
        };
        app.execute_blocking(CommandRequest {
            actor,
            base_revision: app.plan_blocking().expect("revision").revision,
            command,
        })
        .expect("transition");
    }
    assert!(ready(&app, "TEST-B"));
    let completed = app.plan_blocking().expect("snapshot");
    assert_eq!(completed.dependencies, source.dependencies);
    assert!(
        completed
            .find_work_by_key("TEST-C")
            .expect("non-code")
            .contract
            .assets
            .is_empty()
    );
    drop(app);
    let reopened = Application::open_blocking(&path).expect("reopen");
    assert_eq!(reopened.plan_blocking().expect("durable"), completed);
    assert!(ready(&reopened, "TEST-B"));
    let history = reopened
        .query_blocking(Query::History {
            after_sequence: 0,
            limit: 10,
        })
        .expect("history");
    assert_eq!(
        history.data["entries"]
            .as_array()
            .expect("operations")
            .len(),
        4
    );
}
