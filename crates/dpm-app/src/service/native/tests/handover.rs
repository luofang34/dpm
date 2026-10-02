//! Worked sequences: snapshot to subscription, repeated pages, slow consumers, independent feeds
//! and retention. Nothing here compares revisions for order, and no sequence claims the project
//! store and the run store were read atomically.

use super::{harness::*, *};
use dpm_model::RunState;

#[test]
fn a_write_during_startup_is_in_the_first_poll_once_and_never_lost() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);

    // 1. The client reads its snapshot. The view names the position it is anchored to.
    let snapshot = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    assert_eq!(project_basis(&snapshot).history_head, 1);

    // 2. Another process commits while the client is still starting up, before it subscribes.
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );

    // 3. The first poll, from the snapshot's anchor, delivers exactly that write: not the claim
    //    the snapshot already contains, and not nothing.
    let mut client = Consumer::from_views(&[&snapshot]);
    let first = client.poll_blocking(&mut app, 100);
    assert_eq!(client.project, [2]);
    assert!(!first.project.as_ref().expect("project").more);
    assert_eq!(first.watermark.project.history_head, 2);

    // 4. A poll retried because its response was lost returns the same page, and a poll from the
    //    advanced cursor returns nothing: no duplicate can reach the client's state.
    let mut retried = Consumer::from_views(&[&snapshot]);
    retried.poll_blocking(&mut app, 100);
    assert_eq!(retried.project, client.project);
    let quiet = client.poll_blocking(&mut app, 100);
    assert!(quiet.project.expect("project").entries.is_empty());
    assert_eq!(client.project, [2]);

    // 5. Later writes continue the same cursor without a hole.
    commit_blocking(
        &mut other,
        &worker(),
        Command::Submit {
            work: task,
            note: None,
            occurred_at: None,
        },
    );
    client.drain_blocking(&mut app, 100);
    assert_eq!(client.project, [2, 3]);
    assert!(client.resets.is_empty());
}

#[test]
fn a_page_delivered_twice_to_the_same_consumer_is_discarded_by_identity_and_sequence() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);
    let attached = attach_blocking(&mut app);
    let mut client = Consumer::seeded(&attached.watermark);
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let run = start_run_blocking(&mut other);
    activity_blocking(&mut other, run, 1);

    // The page is applied once; the client refreshes and is current.
    let page = changes_blocking(&mut app, client.cursors, Some(100));
    client.apply(&page);
    assert_eq!(
        (
            client.project.clone(),
            client.lifecycle.clone(),
            client.activity.clone()
        ),
        (vec![2], vec![1], vec![1])
    );
    assert!(client.dirty());
    client
        .refresh_all_blocking(
            &mut app,
            &[
                json!({"query": "status", "probabilistic": false}),
                json!({"query": "runs"}),
            ],
            None,
        )
        .expect("refresh");
    assert!(!client.dirty());
    let cursors = client.cursors;

    // The same page arrives again, as a retry whose first answer was already applied would. Nothing
    // is applied twice, the cursors do not move, and the client is not made stale again.
    client.apply(&page);
    client.apply(&page);
    assert_eq!(
        (
            client.project.clone(),
            client.lifecycle.clone(),
            client.activity.clone()
        ),
        (vec![2], vec![1], vec![1])
    );
    assert_eq!(client.discarded, 6, "three entries, twice");
    assert_eq!(client.cursors, cursors);
    assert!(!client.dirty() && client.resets.is_empty());

    // An older page that arrives late cannot rewind a cursor either.
    let older = changes_blocking(
        &mut app,
        Consumer::seeded(&attached.watermark).cursors,
        Some(100),
    );
    client.apply(&older);
    assert_eq!(client.cursors, cursors);
    assert_eq!(client.project, [2]);
}

#[test]
fn a_slow_consumer_catches_up_in_bounded_pages_and_costs_the_producer_nothing() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);
    let mut client = Consumer::seeded(&attach_blocking(&mut app).watermark);

    // The client stops polling while the project and a run both move on.
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let run = start_run_blocking(&mut other);
    for index in 1..=5 {
        activity_blocking(&mut other, run, index);
    }
    report_blocking(&mut other, run, RunState::Waiting);

    // Every page is at most the limit, the limit is clamped to at least one, and `more` stays
    // true until the feed is drained. The producer kept no per-client state while it waited.
    let first = client.poll_blocking(&mut app, 2);
    assert_eq!(first.activity.as_ref().expect("activity").entries.len(), 2);
    assert!(first.activity.as_ref().expect("activity").more);
    assert!(first.lifecycle.as_ref().expect("lifecycle").entries.len() <= 2);
    let pages = client.drain_blocking(&mut app, 2);
    assert!(pages >= 2);
    assert_eq!(client.activity, [1, 2, 3, 4, 5]);
    assert_eq!(client.lifecycle, [1, 2]);
    assert_eq!(client.project, [2]);
    let clamped = changes_blocking(
        &mut app,
        Cursors {
            activity: Some(FeedCursor {
                epoch: first.watermark.runs.epoch,
                after_sequence: 0,
            }),
            ..Cursors::default()
        },
        Some(0),
    );
    assert_eq!(
        clamped.activity.expect("activity").entries.len(),
        1,
        "a zero limit means one"
    );
}

