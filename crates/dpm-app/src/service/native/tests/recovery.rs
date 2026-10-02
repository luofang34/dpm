//! Restored and rolled-back sources: a cursor that cannot be followed is an explicit reset, never
//! a silent continuation, and no sequence relies on comparing revisions for order.

use super::{harness::*, *};
use dpm_model::{ActivityKind, Observation, RunId};

/// A live store with three project operations, a run and two activity records, and a consumer
/// that has read all of it.
fn followed_blocking(directory: &Path) -> (Application, Consumer, Watermark) {
    let mut app = live_blocking(directory);
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let run = app
        .start_run_blocking(crate::RunStartRequest {
            actor: worker(),
            work_key: "TEST-A".into(),
            executor: None,
            run_id: Some(RunId::new()),
            parent: None,
            session: None,
            observation: Observation::ReportedOnly,
            sources: Vec::new(),
            observed_at: None,
            base_lineage: None,
        })
        .expect("run")
        .data
        .run
        .run
        .id;
    for index in 1..=2 {
        app.record_run_activity_blocking(crate::RunActivityRequest {
            actor: worker(),
            entries: vec![dpm_model::ActivityInput {
                run,
                source_sequence: index,
                kind: ActivityKind::Progress,
                text: None,
                observed_at: None,
            }],
            base_lineage: None,
        })
        .expect("activity");
    }
    // A first launch reads every feed from its beginning.
    let seed = attach_blocking(&mut app).watermark;
    let mut client = Consumer::seeded(&Watermark {
        project: ProjectWatermark {
            history_head: 0,
            ..seed.project
        },
        runs: dpm_model::RunFeedHeads::default(),
    });
    client.drain_blocking(&mut app, 100);
    // The client then installs a view of the runs it displays, which is what fixes its link mark.
    client
        .refresh_blocking(&mut app, json!({"query": "runs"}), None)
        .expect("the first view");
    (app, client, seed)
}

#[test]
fn a_restored_copy_resets_every_feed_instead_of_continuing_the_old_cursors() {
    let directory = tempfile::tempdir().expect("directory");
    let (app, client, seed) = followed_blocking(directory.path());
    assert_eq!(client.project, [1, 2]);
    assert_eq!((client.lifecycle.len(), client.activity.len()), (1, 2));
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    let restored = directory.path().join("restored.sqlite");
    crate::restore_store_blocking(&archive, &restored).expect("restore");
    let mut copy = Application::open_blocking(&restored).expect("open the restored copy");

    // The restored store is another history: the client is told so, feed by feed, and receives
    // no entries it could mistake for a continuation.
    let mut stale = client;
    stale.resets.clear();
    let result = stale.poll_blocking(&mut copy, 100);
    for status in [
        result.project.as_ref().expect("project").status,
        result.lifecycle.as_ref().expect("lifecycle").status,
        result.activity.as_ref().expect("activity").status,
        result.links.as_ref().expect("links").status,
    ] {
        assert!(matches!(status, FeedStatus::Reset { .. }), "{status:?}");
    }
    assert_eq!(
        stale.resets,
        [
            ResetReason::LineageChanged,
            ResetReason::EpochChanged,
            ResetReason::EpochChanged,
            ResetReason::EpochChanged
        ]
    );
    assert!(result.project.expect("project").entries.is_empty());
    // The consumer also resets itself on seeing another feed identity: what it held for the old
    // history is discarded, never merged with the new one.
    assert_eq!(
        stale.identity_resets,
        [ResetReason::LineageChanged, ResetReason::EpochChanged]
    );
    assert!(stale.project.is_empty() && stale.lifecycle.is_empty() && stale.activity.is_empty());
    assert!(stale.dirty());

    // A view computed for the old attachment is refused with both identities.
    let response = exchange_blocking(
        &mut copy,
        json!({"type": "query", "query": {"query": "revision"}, "attached": Attachment {
            workspace_id: seed.project.workspace_id,
            lineage_id: seed.project.lineage_id,
        }}),
    );
    let error = refusal(&response);
    assert_eq!(error["code"], "lineage_mismatch");
    assert_eq!(error["details"]["expected"], json!(seed.project.lineage_id));
    assert_ne!(error["details"]["actual"], error["details"]["expected"]);

    // Re-seeding from a fresh attach recovers: the new history is followed from its own head.
    let fresh = attach_blocking(&mut copy);
    assert_eq!(fresh.source, SourceKind::Live);
    assert_ne!(fresh.attachment.lineage_id, seed.project.lineage_id);
    let mut reseeded = Consumer::seeded(&fresh.watermark);
    let task = task_blocking(&copy);
    commit_blocking(
        &mut copy,
        &worker(),
        Command::ReportProgress {
            work: task,
            percent: 5,
            note: None,
        },
    );
    reseeded.drain_blocking(&mut copy, 100);
    assert_eq!(reseeded.project.len(), 1);
    assert!(reseeded.resets.is_empty());
}

#[test]
fn a_source_rolled_back_to_an_older_state_of_the_same_history_is_a_cursor_ahead_reset() {
    let directory = tempfile::tempdir().expect("directory");
    let mut app = live_blocking(directory.path());
    let task = task_blocking(&app);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    commit_blocking(
        &mut app,
        &worker(),
        Command::ReportProgress {
            work: task,
            percent: 5,
            note: None,
        },
    );
    let head = attach_blocking(&mut app).watermark;
    assert_eq!(head.project.history_head, 3);
    let mut client = Consumer::seeded(&head);

    // The archive keeps the lineage it was taken from but holds fewer operations: the client's
    // cursor is beyond what the source holds, which a revision comparison could not say reliably
    // because revisions wrap.
    let mut older = Application::open_blocking(&archive).expect("archive");
    let result = client.poll_blocking(&mut older, 100);
    let delta = result.project.expect("project");
    assert_eq!(
        delta.status,
        FeedStatus::Reset {
            reason: ResetReason::CursorAhead
        }
    );
    assert!(delta.entries.is_empty());
    assert_eq!(
        delta.next_after_sequence, 3,
        "the client's own cursor is echoed back"
    );
    assert_eq!(result.watermark.project.history_head, 1);
}

#[test]
fn revisions_that_wrap_never_decide_order_and_history_still_hands_over() {
    let directory = tempfile::tempdir().expect("directory");
    let mut plan = fixture_plan();
    plan.revision = u64::MAX - 1;
    let mut app = Application::initialize_blocking(&directory.path().join("state.sqlite"), &plan)
        .expect("a store near the end of the revision counter");
    let task = task_blocking(&app);
    let snapshot = attach_blocking(&mut app);
    assert_eq!(snapshot.watermark.project.revision, u64::MAX - 1);
    let mut client = Consumer::seeded(&snapshot.watermark);
    commit_blocking(&mut app, &worker(), Command::Claim { work: task });
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let wrapped = attach_blocking(&mut app).watermark;
    assert!(
        wrapped.project.revision < snapshot.watermark.project.revision,
        "the revision wrapped, so numeric comparison would call this older"
    );
    // The append cursor, not the revision, orders history, so nothing is lost or repeated.
    client.drain_blocking(&mut app, 100);
    assert_eq!(client.project, [1, 2]);
    assert!(client.resets.is_empty());
    // Equality is the only comparison a watermark supports.
    assert_ne!(wrapped, snapshot.watermark);
    assert_eq!(attach_blocking(&mut app).watermark, wrapped);
}
