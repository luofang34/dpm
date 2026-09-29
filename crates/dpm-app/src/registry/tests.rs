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
        format!("version = 3\nworkspace = '{workspace}'\n"),
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
        base_lineage: None,
        operation_id: None,
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
        snapshot.work_items[&work].execution.owner,
        Some(ActorId::agent("worker"))
    );
    assert!(
        second
            .open_with_registry_blocking(&registry)
            .expect("second")
            .execute_blocking(CommandRequest {
                actor: ActorId::agent("other"),
                base_revision: 0,
                base_lineage: None,
                operation_id: None,
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
    assert_eq!(
        code(location.open_with_registry_blocking(&registry)),
        "workspace_identity_mismatch"
    );
}

fn identities(app: &Application) -> (WorkspaceId, Vec<dpm_model::WorkItemId>, Vec<String>) {
    let plan = app.plan_blocking().expect("plan");
    (
        plan.workspace.id,
        plan.work_items.keys().copied().collect(),
        plan.assets.keys().map(ToString::to_string).collect(),
    )
}

#[test]
fn moving_a_checkout_or_renaming_its_repository_keeps_every_identity() {
    let temp = TempDir::new().expect("temp");
    let registry = WorkspaceRegistry::at(temp.path().join("bindings.sqlite"));
    let plan = fixture();
    let database = temp.path().join("shared.sqlite");
    Application::initialize_blocking(&database, &plan).expect("store");
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
    let mut app = ProjectLocation::at_blocking(&checkout)
        .expect("location")
        .open_with_registry_blocking(&registry)
        .expect("open");
    let asset = app.project_asset.expect("bound asset");
    let before = identities(&app);
    let mut renamed = app.plan_blocking().expect("plan");
    if let Some(repository) = renamed.assets.get_mut(&asset) {
        repository.label = "Renamed repository".into();
        repository.kind = dpm_model::AssetKind::GitRepository {
            remotes: vec!["https://example.invalid/renamed.git".into()],
        };
    }
    app.apply_plan_change_blocking(crate::PlanChangeRequest {
        actor: ActorId::human("maintainer"),
        base_revision: 0,
        base_lineage: None,
        operation_id: None,
        plan: Box::new(renamed),
        reason: "repository renamed upstream".into(),
    })
    .expect("reviewed rename");
    let moved = temp.path().join("elsewhere/moved-checkout");
    fs::create_dir_all(moved.parent().expect("parent")).expect("parent");
    fs::rename(&checkout, &moved).expect("move checkout");
    assert!(ProjectLocation::at_blocking(&checkout).is_err());
    let reopened = ProjectLocation::at_blocking(&moved)
        .expect("moved location")
        .open_with_registry_blocking(&registry)
        .expect("reopen");
    assert_eq!(identities(&reopened), before);
    assert_eq!(reopened.project_asset, Some(asset));
    assert_eq!(reopened.plan_blocking().expect("plan").revision, 1);
    assert_eq!(
        registry.list_blocking().expect("bindings").len(),
        1,
        "moving a checkout must not create another workspace binding"
    );
}

fn code<T>(result: Result<T, AppError>) -> &'static str {
    match result {
        Ok(_) => panic!("expected a refusal"),
        Err(error) => error.code(),
    }
}

#[test]
fn an_occupied_path_is_never_bound_to_a_second_identity_without_replace() {
    let temp = TempDir::new().expect("temp");
    let registry = WorkspaceRegistry::at(temp.path().join("bindings.sqlite"));
    let plan = fixture();
    let path = temp.path().join("store.sqlite");
    Application::initialize_blocking(&path, &plan).expect("first");
    registry.register_blocking(&path, false).expect("register");
    fs::remove_file(&path).expect("remove");
    let other = Plan::empty("independent project");
    Application::initialize_blocking(&path, &other).expect("other identity at the same path");
    assert_eq!(
        code(registry.register_blocking(&path, false)),
        "workspace_path_bound"
    );
    let listed = registry.list_blocking().expect("list");
    assert_eq!(listed.len(), 1, "a refused registration adds no binding");
    assert_eq!(listed[0].workspace, plan.workspace.id);
    registry
        .register_blocking(&path, true)
        .expect("explicit rebinding of the path");
    let listed = registry.inspect_blocking().expect("inspect");
    assert_eq!(listed.len(), 1, "the stale identity at the path is removed");
    assert_eq!(listed[0].binding.workspace, other.workspace.id);
    assert_eq!(listed[0].store, StoreState::Ok);
    assert!(listed[0].shared_with.is_empty());
}

#[test]
fn inspection_and_opening_name_missing_changed_and_shared_bindings() {
    let temp = TempDir::new().expect("temp");
    let registry = WorkspaceRegistry::at(temp.path().join("bindings.sqlite"));
    let plan = fixture();
    let path = temp.path().join("store.sqlite");
    Application::initialize_blocking(&path, &plan).expect("store");
    registry.register_blocking(&path, false).expect("register");
    let location = locator_blocking(&temp.path().join("entry"), plan.workspace.id);
    fs::remove_file(&path).expect("remove");
    assert_eq!(
        registry.inspect_blocking().expect("inspect")[0].store,
        StoreState::Missing
    );
    assert_eq!(
        code(location.open_with_registry_blocking(&registry)),
        "workspace_store_missing"
    );
    let other = Plan::empty("independent project");
    Application::initialize_blocking(&path, &other).expect("replacement contents");
    assert_eq!(
        registry.inspect_blocking().expect("inspect")[0].store,
        StoreState::IdentityMismatch {
            found: other.workspace.id
        }
    );
    assert_eq!(
        code(location.open_with_registry_blocking(&registry)),
        "workspace_identity_mismatch"
    );
    // Registries written without the exclusive-path check can hold two identities for one path.
    let canonical = fs::canonicalize(&path).expect("canonical");
    Connection::open(temp.path().join("bindings.sqlite"))
        .expect("registry")
        .execute(
            "INSERT INTO bindings VALUES (?1, ?2)",
            params![
                other.workspace.id.to_string(),
                canonical.to_str().expect("utf-8")
            ],
        )
        .expect("legacy duplicate");
    let reports = registry.inspect_blocking().expect("inspect");
    let stale = reports
        .iter()
        .find(|r| r.binding.workspace == plan.workspace.id)
        .expect("stale");
    let current = reports
        .iter()
        .find(|r| r.binding.workspace == other.workspace.id)
        .expect("current");
    assert_eq!(stale.shared_with, vec![other.workspace.id]);
    assert_eq!(current.shared_with, vec![plan.workspace.id]);
    assert_eq!(current.store, StoreState::Ok);
    registry
        .register_blocking(&path, true)
        .expect("replace clears the stale identity");
    assert_eq!(registry.list_blocking().expect("list").len(), 1);
}
