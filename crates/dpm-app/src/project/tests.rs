#![allow(clippy::expect_used)]
use super::*;
use crate::{CommandRequest, Query};
use dpm_engine::Command;
use dpm_model::ActorId;
use tempfile::TempDir;

fn plan() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("synthetic plan")
}

fn locator_blocking(root: &Path, content: &str) {
    fs::create_dir_all(root.join(".dpm")).expect("directory");
    fs::write(root.join(".dpm/project.toml"), content).expect("locator");
}

#[test]
fn nearest_project_and_git_boundaries_prevent_cross_project_selection() {
    let temp = TempDir::new().expect("temp");
    let outer = temp.path();
    locator_blocking(outer, "version = 1\ndatabase = 'outer.sqlite'");
    let nested = outer.join("nested");
    locator_blocking(&nested, "version = 1\ndatabase = 'inner.sqlite'");
    let child = nested.join("src/feature");
    fs::create_dir_all(&child).expect("child");
    let found = ProjectLocation::discover_blocking(&child).expect("discovery");
    assert_eq!(found.root, fs::canonicalize(&nested).expect("canonical"));
    assert_eq!(
        found.source,
        ProjectSource::Database(found.root.join(".dpm/inner.sqlite"))
    );
    let independent = outer.join("independent");
    fs::create_dir(&independent).expect("repo");
    // Worktree Git boundaries are files, not only .git directories.
    fs::write(independent.join(".git"), "gitdir: elsewhere").expect("git boundary");
    assert!(matches!(
        ProjectLocation::discover_blocking(&independent),
        Err(ProjectError::NotFound { .. })
    ));
    locator_blocking(&independent, "version = 1\ndatabase = 'own.sqlite'");
    assert_eq!(
        ProjectLocation::discover_blocking(&independent)
            .expect("own locator")
            .root,
        fs::canonicalize(independent).expect("canonical")
    );
}

#[test]
fn invalid_nearest_locator_never_falls_back_to_parent() {
    let temp = TempDir::new().expect("temp");
    locator_blocking(temp.path(), "version = 1\ndatabase = 'outer.sqlite'");
    let child = temp.path().join("child");
    for content in [
        "not toml",
        "version = 2\ndatabase = 'state.sqlite'",
        "version = 1",
        "version = 1\ndatabase = ''",
        "version = 1\ndatabase = 'a'\npreview = 'b'",
        "version = 1\ndatabase = 'a'\nunknown = true",
    ] {
        locator_blocking(&child, content);
        assert!(
            ProjectLocation::discover_blocking(&child).is_err(),
            "{content}"
        );
    }
    fs::remove_file(child.join(".dpm/project.toml")).expect("remove locator");
    assert!(ProjectLocation::discover_blocking(&child).is_err());
}

#[test]
fn initialization_is_explicit_and_preserves_existing_state_and_history() {
    let temp = TempDir::new().expect("temp");
    let root = temp.path();
    assert!(open_workspace_blocking(root, None, None).is_err());
    assert!(!root.join(".dpm").exists());
    let expected = plan();
    let database = initialize_project_blocking(root, &expected).expect("initialize");
    let mut app = open_workspace_blocking(root, None, None).expect("open");
    let work = app.work_id_blocking("TEST-A").expect("task");
    app.execute_blocking(CommandRequest {
        actor: ActorId::agent("tester"),
        base_revision: 0,
        command: Command::Claim { work },
    })
    .expect("persist claim");
    let current = app.plan_blocking().expect("snapshot");
    assert!(initialize_project_blocking(root, &expected).is_err());
    assert_eq!(
        Application::open_blocking(&database)
            .expect("reopen")
            .plan_blocking()
            .expect("snapshot"),
        current
    );
    assert_eq!(
        dpm_store::SqliteStore::open_existing_blocking(database)
            .expect("store")
            .operation_count_blocking()
            .expect("ops"),
        1
    );
    assert_eq!(
        fs::read_to_string(root.join(".dpm/.gitignore")).expect("ignore"),
        "*\n!project.toml\n!.gitignore\n"
    );
}

