use super::*;
use dpm_model::Plan;

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

#[test]
fn recovery_failures_carry_stable_codes() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let app = Application::initialize_blocking(&live, &fixture()).expect("app");
    let report = app.backup_blocking(&backup).expect("backup");
    assert_eq!(report.operation_count, 0);
    let code = |error: AppError| error.code();
    assert_eq!(
        code(app.backup_blocking(&backup).expect_err("exists")),
        "target_exists"
    );
    assert_eq!(
        code(restore_store_blocking(&backup, &live).expect_err("live state")),
        "target_exists"
    );
    assert_eq!(
        code(restore_store_blocking(&backup, &dir.path().join("x.sqlite-wal")).expect_err("side")),
        "invalid_request"
    );
    rusqlite::Connection::open(&live)
        .expect("raw connection")
        .execute("UPDATE plan_state SET snapshot_json = '['", [])
        .expect("damage snapshot");
    let damaged = Application::open_blocking(&live)
        .and_then(|app| app.plan_blocking())
        .expect_err("damaged snapshot");
    assert_eq!(code(damaged), "corrupt_store");
    let truncated = dir.path().join("truncated.sqlite");
    let bytes = std::fs::read(&backup).expect("bytes");
    std::fs::write(&truncated, &bytes[..bytes.len() / 2]).expect("truncate");
    assert_eq!(
        code(verify_store_blocking(&truncated).expect_err("truncated")),
        "corrupt_store"
    );
    rusqlite::Connection::open(&backup)
        .expect("raw connection")
        .execute_batch("PRAGMA user_version = 99")
        .expect("future version");
    assert_eq!(
        code(verify_store_blocking(&backup).expect_err("future")),
        "unsupported_schema_version"
    );
    assert_eq!(
        code(Application::open_blocking(&backup).err().expect("future")),
        "unsupported_schema_version"
    );
}

#[test]
fn previews_have_no_store_to_back_up_or_verify() {
    let dir = tempfile::tempdir().expect("directory");
    let preview = Application::preview(fixture()).expect("preview");
    assert_eq!(
        preview
            .backup_blocking(&dir.path().join("backup.sqlite"))
            .expect_err("preview")
            .code(),
        "invalid_request"
    );
    let locator = dir.path().join(".dpm");
    std::fs::create_dir(&locator).expect("locator directory");
    std::fs::write(
        locator.join("project.toml"),
        format!(
            "version = 2\nworkspace = \"{}\"\npreview = \"plan.json\"\n",
            fixture().workspace.id
        ),
    )
    .expect("locator");
    assert!(matches!(
        store_path_blocking(dir.path(), None, None),
        Err(AppError::InvalidRequest(_))
    ));
}

#[test]
fn store_paths_resolve_like_the_adapters_without_opening_the_store() {
    let dir = tempfile::tempdir().expect("directory");
    let database = crate::initialize_project_blocking(dir.path(), &fixture()).expect("project");
    let resolved = store_path_blocking(dir.path(), None, None).expect("discovered");
    assert_eq!(
        std::fs::canonicalize(resolved).expect("resolved"),
        std::fs::canonicalize(&database).expect("database")
    );
    assert_eq!(
        store_path_blocking(dir.path(), None, Some(Path::new("other.sqlite"))).expect("explicit"),
        dir.path().join("other.sqlite")
    );
    assert!(!dir.path().join("other.sqlite").exists());
}
