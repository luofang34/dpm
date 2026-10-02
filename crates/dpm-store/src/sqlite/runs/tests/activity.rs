//! Bounded activity, retention, links and restart.

use super::*;

#[test]
fn activity_is_recorded_without_touching_the_lifecycle() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let before = fixture
        .store
        .lifecycle_page_blocking(0, 100, None)
        .expect("page");
    for (index, kind) in [
        ActivityKind::ToolStarted,
        ActivityKind::ToolResult,
        ActivityKind::InputRequested,
        ActivityKind::Heartbeat,
    ]
    .into_iter()
    .enumerate()
    {
        fixture
            .record_activity(run, index as u64 + 1, kind)
            .expect("activity");
    }
    let after = fixture
        .store
        .lifecycle_page_blocking(0, 100, None)
        .expect("page");
    assert_eq!(before, after, "activity changes no lifecycle fact");
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(snapshot.last.state, RunState::Working);
    assert_eq!(
        (snapshot.activity.recorded, snapshot.activity.retained),
        (4, 4)
    );
    let latest = snapshot.activity.latest.expect("latest");
    assert_eq!(
        (latest.kind, latest.source_sequence),
        (ActivityKind::Heartbeat, 4)
    );
    let page = fixture
        .store
        .activity_page_blocking(0, 100, Some(run))
        .expect("page");
    assert_eq!(page.entries.len(), 4);
    assert_eq!(page.gap, None);
}

#[test]
fn duplicate_delivery_is_answered_and_a_changed_payload_refuses_the_whole_batch() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let heartbeat = ActivityInput {
        run,
        source_sequence: 1,
        kind: ActivityKind::Heartbeat,
        text: Some("alive".into()),
        observed_at: None,
    };
    let now = Utc::now();
    let first = fixture
        .store
        .append_activity_blocking(
            std::slice::from_ref(&heartbeat),
            &worker(),
            now,
            fixture.lineage,
        )
        .expect("first");
    let again = fixture
        .store
        .append_activity_blocking(
            std::slice::from_ref(&heartbeat),
            &worker(),
            now + TimeDelta::seconds(9),
            fixture.lineage,
        )
        .expect("duplicate");
    assert!(again[0].replayed);
    assert_eq!(
        again[0].value, first[0].value,
        "the first receipt time stands"
    );
    let fresh = ActivityInput {
        source_sequence: 2,
        ..heartbeat.clone()
    };
    let conflicting = ActivityInput {
        text: Some("different".into()),
        ..heartbeat
    };
    assert!(matches!(
        fixture.store.append_activity_blocking(
            &[fresh, conflicting],
            &worker(),
            now,
            fixture.lineage
        ),
        Err(RunStoreError::DuplicateActivity { .. })
    ));
    let page = fixture
        .store
        .activity_page_blocking(0, 100, None)
        .expect("page");
    assert_eq!(
        page.entries.len(),
        1,
        "the new record of the refused batch was rolled back"
    );
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(snapshot.activity.recorded, 1);
}

#[test]
fn activity_batches_are_bounded_and_need_a_known_authorized_run() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    assert!(matches!(
        fixture
            .store
            .append_activity_blocking(&[], &worker(), Utc::now(), fixture.lineage),
        Err(RunStoreError::Run(RunError::Validation(_)))
    ));
    let many: Vec<_> = (0..=dpm_model::MAX_ACTIVITY_BATCH)
        .map(|index| ActivityInput {
            run,
            source_sequence: index as u64 + 1,
            kind: ActivityKind::Heartbeat,
            text: None,
            observed_at: None,
        })
        .collect();
    assert!(
        fixture
            .store
            .append_activity_blocking(&many, &worker(), Utc::now(), fixture.lineage)
            .is_err()
    );
    assert!(matches!(
        fixture.record_activity(RunId::new(), 1, ActivityKind::Heartbeat),
        Err(RunStoreError::UnknownRun(_))
    ));
    let input = ActivityInput {
        run,
        source_sequence: 1,
        kind: ActivityKind::Heartbeat,
        text: None,
        observed_at: None,
    };
    assert!(matches!(
        fixture.store.append_activity_blocking(
            &[input],
            &ActorId::agent("rival"),
            Utc::now(),
            fixture.lineage
        ),
        Err(RunStoreError::Run(RunError::ActorNotAllowed { .. }))
    ));
    let page = fixture
        .store
        .activity_page_blocking(0, 100, None)
        .expect("page");
    assert!(page.entries.is_empty() && page.head_sequence == 0);
}

