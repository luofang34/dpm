use super::*;

fn tamper(path: &Path, sql: &str) {
    Connection::open(path)
        .expect("raw connection")
        .execute_batch(sql)
        .expect("tamper");
}

fn backup_of(dir: &Path, name: &str, operations: u64) -> PathBuf {
    let backup = dir.join(name);
    store_with_history(&dir.join(format!("live-{name}")), operations)
        .backup_blocking(&backup)
        .expect("backup");
    backup
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .expect("listing")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[test]
fn planted_triggers_views_and_indexes_are_refused_before_any_write() {
    let dir = tempfile::tempdir().expect("directory");
    for (name, ddl) in [
        (
            "trigger.sqlite",
            "CREATE TRIGGER wipe AFTER INSERT ON operations BEGIN \
             DELETE FROM operations WHERE sequence < NEW.sequence; END",
        ),
        (
            "view.sqlite",
            "CREATE VIEW recent AS SELECT * FROM operations",
        ),
        (
            "index.sqlite",
            "CREATE INDEX by_base ON operations(base_revision)",
        ),
    ] {
        let backup = backup_of(dir.path(), name, 2);
        tamper(&backup, ddl);
        let error = verify_store_blocking(&backup).expect_err("planted schema object");
        assert!(error.is_corruption(), "{name}: {error}");
        let before = std::fs::read(&backup).expect("bytes");
        assert!(
            SqliteStore::open_existing_blocking(&backup).is_err(),
            "{name}"
        );
        assert!(SqliteStore::open_blocking(&backup).is_err(), "{name}");
        assert_eq!(std::fs::read(&backup).expect("bytes"), before, "{name}");
        let target = dir.path().join(format!("restored-{name}"));
        assert!(restore_store_blocking(&backup, &target).is_err(), "{name}");
        assert!(!target.exists());
    }
}

#[test]
fn damaged_json_is_corruption_naming_the_file_and_record() {
    let dir = tempfile::tempdir().expect("directory");
    let command = backup_of(dir.path(), "command.sqlite", 3);
    tamper(
        &command,
        "UPDATE operations SET command_json = '{' WHERE sequence = 2",
    );
    let error = verify_store_blocking(&command).expect_err("damaged command");
    assert!(error.is_corruption(), "{error}");
    let message = error.to_string();
    assert!(
        message.contains("command.sqlite") && message.contains("sequence 2"),
        "{message}"
    );
    let snapshot = backup_of(dir.path(), "snapshot.sqlite", 1);
    tamper(
        &snapshot,
        "UPDATE plan_state SET snapshot_json = 'not json'",
    );
    for error in [
        verify_store_blocking(&snapshot).expect_err("damaged snapshot"),
        SqliteStore::open_existing_blocking(&snapshot)
            .expect("open")
            .load_blocking()
            .expect_err("load"),
    ] {
        assert!(error.is_corruption(), "{error}");
        let message = error.to_string();
        assert!(
            message.contains("snapshot.sqlite") && message.contains("snapshot snapshot_json"),
            "{message}"
        );
    }
}

#[test]
fn losing_the_start_of_history_fails_verification() {
    let dir = tempfile::tempdir().expect("directory");
    let backup = backup_of(dir.path(), "head.sqlite", 3);
    tamper(&backup, "DELETE FROM operations WHERE sequence = 1");
    let error = verify_store_blocking(&backup).expect_err("missing first operation");
    assert!(error.is_corruption(), "{error}");
    let emptied = backup_of(dir.path(), "emptied.sqlite", 2);
    tamper(&emptied, "DELETE FROM operations");
    let error = verify_store_blocking(&emptied).expect_err("missing every operation");
    assert!(error.is_corruption(), "{error}");
}

#[test]
fn side_file_names_are_refused_as_backup_and_restore_targets() {
    let dir = tempfile::tempdir().expect("directory");
    let store = store_with_history(&dir.path().join("live.sqlite"), 1);
    let backup = dir.path().join("backup.sqlite");
    store.backup_blocking(&backup).expect("backup");
    for name in ["live.sqlite-journal", "x.sqlite-WAL", "y-shm"] {
        let target = dir.path().join(name);
        assert!(store.backup_blocking(&target).is_err(), "backup to {name}");
        assert!(
            restore_store_blocking(&backup, &target).is_err(),
            "restore to {name}"
        );
        assert!(!target.exists(), "{name}");
    }
}

#[test]
fn a_refused_initialization_leaves_an_existing_store_byte_for_byte_unchanged() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    drop(store_with_history(&live, 2));
    let retired = dir.path().join("baseline.sqlite");
    std::fs::copy(backup_of(dir.path(), "source.sqlite", 2), &retired).expect("copy");
    tamper(&retired, "PRAGMA user_version = 2");
    for path in [&live, &retired] {
        let before = std::fs::read(path).expect("bytes");
        let refused = SqliteStore::open_blocking(path)
            .and_then(|mut store| store.initialize_blocking(&fixture()));
        assert!(
            matches!(
                refused,
                Err(StoreError::AlreadyInitialized(_) | StoreError::RetiredSchemaVersion { .. })
            ),
            "{path:?}: {refused:?}"
        );
        assert!(std::fs::read(path).expect("bytes") == before, "{path:?}");
    }
    assert_eq!(
        listing(dir.path())
            .into_iter()
            .filter(|name| name.starts_with("live") || name.starts_with("baseline"))
            .collect::<Vec<_>>(),
        ["baseline.sqlite", "live-source.sqlite", "live.sqlite"]
    );
}

