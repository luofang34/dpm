//! A store written by an earlier build must keep replaying: an engine, diff or validation change
//! that makes recorded operations refuse or diverge fails here instead of in a user's workspace.
//! Regenerate the fixture only for a deliberate format change (`scripts/golden_store.py`).

use super::*;

const GOLDEN: &str = include_str!("../../../../../../tests/support/golden-v3-store.sql");

#[test]
fn the_checked_in_store_still_replays_to_its_snapshot() {
    let dir = tempfile::tempdir().expect("directory");
    let path = dir.path().join("golden.sqlite");
    Connection::open(&path)
        .expect("raw connection")
        .execute_batch(GOLDEN)
        .expect("load fixture");
    let report = verify_store_blocking(&path).expect("golden store replays");
    assert_eq!(report.schema_version, crate::SCHEMA_VERSION);
    assert!(report.operation_count >= 10, "{report:?}");
    let store = SqliteStore::open_existing_blocking(&path).expect("open");
    let history = store.history_blocking(0, 1000).expect("history");
    let kinds: Vec<_> = history
        .entries
        .iter()
        .map(|entry| format!("{:?}", entry.operation.command))
        .collect();
    assert!(
        kinds.iter().any(|kind| kind.starts_with("ApplyChange")),
        "{kinds:?}"
    );
    assert!(
        kinds.iter().any(|kind| kind.starts_with("Verify")),
        "{kinds:?}"
    );
}
