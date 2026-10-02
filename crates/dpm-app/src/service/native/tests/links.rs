//! An operation linked to a run after the fact changes the run's view with no lifecycle
//! transition and no activity. It is tracked by an invalidation count of its own, so equal run
//! positions in one epoch never describe different run facts and a link never makes a run look
//! newer. The count is not a feed: it carries no order and no resumable position, so these
//! sequences exercise only what it does promise, including across restart, backup, restore and
//! rollback.

use super::{harness::*, *};
use dpm_model::RunId;

/// A started task, a run on it, and a submitted operation that is not yet linked to the run.
fn unlinked_blocking(
    directory: &Path,
) -> (Application, Application, RunId, dpm_model::OperationId) {
    let (mut app, other) = shared_blocking(directory);
    let task = task_blocking(&app);
    commit_blocking(
        &mut app,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let run = start_run_blocking(&mut app);
    let submitted = commit_blocking(
        &mut app,
        &worker(),
        Command::Submit {
            work: task,
            note: None,
            occurred_at: None,
        },
    );
    (app, other, run, submitted.operation.id)
}

fn link_blocking(app: &mut Application, run: RunId, operation: dpm_model::OperationId) -> bool {
    app.link_run_operation_blocking(crate::RunLinkRequest {
        actor: worker(),
        run,
        operation,
        base_lineage: None,
    })
    .expect("link")
    .data
    .replayed
}

fn run_view_blocking(app: &mut Application, run: RunId) -> View {
    view_blocking(app, json!({"query": "run", "id": run}))
}

fn linked_operations(view: &View) -> usize {
    view.envelope.data["operations"]
        .as_array()
        .map_or(0, Vec::len)
}

#[test]
fn a_late_link_moves_the_link_count_and_nothing_a_run_report_would() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _, run, operation) = unlinked_blocking(directory.path());
    let before = run_view_blocking(&mut app, run);
    assert_eq!(linked_operations(&before), 0);
    assert_eq!(run_basis(&before).link_count, 0);

    assert!(
        !link_blocking(&mut app, run, operation),
        "a first link is recorded, not replayed"
    );
    let after = run_view_blocking(&mut app, run);
    assert_eq!(linked_operations(&after), 1);
    assert_eq!(
        after.envelope.data["operations"][0]["operation"],
        json!(operation)
    );

    // The run's view changed, so its basis must differ; the count that moved is the link's, and
    // nothing else did.
    assert_ne!(before.basis, after.basis);
    assert_eq!(run_basis(&after).link_count, 1);
    assert_eq!(after.basis.project, before.basis.project);
    let (was, now) = (run_basis(&before), run_basis(&after));
    assert_eq!(
        (
            now.epoch,
            now.lifecycle_head,
            now.activity_head,
            now.activity_pruned_through
        ),
        (
            was.epoch,
            was.lifecycle_head,
            was.activity_head,
            was.activity_pruned_through
        )
    );
    // A link is not a sign of life: the run is no fresher and no transition was invented.
    for field in [
        "state",
        "state_since",
        "status",
        "last_receipt_at",
        "stale_at",
    ] {
        assert_eq!(
            before.envelope.data[field], after.envelope.data[field],
            "{field}"
        );
    }
    assert_eq!(
        after.envelope.data["activity"],
        before.envelope.data["activity"]
    );

    // A replay of the same link is not a new fact, so the count does not move.
    assert!(
        link_blocking(&mut app, run, operation),
        "the second delivery is a replay"
    );
    assert_eq!(run_view_blocking(&mut app, run).basis, after.basis);
}

