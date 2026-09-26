use super::*;
use crate::{CommandRequest, ProjectLocation};
use dpm_engine::Command;
use dpm_model::{ActorId, Plan};
use tempfile::TempDir;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn locator_blocking(path: &Path, workspace: WorkspaceId) -> ProjectLocation {
    fs::create_dir_all(path.join(".dpm")).expect("directory");
    fs::write(
        path.join(".dpm/project.toml"),
        format!("version = 2\nworkspace = '{workspace}'\n"),
    )
    .expect("locator");
    ProjectLocation::at_blocking(path).expect("location")
}

#[test]
fn two_entry_points_observe_one_claim_without_copying_or_resetting_history() {
    let temp = TempDir::new().expect("temp");
    let registry = WorkspaceRegistry::at(temp.path().join("config/bindings.sqlite"));
    let plan = fixture();
    let database = temp.path().join("shared.sqlite");
    let first = locator_blocking(&temp.path().join("code"), plan.workspace.id);
    let second = locator_blocking(&temp.path().join("documents"), plan.workspace.id);
    assert!(registry.list_blocking().expect("empty").is_empty());
    assert!(!temp.path().join("config").exists());
    assert_eq!(
        first
            .open_with_registry_blocking(&registry)
            .err()
            .expect("unbound")
            .code(),
        "workspace_not_bound"
    );
    assert!(!database.exists());
    Application::initialize_blocking(&database, &plan).expect("store");
    registry
        .register_blocking(&database, false)
        .expect("register");
    let mut app = first.open_with_registry_blocking(&registry).expect("first");
    let work = app.work_id_blocking("TEST-A").expect("task");
    app.execute_blocking(CommandRequest {
        actor: ActorId::agent("worker"),
        base_revision: 0,
        command: Command::Claim { work },
    })
    .expect("claim");
    let snapshot = second
        .open_with_registry_blocking(&registry)
        .expect("second")
        .plan_blocking()
        .expect("plan");
    assert_eq!(snapshot.revision, 1);
    assert_eq!(
        snapshot.work_items[&work].owner,
        Some(ActorId::agent("worker"))
    );
    assert!(
        second
            .open_with_registry_blocking(&registry)
            .expect("second")
            .execute_blocking(CommandRequest {
                actor: ActorId::agent("other"),
                base_revision: 0,
                command: Command::Claim { work }
            })
            .is_err()
    );
    assert_eq!(
        dpm_store::SqliteStore::open_existing_blocking(&database)
            .expect("store")
            .operation_count_blocking()
            .expect("ops"),
        1
    );
}

#[test]
fn replacement_is_explicit_and_mismatched_store_identity_is_rejected() {
    let temp = TempDir::new().expect("temp");
    let registry = WorkspaceRegistry::at(temp.path().join("bindings.sqlite"));
    let plan = fixture();
    let first = temp.path().join("first.sqlite");
    let second = temp.path().join("second.sqlite");
    Application::initialize_blocking(&first, &plan).expect("first");
    Application::initialize_blocking(&second, &plan).expect("second");
    registry.register_blocking(&first, false).expect("register");
    assert_eq!(
        registry
            .register_blocking(&second, false)
            .expect_err("conflict")
            .code(),
        "workspace_already_bound"
    );
    registry
        .register_blocking(&second, true)
        .expect("explicit replacement");
    assert_eq!(
        registry
            .resolve_blocking(plan.workspace.id)
            .expect("resolve"),
        fs::canonicalize(&second).expect("canonical")
    );
    let location = locator_blocking(&temp.path().join("entry"), plan.workspace.id);
    fs::remove_file(&second).expect("replace database contents");
    Application::initialize_blocking(&second, &Plan::empty("independent project"))
        .expect("different identity");
    assert!(location.open_with_registry_blocking(&registry).is_err());
}
