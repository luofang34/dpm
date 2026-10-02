//! Retries under bounded retention: identity covers the whole request, and a record that retention
//! removed can never come back as a new receipt.

use super::*;
use dpm_model::{MAX_ACTIVITY_TEXT_BYTES, ObservedStatus};

fn input(run: RunId, source_sequence: u64, text: Option<String>) -> ActivityInput {
    ActivityInput {
        run,
        source_sequence,
        kind: ActivityKind::ToolResult,
        text,
        observed_at: None,
    }
}

fn heartbeat(run: RunId, source_sequence: u64) -> ActivityInput {
    ActivityInput {
        kind: ActivityKind::Heartbeat,
        ..input(run, source_sequence, None)
    }
}

impl Fixture {
    fn append(
        &mut self,
        inputs: &[ActivityInput],
        at: chrono::DateTime<Utc>,
    ) -> Result<Vec<Written<dpm_model::ActivityEntry>>, RunStoreError> {
        self.store
            .append_activity_blocking(inputs, &worker(), at, self.lineage)
    }

    fn feed(&self) -> dpm_model::ActivityPage {
        self.store
            .activity_page_blocking(0, 1000, None)
            .expect("feed")
    }
}

#[test]
fn a_changed_tail_beyond_the_kept_text_is_a_conflict_and_an_exact_resend_is_a_replay() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let prefix = "x".repeat(MAX_ACTIVITY_TEXT_BYTES);
    let original = input(run, 1, Some(format!("{prefix}A")));
    let now = Utc::now();
    let stored = fixture
        .append(std::slice::from_ref(&original), now)
        .expect("stored");
    let record = &stored[0].value.record;
    assert!(record.truncated);
    assert_eq!(
        record.text.as_deref(),
        Some(prefix.as_str()),
        "the kept text is bounded"
    );
    assert_eq!(record.text_digest.as_deref().map(str::len), Some(64));

    let again = fixture
        .append(std::slice::from_ref(&original), now + TimeDelta::seconds(5))
        .expect("exact resend");
    assert!(again[0].replayed);
    assert_eq!(again[0].value, stored[0].value, "the first receipt stands");

    let changed = input(run, 1, Some(format!("{prefix}B")));
    assert!(matches!(
        fixture.append(std::slice::from_ref(&changed), now),
        Err(RunStoreError::DuplicateActivity { .. })
    ));
    // A prefix-only difference is a conflict as well, as is dropping the text altogether.
    for text in [Some(prefix.clone()), None, Some(format!("y{prefix}A"))] {
        assert!(matches!(
            fixture.append(&[input(run, 1, text)], now),
            Err(RunStoreError::DuplicateActivity { .. })
        ));
    }
    assert_eq!(
        fixture.feed().entries.len(),
        1,
        "nothing was written by the conflicts"
    );
}

#[test]
fn a_conflict_inside_a_batch_rolls_the_whole_batch_back() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let prefix = "x".repeat(MAX_ACTIVITY_TEXT_BYTES);
    let now = Utc::now();
    fixture
        .append(&[input(run, 1, Some(format!("{prefix}A")))], now)
        .expect("stored");
    let batch = [heartbeat(run, 2), input(run, 1, Some(format!("{prefix}B")))];
    assert!(matches!(
        fixture.append(&batch, now),
        Err(RunStoreError::DuplicateActivity { .. })
    ));
    let feed = fixture.feed();
    assert_eq!(feed.entries.len(), 1);
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(
        (
            snapshot.activity.recorded,
            snapshot.activity.source_high_water
        ),
        (1, 1)
    );
}