#[test]
fn an_observer_that_missed_a_link_is_told_to_read_its_runs_again_and_then_is_current() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other, run, operation) = unlinked_blocking(directory.path());
    let held = run_view_blocking(&mut app, run);
    let mut client = Consumer::from_views(&[&held]);

    // Another process links while the observer is away.
    link_blocking(&mut other, run, operation);

    let result = client.poll_blocking(&mut app, 100);
    let signal = result.links.expect("links");
    assert_eq!(signal.status, FeedStatus::Continue);
    assert!(signal.changed);
    assert_eq!(signal.count, 1);
    assert_eq!(
        signal.count, result.watermark.runs.link_count,
        "the count and the watermark agree"
    );
    // No feed that tracks transitions, activity or project operations has anything to say: the
    // token is the only thing that tells the observer its view is stale.
    assert!(
        result
            .lifecycle
            .as_ref()
            .expect("lifecycle")
            .entries
            .is_empty()
    );
    assert!(
        result
            .activity
            .as_ref()
            .expect("activity")
            .entries
            .is_empty()
    );
    assert!(result.project.as_ref().expect("project").entries.is_empty());
    assert_ne!(result.watermark.runs, run_basis(&held));

    // Reading the runs again, as told, gives a view anchored at the position the poll reported.
    assert!(client.runs_stale());
    let list = client
        .refresh_blocking(&mut app, json!({"query": "runs"}), None)
        .expect("a refresh");
    assert_eq!(
        list.envelope.data["runs"][0]["operations"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
    assert_eq!(list.basis.runs, Some(result.watermark.runs));

    // With the view installed the observer is current; a replayed link adds nothing.
    assert!(!client.dirty());
    link_blocking(&mut other, run, operation);
    let quiet = client.poll_blocking(&mut app, 100);
    assert!(!quiet.links.expect("links").changed);
    assert_eq!(client.links, [1]);
    assert!(client.resets.is_empty() && !client.dirty());
}

#[test]
fn a_failed_refresh_and_a_reconnect_leave_the_stale_link_view_stale_until_one_is_installed() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other, run, operation) = unlinked_blocking(directory.path());
    let held = run_view_blocking(&mut app, run);
    let mut client = Consumer::from_views(&[&held]);
    link_blocking(&mut other, run, operation);

    // The poll says the displayed runs are stale, and the client records that.
    assert!(
        client
            .poll_blocking(&mut app, 100)
            .links
            .expect("links")
            .changed
    );
    assert!(client.dirty());

    // The refresh is refused (a wrong attachment stands in for any failed read). The client did
    // not adopt the count, so it is still stale.
    let wrong = Attachment {
        workspace_id: dpm_model::WorkspaceId::new(),
        lineage_id: None,
    };
    let failed = client.refresh_blocking(&mut app, json!({"query": "runs"}), Some(wrong));
    assert_eq!(failed.expect_err("a refusal"), "workspace_mismatch");
    assert!(client.dirty());

    // The connection drops and is reopened. The client keeps its local state, so the next poll
    // still reports the change instead of calling the view current.
    drop((app, other));
    let mut app =
        Application::open_blocking(directory.path().join("state.sqlite")).expect("reopen");
    let again = client.poll_blocking(&mut app, 100);
    assert!(
        again.links.expect("links").changed,
        "the staleness was not forgotten"
    );
    assert!(client.dirty());

    // Only an installed view clears it, and then a poll is quiet.
    let list = client
        .refresh_blocking(&mut app, json!({"query": "runs"}), None)
        .expect("a successful refresh");
    assert_eq!(linked_operations_in_list(&list), 1);
    assert!(!client.dirty());
    assert!(
        !client
            .poll_blocking(&mut app, 100)
            .links
            .expect("links")
            .changed
    );
    assert!(!client.dirty());
}

fn linked_operations_in_list(view: &View) -> usize {
    view.envelope.data["runs"][0]["operations"]
        .as_array()
        .map_or(0, Vec::len)
}

#[test]
fn a_failed_refresh_after_a_feed_entry_also_stays_stale_though_the_cursor_moved_on() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let held = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    let mut client = Consumer::from_views(&[&held]);
    let task = task_blocking(&app);

    // A project operation arrives. Its page is applied, so the cursor advances; the view is stale.
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    client.poll_blocking(&mut app, 100);
    assert_eq!(client.project.len(), 1);
    assert!(client.dirty());

    // The refresh fails. The next poll has no new entries, yet the client is still stale: the
    // consumer's own state, not the absence of entries, decides when it is current.
    let wrong = Attachment {
        workspace_id: dpm_model::WorkspaceId::new(),
        lineage_id: None,
    };
    let status = json!({"query": "status", "probabilistic": false});
    assert!(
        client
            .refresh_blocking(&mut app, status.clone(), Some(wrong))
            .is_err()
    );
    let quiet = client.poll_blocking(&mut app, 100);
    assert!(quiet.project.expect("project").entries.is_empty());
    assert!(client.dirty());

    client
        .refresh_blocking(&mut app, status, None)
        .expect("a successful refresh");
    assert!(!client.dirty());
}