#[test]
fn retention_bounds_activity_and_reports_a_gap_to_a_cursor_it_outran() {
    let mut fixture = fixture();
    fixture.store.set_activity_limit(3);
    let run = fixture.start().id;
    for index in 1..=5 {
        fixture
            .record_activity(run, index, ActivityKind::Progress)
            .expect("activity");
    }
    let page = fixture
        .store
        .activity_page_blocking(0, 100, None)
        .expect("page");
    let sequences: Vec<_> = page.entries.iter().map(|entry| entry.sequence).collect();
    assert_eq!(sequences, [3, 4, 5], "the newest three survive");
    assert_eq!(page.head_sequence, 5);
    let gap = page.gap.expect("a cursor at zero outran retention");
    assert_eq!((gap.requested_after, gap.resumes_at, gap.lost), (0, 3, 2));
    assert_eq!(page.next_after_sequence, 5);
    // A client that kept up sees a continuous feed.
    let caught_up = fixture
        .store
        .activity_page_blocking(2, 100, None)
        .expect("page");
    assert_eq!(caught_up.gap, None);
    assert_eq!(caught_up.entries.len(), 3);
    // Sequences are never reused: the next record continues from the head.
    let next = fixture
        .record_activity(run, 6, ActivityKind::Progress)
        .expect("activity");
    assert_eq!(next[0].value.sequence, 6);
    let tail = fixture
        .store
        .activity_page_blocking(5, 100, None)
        .expect("page");
    assert_eq!(tail.gap, None);
    assert_eq!(tail.entries.len(), 1);
    // Retention removed records, not the knowledge that the run reported them.
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(
        (snapshot.activity.recorded, snapshot.activity.retained),
        (6, 3)
    );
    assert_eq!(snapshot.activity.latest.expect("latest").source_sequence, 6);
    assert_eq!(snapshot.activity.source_high_water, 6);
    // Lifecycle is never pruned.
    let lifecycle = fixture
        .store
        .lifecycle_page_blocking(0, 100, None)
        .expect("page");
    assert_eq!(lifecycle.entries.len(), 1);
}

#[test]
fn feed_pages_are_resumable_and_bounded_by_the_limit() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    for index in 1..=5 {
        fixture
            .record_activity(run, index, ActivityKind::Progress)
            .expect("activity");
    }
    let mut cursor = 0;
    let mut seen = Vec::new();
    loop {
        let page = fixture
            .store
            .activity_page_blocking(cursor, 2, None)
            .expect("page");
        assert!(page.entries.len() <= 2);
        if page.entries.is_empty() {
            assert_eq!(
                page.next_after_sequence, cursor,
                "an empty page keeps the cursor"
            );
            break;
        }
        seen.extend(page.entries.iter().map(|entry| entry.sequence));
        cursor = page.next_after_sequence;
    }
    assert_eq!(seen, [1, 2, 3, 4, 5]);
}