#[test]
fn preview_reads_are_identical_and_all_mutations_are_rejected_without_files() {
    let temp = TempDir::new().expect("temp");
    let expected = plan();
    let path = temp.path().join("plan.json");
    let original = serde_json::to_string(&expected).expect("json");
    fs::write(&path, &original).expect("plan");
    locator_blocking(temp.path(), "version = 1\npreview = '../plan.json'");
    let mut app = open_workspace_blocking(temp.path(), None, None).expect("preview");
    assert!(app.is_read_only());
    let database = Application::in_memory_blocking(&expected).expect("equivalent db");
    let query = Query::Show {
        key: "TEST-A".into(),
    };
    assert_eq!(
        app.query_blocking(query.clone())
            .expect("preview query")
            .data,
        database.query_blocking(query).expect("db query").data
    );
    let work = app.work_id_blocking("TEST-A").expect("task");
    let error = app
        .execute_blocking(CommandRequest {
            actor: ActorId::agent("tester"),
            base_revision: 0,
            command: Command::Claim { work },
        })
        .expect_err("preview must reject even eligible work");
    assert_eq!(error.code(), "read_only_project");
    assert_eq!(app.plan_blocking().expect("snapshot"), expected);
    assert_eq!(fs::read_to_string(path).expect("unchanged file"), original);
    assert_eq!(
        fs::read_dir(temp.path().join(".dpm"))
            .expect("listing")
            .count(),
        1
    );
}

#[test]
fn explicit_database_overrides_discovery_and_project_is_exact() {
    let temp = TempDir::new().expect("temp");
    locator_blocking(temp.path(), "invalid = true");
    let database = temp.path().join("selected.sqlite");
    let expected = plan();
    Application::initialize_blocking(&database, &expected).expect("database");
    assert_eq!(
        open_workspace_blocking(temp.path(), None, Some(&database))
            .expect("override")
            .plan_blocking()
            .expect("plan"),
        expected
    );
    let child = temp.path().join("child");
    fs::create_dir(&child).expect("child");
    assert!(open_workspace_blocking(temp.path(), Some(&child), None).is_err());
    assert!(open_workspace_blocking(temp.path(), Some(temp.path()), Some(&database)).is_err());
}

#[test]
fn legacy_database_is_preserved_and_requires_explicit_selection() {
    let temp = TempDir::new().expect("temp");
    let database = temp.path().join(".dagplan/dagplan.sqlite");
    let expected = plan();
    Application::initialize_blocking(&database, &expected).expect("legacy database");
    assert!(matches!(
        ProjectLocation::discover_blocking(temp.path()),
        Err(ProjectError::Legacy { .. })
    ));
    assert!(initialize_project_blocking(temp.path(), &expected).is_err());
    assert!(!temp.path().join(".dpm").exists());
    assert_eq!(
        open_workspace_blocking(temp.path(), None, Some(&database))
            .expect("explicit old path")
            .plan_blocking()
            .expect("plan"),
        expected
    );
}

#[test]
fn invalid_plan_creates_no_project_and_missing_sources_are_not_initialized() {
    let temp = TempDir::new().expect("temp");
    let mut invalid = plan();
    invalid
        .work_items
        .values_mut()
        .find(|w| w.is_executable())
        .expect("task")
        .acceptance
        .clear();
    assert!(initialize_project_blocking(temp.path(), &invalid).is_err());
    assert!(!temp.path().join(".dpm").exists());
    for source in ["database", "preview"] {
        locator_blocking(temp.path(), &format!("version = 1\n{source} = 'missing'"));
        assert!(open_workspace_blocking(temp.path(), None, None).is_err());
        assert!(!temp.path().join(".dpm/missing").exists());
    }
}

#[test]
fn a_cloned_database_locator_can_be_initialized_but_preview_cannot() {
    let temp = TempDir::new().expect("temp");
    let config = "version = 1\ndatabase = 'state.sqlite'";
    locator_blocking(temp.path(), config);
    let expected = plan();
    initialize_project_blocking(temp.path(), &expected).expect("initialize cloned project");
    assert_eq!(
        fs::read_to_string(temp.path().join(".dpm/project.toml")).expect("config"),
        config
    );
    assert_eq!(
        open_workspace_blocking(temp.path(), None, None)
            .expect("open")
            .plan_blocking()
            .expect("plan"),
        expected
    );
    locator_blocking(temp.path(), "version = 1\npreview = '../plan.json'");
    assert_eq!(
        initialize_project_blocking(temp.path(), &expected)
            .expect_err("preview")
            .code(),
        "read_only_project"
    );
}
