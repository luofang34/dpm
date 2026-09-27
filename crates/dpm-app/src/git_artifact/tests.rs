use crate::{AppError, Application, ProjectLocation, WorkspaceRegistry};
use dpm_model::{
    ActorId, AssetAccess, AssetId, AssetKind, AssetRequirement, Key, Plan, WorkspaceAsset,
};
use std::fs;
use tempfile::TempDir;

/// TEST-A requires two Git repositories and one document collection.
fn plan() -> Plan {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    for (key, kind) in [
        ("SECOND-REPO", AssetKind::GitRepository { remotes: vec![] }),
        ("NOTES", AssetKind::DocumentCollection),
        ("UNUSED-REPO", AssetKind::GitRepository { remotes: vec![] }),
    ] {
        let id = AssetId::new();
        plan.assets.insert(
            id,
            WorkspaceAsset {
                id,
                key: Key::new(key),
                label: key.into(),
                kind,
            },
        );
        if key != "UNUSED-REPO" {
            plan.work_items
                .get_mut(&work)
                .expect("task")
                .contract
                .assets
                .push(AssetRequirement {
                    asset: id,
                    access: AssetAccess::Write,
                });
        }
    }
    plan.validate().expect("valid multi-asset plan");
    plan
}

fn rejected(app: &Application, asset: Option<&str>) -> String {
    let work = app.work_id_blocking("TEST-A").expect("task");
    match app.git_head_artifact_blocking(ActorId::agent("worker"), work, asset) {
        Err(AppError::InvalidRequest(message)) => message,
        other => panic!("expected an invalid request, got {other:?}"),
    }
}

#[test]
fn capture_without_one_explicit_task_repository_is_rejected_before_reading_git() {
    let temp = TempDir::new().expect("temp");
    let plan = plan();
    let database = temp.path().join("shared.sqlite");
    Application::initialize_blocking(&database, &plan).expect("store");
    let unbound = Application::open_blocking(&database).expect("open");
    assert!(rejected(&unbound, None).contains("explicit --asset"));
    assert!(rejected(&unbound, Some("MISSING")).contains("unknown asset"));
    assert!(rejected(&unbound, Some("NOTES")).contains("task's requirements"));
    assert!(rejected(&unbound, Some("UNUSED-REPO")).contains("task's requirements"));

    let registry = WorkspaceRegistry::at(temp.path().join("bindings.sqlite"));
    registry
        .register_blocking(&database, false)
        .expect("register");
    let checkout = temp.path().join("checkout");
    fs::create_dir_all(checkout.join(".dpm")).expect("directory");
    fs::write(
        checkout.join(".dpm/project.toml"),
        format!(
            "version = 3\nworkspace = '{}'\nasset = 'TEST-REPO'\n",
            plan.workspace.id
        ),
    )
    .expect("locator");
    let bound = ProjectLocation::at_blocking(&checkout)
        .expect("location")
        .open_with_registry_blocking(&registry)
        .expect("open");
    assert!(rejected(&bound, Some("SECOND-REPO")).contains("locator binding"));
    assert_eq!(
        Application::open_blocking(&database)
            .expect("reopen")
            .plan_blocking()
            .expect("plan"),
        plan
    );
}