#[test]
fn a_run_store_refuses_writes_for_another_lineage_and_for_an_archive() {
    let mut fixture = fixture();
    let record = record(&fixture.plan, fixture.lineage, &request(fixture.work));
    let other = LineageId::new();
    assert!(matches!(
        fixture.store.start_blocking(&record, other),
        Err(RunStoreError::Store(StoreError::Lineage(
            LineageError::Mismatch { .. }
        )))
    ));
    assert!(
        fixture
            .store
            .snapshot_blocking(record.id)
            .expect("read")
            .is_none(),
        "the refused start wrote nothing"
    );
    fixture
        .store
        .connection
        .execute("UPDATE run_binding SET archived = 1", [])
        .expect("archive");
    assert!(matches!(
        fixture.store.start_blocking(&record, fixture.lineage),
        Err(RunStoreError::Store(StoreError::Lineage(
            LineageError::Archived { .. }
        )))
    ));
}

#[test]
fn an_operation_links_to_one_run_and_only_when_it_is_the_runs_own() {
    let mut fixture = fixture();
    let first = fixture.start();
    let second = fixture.start();
    let operation = OperationFacts {
        id: OperationId::new(),
        actor: worker(),
        timestamp: first.started_at + TimeDelta::seconds(1),
        work: Some(fixture.work),
        workspace: fixture.plan.workspace.id,
        lineage: fixture.lineage,
    };
    let now = Utc::now();
    let linked = fixture
        .store
        .link_blocking(first.id, &operation, &worker(), now, fixture.lineage)
        .expect("link");
    assert!(!linked.replayed);
    let again = fixture
        .store
        .link_blocking(first.id, &operation, &worker(), now, fixture.lineage)
        .expect("same link");
    assert!(again.replayed);
    assert!(matches!(
        fixture
            .store
            .link_blocking(second.id, &operation, &worker(), now, fixture.lineage),
        Err(RunStoreError::LinkConflict { linked, .. }) if linked == first.id
    ));
    let foreign = OperationFacts {
        id: OperationId::new(),
        actor: ActorId::agent("someone-else"),
        ..operation.clone()
    };
    assert!(matches!(
        fixture
            .store
            .link_blocking(first.id, &foreign, &worker(), now, fixture.lineage),
        Err(RunStoreError::Run(RunError::LinkRefused { .. }))
    ));
    let snapshot = fixture
        .store
        .snapshot_blocking(first.id)
        .expect("read")
        .expect("run");
    assert_eq!(snapshot.operations.len(), 1);
    assert_eq!(snapshot.operations[0].operation, operation.id);
    // After the run ends, an operation committed later cannot be attributed to it.
    fixture
        .transition(first.id, RunState::Completed)
        .expect("complete");
    let ended = fixture
        .store
        .snapshot_blocking(first.id)
        .expect("read")
        .expect("run")
        .last;
    let late = OperationFacts {
        id: OperationId::new(),
        timestamp: ended.recorded_at + TimeDelta::seconds(1),
        ..operation
    };
    assert!(matches!(
        fixture
            .store
            .link_blocking(first.id, &late, &worker(), now, fixture.lineage),
        Err(RunStoreError::Run(RunError::LinkRefused { .. }))
    ));
}

#[test]
fn restart_recovers_every_recorded_fact_from_the_file() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let mut store =
        RunStore::open_or_create_blocking(&path, plan.workspace.id, lineage).expect("create");
    let record = record(&plan, lineage, &request(work));
    store.start_blocking(&record, lineage).expect("start");
    store
        .append_activity_blocking(
            &[ActivityInput {
                run: record.id,
                source_sequence: 1,
                kind: ActivityKind::Heartbeat,
                text: None,
                observed_at: None,
            }],
            &worker(),
            Utc::now(),
            lineage,
        )
        .expect("activity");
    drop(store);
    let reopened = RunStore::open_existing_blocking(&path, plan.workspace.id)
        .expect("open")
        .expect("exists");
    let snapshot = reopened
        .snapshot_blocking(record.id)
        .expect("read")
        .expect("run");
    assert_eq!(snapshot.record, record);
    assert_eq!(
        snapshot.last.state,
        RunState::Working,
        "no one reported an end"
    );
    assert_eq!(snapshot.activity.recorded, 1);
    assert_eq!(
        reopened.binding_blocking().expect("binding").lineage_id,
        lineage
    );
}