#[cfg(unix)]
#[test]
fn verification_creates_no_files_and_works_in_a_read_only_directory() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().expect("directory");
    let sealed = dir.path().join("sealed");
    std::fs::create_dir(&sealed).expect("directory");
    let live = sealed.join("state.sqlite");
    drop(store_with_history(&live, 2));
    assert_eq!(journal_mode(&live), [[Value::Text("wal".into())]]);
    assert_eq!(listing(&sealed), ["state.sqlite"]);
    let restored = dir.path().join("restored.sqlite");
    restore_store_blocking(&live, &restored).expect("restore");
    assert_eq!(listing(&sealed), ["state.sqlite"], "restore source");
    assert_eq!(
        listing(dir.path()),
        ["restored.sqlite", "sealed"],
        "no stray side files next to the restored store"
    );
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o555)).expect("seal");
    let result = verify_store_blocking(&live);
    let after = listing(&sealed);
    std::fs::set_permissions(&sealed, std::fs::Permissions::from_mode(0o755)).expect("unseal");
    assert_eq!(
        result
            .expect("verify in read-only directory")
            .operation_count,
        2
    );
    assert_eq!(after, ["state.sqlite"]);
}

#[test]
fn reports_name_canonical_absolute_paths() {
    let dir = tempfile::tempdir().expect("directory");
    std::fs::create_dir(dir.path().join("nested")).expect("directory");
    let store = store_with_history(&dir.path().join("live.sqlite"), 1);
    let indirect = dir.path().join("nested/../backup.sqlite");
    let canonical = |path: &Path| std::fs::canonicalize(path).expect("canonical");
    let report = store.backup_blocking(&indirect).expect("backup");
    assert_eq!(report.path, canonical(&indirect));
    let restored = dir.path().join("nested/./restored.sqlite");
    let report = restore_store_blocking(&indirect, &restored).expect("restore");
    assert_eq!(report.path, canonical(&restored));
    let report = verify_store_blocking(&restored).expect("verify");
    assert_eq!(report.path, canonical(&restored));
}

#[test]
fn a_trigger_planted_after_open_is_refused_inside_the_write_transaction() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    let mut store = store_with_history(&live, 2);
    let mut plan = store.load_blocking().expect("load").expect("plan");
    tamper(
        &live,
        "CREATE TRIGGER wipe AFTER INSERT ON operations BEGIN \
         DELETE FROM operations WHERE sequence < NEW.sequence; END",
    );
    let command = next_command(&plan, 2);
    let operation = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        command,
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("command");
    let refused = store
        .persist_blocking(&plan, &operation)
        .expect_err("refused write");
    assert!(
        matches!(refused, StoreError::UnrecognizedSchema { .. }),
        "{refused}"
    );
    assert_eq!(
        rows(&live, "SELECT COUNT(*) FROM operations"),
        [[Value::Integer(2)]]
    );
}
