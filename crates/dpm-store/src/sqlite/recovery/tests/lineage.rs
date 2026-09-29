//! A backup archives its source's lineage; a restore starts a new writable one.

use super::*;

#[test]
fn backups_are_archives_and_restores_start_a_new_lineage() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    let backup = dir.path().join("backup.sqlite");
    let restored = dir.path().join("restored.sqlite");
    let store = store_with_history(&live, 2);
    let source = store
        .lineage_blocking()
        .expect("lineage")
        .expect("initialized");
    let archived = store.backup_blocking(&backup).expect("backup");
    assert_eq!(
        (archived.lineage_id, archived.archived),
        (source.lineage_id, true)
    );
    let report = restore_store_blocking(&backup, &restored).expect("restore");
    assert_ne!(report.lineage_id, source.lineage_id);
    assert!(!report.archived);
    assert_eq!(verify_store_blocking(&backup).expect("verify"), archived);
    assert_eq!(
        store.lineage_blocking().expect("lineage"),
        Some(source),
        "the source keeps its lineage"
    );

    let mut archive = SqliteStore::open_existing_blocking(&backup).expect("open archive");
    let before = std::fs::read(&backup).expect("bytes");
    let mut plan = archive.load_blocking().expect("load").expect("plan");
    let command = next_command(&plan, 2);
    let operation = apply_command(
        &mut plan,
        ActorId::agent("owner"),
        command,
        Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("command");
    assert!(matches!(
        archive.persist_blocking(&plan, &operation, None),
        Err(StoreError::Lineage(crate::LineageError::Archived { .. }))
    ));
    drop(archive);
    assert!(std::fs::read(&backup).expect("bytes") == before);

    let mut writable = SqliteStore::open_existing_blocking(&restored).expect("open restored");
    append(&mut writable, 2);
    let history = writable.history_blocking(0, 10).expect("history");
    assert_eq!(history.lineage_id, Some(report.lineage_id));
    let lineages: Vec<_> = history
        .entries
        .iter()
        .map(|entry| entry.operation.lineage_id)
        .collect();
    assert_eq!(
        lineages,
        [source.lineage_id, source.lineage_id, report.lineage_id],
        "copied operations keep the lineage that recorded them"
    );
}

#[test]
fn a_store_without_its_lineage_row_is_corrupt() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("live.sqlite");
    drop(store_with_history(&path, 1));
    Connection::open(&path)
        .expect("raw connection")
        .execute("DELETE FROM store_lineage", [])
        .expect("tamper");
    let refused = verify_store_blocking(&path).expect_err("no lineage");
    assert!(
        matches!(
            refused,
            StoreError::Lineage(crate::LineageError::Missing { .. })
        ) && refused.is_corruption(),
        "{refused:?}"
    );
    assert!(matches!(
        SqliteStore::open_existing_blocking(&path).err(),
        Some(StoreError::Lineage(crate::LineageError::Missing { .. }))
    ));
}

#[test]
fn a_live_store_is_never_a_restore_source() {
    let dir = tempfile::tempdir().expect("directory");
    let live = dir.path().join("live.sqlite");
    let restored = dir.path().join("restored.sqlite");
    drop(store_with_history(&live, 2));
    let before = std::fs::read(&live).expect("bytes");
    assert!(matches!(
        restore_store_blocking(&live, &restored),
        Err(StoreError::Lineage(
            crate::LineageError::NotAnArchive { .. }
        ))
    ));
    assert!(!restored.exists(), "no partial copy is left behind");
    assert!(std::fs::read(&live).expect("bytes") == before);
}