#[test]
fn a_link_landing_between_the_anchor_and_the_read_is_in_the_view_and_still_signalled() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other, run, operation) = unlinked_blocking(directory.path());
    let when: DateTime<Utc> = "2026-10-02T12:00:00Z".parse().expect("time");
    at(&mut app, when);

    let mut linked = false;
    let view = app
        .read_view_blocking(&Query::Run { id: run }, when, || {
            if !linked {
                linked = true;
                link_blocking(&mut other, run, operation);
            }
        })
        .expect("a view");
    assert!(linked);

    // The anchor is the count before the link; the answer already lists it.
    assert_eq!(run_basis(&view).link_count, 0);
    assert_eq!(linked_operations(&view), 1);

    // The signal still fires, so the observer reads once more than it strictly needed to, and can
    // never read once less. Reading again anchors it at the count it has by then.
    let mut client = Consumer::from_views(&[&view]);
    assert!(
        client
            .poll_blocking(&mut app, 100)
            .links
            .expect("links")
            .changed
    );
    let list = client
        .refresh_blocking(&mut app, json!({"query": "runs"}), None)
        .expect("a refresh");
    assert_eq!(run_basis(&list).link_count, 1);
    assert!(
        !client
            .poll_blocking(&mut app, 100)
            .links
            .expect("links")
            .changed
    );
}

#[test]
fn the_link_count_survives_a_restart_and_a_followed_observer_sees_no_spurious_change() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, other, run, operation) = unlinked_blocking(directory.path());
    link_blocking(&mut app, run, operation);
    let held = attach_blocking(&mut app).watermark;
    assert_eq!(held.runs.link_count, 1);
    let mut client = Consumer::seeded(&held);
    drop((app, other));

    // A new process opens the same files: same epoch, same count, nothing to re-read.
    let mut reopened =
        Application::open_blocking(directory.path().join("state.sqlite")).expect("reopen");
    assert_eq!(attach_blocking(&mut reopened).watermark, held);
    let result = client.poll_blocking(&mut reopened, 100);
    assert!(!result.links.expect("links").changed);
    assert!(client.resets.is_empty() && client.links.is_empty());
}

#[test]
fn a_restored_copy_keeps_the_count_but_is_another_epoch_so_an_old_mark_resets() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _, run, operation) = unlinked_blocking(directory.path());
    link_blocking(&mut app, run, operation);
    let held = attach_blocking(&mut app).watermark;
    let mut client = Consumer::seeded(&held);
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");

    // The archive is the same epoch and holds the same links.
    let mut archived = Application::open_blocking(&archive).expect("archive");
    let seen = attach_blocking(&mut archived);
    assert_eq!(seen.source, SourceKind::Archive);
    assert_eq!(seen.watermark.runs.epoch, held.runs.epoch);
    assert_eq!(seen.watermark.runs.link_count, 1);
    assert!(
        !client
            .poll_blocking(&mut archived, 100)
            .links
            .expect("links")
            .changed
    );

    // A restore forks a new epoch carrying the same links: the old mark is no longer comparable,
    // though the numbers happen to be equal.
    let restored = directory.path().join("restored.sqlite");
    crate::restore_store_blocking(&archive, &restored).expect("restore");
    let mut copy = Application::open_blocking(&restored).expect("open");
    let fresh = attach_blocking(&mut copy);
    assert_eq!(fresh.watermark.runs.link_count, 1);
    assert_ne!(fresh.watermark.runs.epoch, held.runs.epoch);
    client.resets.clear();
    let result = client.poll_blocking(&mut copy, 100);
    assert_eq!(
        result.links.expect("links").status,
        FeedStatus::Reset {
            reason: ResetReason::EpochChanged
        }
    );
    assert!(client.resets.contains(&ResetReason::EpochChanged));

    // Re-seeded from the copy, the observer is current in the new epoch with nothing to re-read.
    // (The copy's inherited runs are read-only history: writes into them are refused.)
    let mut reseeded = Consumer::seeded(&fresh.watermark);
    let after = reseeded.poll_blocking(&mut copy, 100);
    assert!(!after.links.expect("links").changed);
    assert!(reseeded.resets.is_empty());
}

#[test]
fn a_source_rolled_back_to_fewer_links_is_a_cursor_ahead_reset_not_a_quiet_decrease() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _, run, operation) = unlinked_blocking(directory.path());
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive)
        .expect("backup before the link");
    link_blocking(&mut app, run, operation);
    let held = attach_blocking(&mut app).watermark;
    assert_eq!(held.runs.link_count, 1);
    let mut client = Consumer::seeded(&held);

    // The archive keeps the epoch but holds no link: the mark is above what the source has.
    let mut older = Application::open_blocking(&archive).expect("archive");
    let result = client.poll_blocking(&mut older, 100);
    let signal = result.links.expect("links");
    assert_eq!(
        signal.status,
        FeedStatus::Reset {
            reason: ResetReason::CursorAhead
        }
    );
    assert!(!signal.changed);
    assert_eq!(signal.count, 0);
}