#[test]
fn project_lifecycle_and_activity_positions_move_independently() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _) = shared_blocking(directory.path());
    let task = task_blocking(&app);
    let run = start_run_blocking(&mut app);
    let base = attach_blocking(&mut app).watermark;
    assert_eq!(
        (
            base.project.history_head,
            base.runs.lifecycle_head,
            base.runs.activity_head
        ),
        (1, 1, 0)
    );

    // Activity moves only the activity head.
    activity_blocking(&mut app, run, 1);
    let after_activity = attach_blocking(&mut app).watermark;
    assert_eq!(after_activity.project, base.project);
    assert_eq!(after_activity.runs.lifecycle_head, 1);
    assert_eq!(after_activity.runs.activity_head, 1);

    // A lifecycle transition moves only the lifecycle head.
    report_blocking(&mut app, run, RunState::Waiting);
    let after_lifecycle = attach_blocking(&mut app).watermark;
    assert_eq!(after_lifecycle.project, base.project);
    assert_eq!(after_lifecycle.runs.lifecycle_head, 2);
    assert_eq!(after_lifecycle.runs.activity_head, 1);

    // A project operation moves only the project head; the run's report of work is not one.
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let after_project = attach_blocking(&mut app).watermark;
    assert_eq!(after_project.project.history_head, 2);
    assert_eq!(after_project.runs, after_lifecycle.runs);

    // The three feeds answer separately, each from its own cursor, in one call.
    let at_base = Cursors {
        project: Some(ProjectCursor {
            lineage_id: base.project.lineage_id,
            after_sequence: base.project.history_head,
        }),
        lifecycle: Some(FeedCursor {
            epoch: base.runs.epoch,
            after_sequence: 0,
        }),
        activity: Some(FeedCursor {
            epoch: base.runs.epoch,
            after_sequence: 1,
        }),
        links: None,
    };
    let result = changes_blocking(&mut app, at_base, None);
    assert_eq!(result.project.expect("project").entries.len(), 1);
    assert_eq!(result.lifecycle.expect("lifecycle").entries.len(), 2);
    assert!(result.activity.expect("activity").entries.is_empty());
}

#[test]
fn a_client_attached_before_any_run_follows_the_first_run_without_a_spurious_reset() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _) = shared_blocking(directory.path());
    let attached = attach_blocking(&mut app);
    assert_eq!(
        attached.watermark.runs.epoch, None,
        "no run store exists yet"
    );
    let mut client = Consumer::seeded(&attached.watermark);
    let run = start_run_blocking(&mut app);
    activity_blocking(&mut app, run, 1);
    client.drain_blocking(&mut app, 100);
    assert!(client.resets.is_empty(), "{:?}", client.resets);
    assert_eq!((client.lifecycle.len(), client.activity.len()), (1, 1));
    assert!(client.cursors.lifecycle.expect("cursor").epoch.is_some());
}

#[test]
fn retention_that_outruns_a_cursor_is_reported_as_a_gap_and_the_feed_continues() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _) = shared_blocking(directory.path());
    app.set_run_retention(3);
    let run = start_run_blocking(&mut app);
    let epoch = attach_blocking(&mut app).watermark.runs.epoch;
    for index in 1..=5 {
        activity_blocking(&mut app, run, index);
    }
    let behind = Cursors {
        activity: Some(FeedCursor {
            epoch,
            after_sequence: 0,
        }),
        ..Cursors::default()
    };
    let result = changes_blocking(&mut app, behind, None);
    let delta = result.activity.expect("activity");
    let gap = delta.gap.expect("the cursor was behind retention");
    assert_eq!((gap.requested_after, gap.resumes_at, gap.lost), (0, 3, 2));
    assert_eq!(
        delta.status,
        FeedStatus::Continue,
        "a gap is not a reset: the feed goes on"
    );
    assert_eq!(delta.entries.len(), 3);
    assert_eq!(result.watermark.runs.activity_pruned_through, 2);
    // Following the page's cursor reads on without a gap, and the lifecycle feed was untouched.
    let next = Cursors {
        activity: Some(FeedCursor {
            epoch,
            after_sequence: delta.next_after_sequence,
        }),
        lifecycle: Some(FeedCursor {
            epoch,
            after_sequence: 0,
        }),
        ..Cursors::default()
    };
    let tail = changes_blocking(&mut app, next, None);
    let activity = tail.activity.expect("activity");
    assert!(activity.entries.is_empty() && activity.gap.is_none() && !activity.more);
    assert_eq!(tail.lifecycle.expect("lifecycle").entries.len(), 1);
}