#[test]
fn the_text_identity_survives_a_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let mut store =
        RunStore::open_or_create_blocking(&path, plan.workspace.id, lineage).expect("create");
    let record = record(&plan, lineage, &request(work));
    store.start_blocking(&record, lineage).expect("start");
    let prefix = "x".repeat(MAX_ACTIVITY_TEXT_BYTES);
    let original = input(record.id, 1, Some(format!("{prefix}A")));
    store
        .append_activity_blocking(
            std::slice::from_ref(&original),
            &worker(),
            Utc::now(),
            lineage,
        )
        .expect("stored");
    drop(store);
    let mut reopened = RunStore::open_existing_blocking(&path, plan.workspace.id)
        .expect("open")
        .expect("exists");
    let replay = reopened
        .append_activity_blocking(
            std::slice::from_ref(&original),
            &worker(),
            Utc::now(),
            lineage,
        )
        .expect("resend");
    assert!(replay[0].replayed);
    let changed = input(record.id, 1, Some(format!("{prefix}B")));
    assert!(matches!(
        reopened.append_activity_blocking(&[changed], &worker(), Utc::now(), lineage),
        Err(RunStoreError::DuplicateActivity { .. })
    ));
}

#[test]
fn a_pruned_retry_is_expired_and_neither_records_nor_refreshes_the_run() {
    let mut fixture = fixture();
    fixture.store.set_activity_limit(1);
    let run = fixture.start().id;
    let now = Utc::now();
    fixture.append(&[heartbeat(run, 1)], now).expect("first");
    fixture
        .append(&[heartbeat(run, 2)], now + TimeDelta::seconds(1))
        .expect("second");
    let before = fixture.feed();
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    let refused = fixture.append(&[heartbeat(run, 1)], now + TimeDelta::seconds(600));
    assert!(
        matches!(
            refused,
            Err(RunStoreError::ActivityExpired {
                source_sequence: 1,
                high_water: 2,
                ..
            })
        ),
        "{refused:?}"
    );
    assert_eq!(fixture.feed(), before, "the feed did not move");
    let after = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(
        after, snapshot,
        "neither the tally nor the last receipt changed"
    );
    assert_eq!(after.activity.recorded, 2);
    assert_eq!(
        after
            .activity
            .latest
            .as_ref()
            .map(|latest| latest.recorded_at),
        Some(now + TimeDelta::seconds(1))
    );
    // Derived freshness is exactly what it was: the retry cannot make a silent run look alive.
    let at = now + TimeDelta::seconds(1) + dpm_engine::STALE_AFTER;
    let view = dpm_engine::project_run(after, None, Some(fixture.lineage), at);
    assert_eq!(view.status, ObservedStatus::Stale);
}

#[test]
fn retention_zero_keeps_nothing_and_still_refuses_every_retry() {
    let mut fixture = fixture();
    fixture.store.set_activity_limit(0);
    let run = fixture.start().id;
    let now = Utc::now();
    let written = fixture
        .append(&[heartbeat(run, 1)], now)
        .expect("accepted, then pruned");
    assert!(!written[0].replayed);
    let feed = fixture.feed();
    assert!(feed.entries.is_empty());
    assert_eq!(feed.head_sequence, 1);
    assert!(matches!(
        fixture.append(&[heartbeat(run, 1)], now + TimeDelta::seconds(9)),
        Err(RunStoreError::ActivityExpired { .. })
    ));
    fixture
        .append(&[heartbeat(run, 2)], now + TimeDelta::seconds(10))
        .expect("a newer sequence is still accepted");
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(
        (
            snapshot.activity.recorded,
            snapshot.activity.retained,
            snapshot.activity.source_high_water
        ),
        (2, 0, 2)
    );
}

#[test]
fn an_expired_record_inside_a_batch_rolls_the_whole_batch_back() {
    let mut fixture = fixture();
    fixture.store.set_activity_limit(1);
    let run = fixture.start().id;
    let now = Utc::now();
    fixture.append(&[heartbeat(run, 1)], now).expect("first");
    fixture.append(&[heartbeat(run, 2)], now).expect("second");
    let before = fixture.feed();
    let batch = [heartbeat(run, 3), heartbeat(run, 1)];
    assert!(matches!(
        fixture.append(&batch, now + TimeDelta::seconds(9)),
        Err(RunStoreError::ActivityExpired { .. })
    ));
    assert_eq!(fixture.feed(), before);
    let snapshot = fixture
        .store
        .snapshot_blocking(run)
        .expect("read")
        .expect("run");
    assert_eq!(
        (
            snapshot.activity.recorded,
            snapshot.activity.source_high_water
        ),
        (2, 2)
    );
}

