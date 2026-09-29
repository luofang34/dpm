use crate::{Application, CommandRequest, open_workspace_blocking};
use dpm_engine::Command;
use dpm_model::{ActorId, Plan};
use std::{fs, path::Path};
use tempfile::TempDir;

fn plan() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("synthetic plan")
}

fn store_blocking(path: &Path, plan: &Plan) {
    dpm_store::SqliteStore::open_blocking(path)
        .expect("store")
        .initialize_blocking(plan)
        .expect("initialize");
}

fn locate_blocking(root: &Path, plan: &Plan, database: &str) {
    fs::write(
        root.join(".dpm/project.toml"),
        format!(
            "version = 3\nworkspace = '{}'\ndatabase = '{database}'",
            plan.workspace.id
        ),
    )
    .expect("locator");
}

fn claim_blocking(app: &mut Application, plan: &Plan) {
    let work = plan.find_work_by_key("TEST-A").expect("task").id;
    app.execute_blocking(CommandRequest {
        actor: ActorId::agent("writer"),
        base_revision: 0,
        base_lineage: None,
        operation_id: None,
        command: Command::Claim { work },
    })
    .expect("claim");
}

#[test]
fn a_repointed_locator_reports_the_other_lineage_before_and_after_reload() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path();
    let plan = plan();
    fs::create_dir_all(root.join(".dpm")).expect("directory");
    store_blocking(&root.join(".dpm/first.sqlite"), &plan);
    store_blocking(&root.join(".dpm/second.sqlite"), &plan);
    locate_blocking(root, &plan, "first.sqlite");
    let console = open_workspace_blocking(root, None, None).expect("console");
    let shown = console.refreshed_revision_blocking().expect("probe");
    assert_eq!(shown, console.revision_blocking().expect("revision"));

    // Another application commits to the same store; the lineage stays and the revision advances.
    let mut writer = open_workspace_blocking(root, None, None).expect("writer");
    claim_blocking(&mut writer, &plan);
    let advanced = console.refreshed_revision_blocking().expect("probe");
    assert_eq!(advanced.revision, shown.revision + 1);
    assert_eq!(advanced.lineage_id, shown.lineage_id);
    let snapshot = console.refreshed_snapshot_blocking().expect("reload");
    assert_eq!(
        (snapshot.revision, snapshot.lineage_id),
        (advanced.revision, advanced.lineage_id)
    );
    assert_eq!(snapshot.data.revision, snapshot.revision);

    locate_blocking(root, &plan, "second.sqlite");
    let other = console.refreshed_revision_blocking().expect("probe");
    assert_eq!(other.revision, shown.revision);
    assert_ne!(other.lineage_id, shown.lineage_id);
    // Only the probe follows the locator; the connection opened at startup keeps the first store.
    assert_eq!(console.revision_blocking().expect("revision"), advanced);
    let reloaded = console.refreshed_snapshot_blocking().expect("reload");
    assert_eq!(
        (reloaded.revision, reloaded.lineage_id),
        (other.revision, other.lineage_id)
    );
}

#[test]
fn an_explicit_database_is_probed_through_its_own_connection() {
    let temp = TempDir::new().expect("temp");
    let path = temp.path().join("state.sqlite");
    let plan = plan();
    store_blocking(&path, &plan);
    let console = Application::open_blocking(&path).expect("console");
    let mut writer = Application::open_blocking(&path).expect("writer");
    claim_blocking(&mut writer, &plan);
    let probed = console.refreshed_revision_blocking().expect("probe");
    assert_eq!(probed.revision, 1);
    let snapshot = console.refreshed_snapshot_blocking().expect("reload");
    assert_eq!(snapshot.revision, 1);
    assert_eq!(snapshot.lineage_id, probed.lineage_id);
}
