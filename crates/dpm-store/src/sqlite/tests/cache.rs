use super::*;

#[test]
fn cached_reads_see_external_writes_and_do_not_trust_revision_alone() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("plan.sqlite");
    let mut first = SqliteStore::open_blocking(&path).expect("store");
    let original = fixture();
    first.initialize_blocking(&original).expect("initialize");
    let mut returned = first.load_blocking().expect("load").expect("plan");
    returned.workspace.name = "caller edit".into();
    assert_eq!(
        first.load_blocking().expect("cached"),
        Some(original.clone())
    );
    let mut other = SqliteStore::open_existing_blocking(&path).expect("other");
    let mut updated = original.clone();
    let operation = claim(&mut updated, "other");
    other
        .persist_blocking(&updated, &operation, None)
        .expect("commit");
    assert_eq!(
        first.load_blocking().expect("refresh"),
        Some(updated.clone())
    );
    let connection = Connection::open(&path).expect("external");
    connection
        .execute("UPDATE plan_state SET snapshot_json = '{}'", [])
        .expect("corrupt");
    assert!(
        first.load_blocking().is_err(),
        "same revision must not hide corrupted bytes"
    );
    connection
        .execute(
            "UPDATE plan_state SET snapshot_json = ?1, revision = 2",
            [serde_json::to_string(&updated).expect("json")],
        )
        .expect("row mismatch");
    assert!(matches!(
        first.load_blocking(),
        Err(StoreError::CorruptSnapshot { .. })
    ));
    connection
        .execute("UPDATE plan_state SET revision = 1", [])
        .expect("repair row");
    assert_eq!(first.load_blocking().expect("repaired"), Some(updated));
}

#[test]
fn the_revision_query_reads_rows_without_decoding_the_snapshot() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("plan.sqlite");
    let mut store = SqliteStore::open_blocking(&path).expect("store");
    assert_eq!(store.revision_blocking().expect("empty"), None);
    let mut plan = fixture();
    store.initialize_blocking(&plan).expect("initialize");
    let lineage = store.lineage_blocking().expect("lineage").expect("row");
    let initial = store.revision_blocking().expect("revision").expect("row");
    assert_eq!(
        (initial.revision, initial.lineage),
        (plan.revision, lineage)
    );
    let operation = claim(&mut plan, "owner");
    store
        .persist_blocking(&plan, &operation, None)
        .expect("commit");
    let next = store.revision_blocking().expect("revision").expect("row");
    assert_eq!(next.revision, initial.revision.wrapping_add(1));
    // Damaged snapshot text would fail any decode; the revision row alone still answers.
    Connection::open(&path)
        .expect("external")
        .execute("UPDATE plan_state SET snapshot_json = '{}'", [])
        .expect("damage");
    assert_eq!(store.revision_blocking().expect("revision"), Some(next));
    assert!(store.load_blocking().is_err());
}