#[test]
fn sequences_must_rise_but_may_skip_and_a_late_arrival_is_expired() {
    let mut fixture = fixture();
    let run = fixture.start().id;
    let now = Utc::now();
    fixture.append(&[heartbeat(run, 1)], now).expect("one");
    fixture
        .append(&[heartbeat(run, 10)], now)
        .expect("a gap is allowed");
    let late = fixture.append(&[heartbeat(run, 5)], now);
    assert!(
        matches!(
            late,
            Err(RunStoreError::ActivityExpired { high_water: 10, .. })
        ),
        "{late:?}"
    );
    // The same sequence from another run is independent.
    let other = fixture.start().id;
    fixture
        .append(&[heartbeat(other, 1)], now)
        .expect("per-run sequences");
    // Zero is not a sequence.
    assert!(matches!(
        fixture.append(&[heartbeat(run, 0)], now),
        Err(RunStoreError::Run(RunError::Validation(_)))
    ));
}

#[test]
fn the_high_water_mark_survives_a_restart() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let mut store =
        RunStore::open_or_create_blocking(&path, plan.workspace.id, lineage).expect("create");
    store.set_activity_limit(1);
    let record = record(&plan, lineage, &request(work));
    store.start_blocking(&record, lineage).expect("start");
    let now = Utc::now();
    for sequence in [1, 2] {
        store
            .append_activity_blocking(&[heartbeat(record.id, sequence)], &worker(), now, lineage)
            .expect("activity");
    }
    drop(store);
    let mut reopened = RunStore::open_existing_blocking(&path, plan.workspace.id)
        .expect("open")
        .expect("exists");
    reopened.set_activity_limit(1);
    let refused = reopened.append_activity_blocking(
        &[heartbeat(record.id, 1)],
        &worker(),
        now + TimeDelta::seconds(600),
        lineage,
    );
    assert!(
        matches!(
            refused,
            Err(RunStoreError::ActivityExpired { high_water: 2, .. })
        ),
        "{refused:?}"
    );
    let snapshot = reopened
        .snapshot_blocking(record.id)
        .expect("read")
        .expect("run");
    assert_eq!(snapshot.activity.recorded, 2);
}

#[test]
fn the_receipt_counter_wraps_and_the_sequence_boundary_is_a_typed_refusal() {
    let directory = tempfile::tempdir().expect("directory");
    let path = directory.path().join("plan.sqlite.runs");
    let (plan, work) = claimed();
    let lineage = LineageId::new();
    let mut store =
        RunStore::open_or_create_blocking(&path, plan.workspace.id, lineage).expect("create");
    let record = record(&plan, lineage, &request(work));
    store.start_blocking(&record, lineage).expect("start");
    let now = Utc::now();
    store
        .append_activity_blocking(&[heartbeat(record.id, 1)], &worker(), now, lineage)
        .expect("first");
    // The counter stores the whole u64 range bit for bit; put it on its last value.
    store
        .connection
        .execute("UPDATE run_activity_tally SET recorded = -1", [])
        .expect("boundary");
    let read = |store: &RunStore| {
        store
            .snapshot_blocking(record.id)
            .expect("read")
            .expect("run")
    };
    assert_eq!(read(&store).activity.recorded, u64::MAX);
    store
        .append_activity_blocking(&[heartbeat(record.id, 2)], &worker(), now, lineage)
        .expect("wraps instead of failing");
    assert_eq!(read(&store).activity.recorded, 0);
    // The highest possible sequence is accepted once; nothing can follow it, and that is typed.
    store
        .append_activity_blocking(&[heartbeat(record.id, u64::MAX)], &worker(), now, lineage)
        .expect("the final sequence");
    assert_eq!(read(&store).activity.source_high_water, u64::MAX);
    let refused = store.append_activity_blocking(
        &[heartbeat(record.id, u64::MAX - 1)],
        &worker(),
        now,
        lineage,
    );
    assert!(
        matches!(
            refused,
            Err(RunStoreError::ActivityExpired {
                high_water: u64::MAX,
                ..
            })
        ),
        "{refused:?}"
    );
    drop(store);
    crate::sqlite::runs::recovery::verify_blocking(&path)
        .expect("a wrapped counter and a full sequence still verify");
}
