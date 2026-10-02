//! A view is anchored to each store it reads, before it reads it. The anchor is a lower bound: a
//! write that lands between the anchor and the read is delivered by the next poll, possibly twice
//! over what the view holds, and never lost. Telemetry never starves a read, and a project-only
//! answer cannot move a run cursor.

use super::{harness::*, *};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc,
};
use std::thread;

fn status_query() -> Query {
    Query::Status {
        probabilistic: false,
        calibrated: false,
    }
}

#[test]
fn a_write_between_the_anchor_and_the_read_is_in_the_view_and_still_delivered() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);
    let when: DateTime<Utc> = "2026-10-02T12:00:00Z".parse().expect("time");
    at(&mut app, when);

    // Another process commits exactly between the anchor read and the payload read.
    let mut landed = false;
    let view = app
        .read_view_blocking(&status_query(), when, || {
            if !landed {
                landed = true;
                commit_blocking(
                    &mut other,
                    &worker(),
                    Command::Start {
                        work: task,
                        occurred_at: None,
                    },
                );
            }
        })
        .expect("a view");
    assert!(landed);

    // The anchor is the position before the write; the answer already reflects the write.
    assert_eq!(project_basis(&view).history_head, 1);
    assert_eq!(view.envelope.revision, Some(2));

    // Nothing is lost: polling from the anchor delivers the write, which the view already holds.
    // That is an overlap, which a client discards by feed identity and sequence, not a gap.
    let mut client = Consumer::from_views(&[&view]);
    client.poll_blocking(&mut app, 100);
    assert_eq!(client.project, [2]);
    let result = changes_blocking(&mut app, client.cursors, None);
    client.apply(&result);
    assert_eq!(client.project, [2], "a repeat of the page changes nothing");
}

#[test]
fn a_project_only_answer_carries_no_run_basis_and_cannot_move_a_run_cursor() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let run = start_run_blocking(&mut app);
    activity_blocking(&mut app, run, 1);

    // A Now view is several queries: ranked work (project only) and the runs it shows.
    let runs = view_blocking(&mut app, json!({"query": "runs"}));
    assert_eq!(run_basis(&runs).activity_head, 1);

    // Activity arrives, and then the project-only part of the view is read: later than the runs.
    activity_blocking(&mut other, run, 2);
    activity_blocking(&mut other, run, 3);
    let next = view_blocking(
        &mut app,
        json!({"query": "next", "capabilities": [], "probabilistic": false, "limit": 5}),
    );
    assert_eq!(project_basis(&next).history_head, 1);
    assert_eq!(
        next.basis.runs, None,
        "a project-only answer says nothing about the run store"
    );

    // The consumer takes its run cursors from the held run snapshot, never from the later answer,
    // so the activity absent from that snapshot is delivered rather than skipped.
    let mut client = Consumer::from_views(&[&next, &runs]);
    client.poll_blocking(&mut app, 100);
    assert_eq!(client.activity, [2, 3]);
    assert!(client.runs_stale());

    // A refresh of only the project-only query cannot mark the run views current; the run view
    // must be read again.
    client
        .refresh_all_blocking(
            &mut app,
            &[json!({"query": "next", "capabilities": [], "probabilistic": false, "limit": 5})],
            None,
        )
        .expect("project refresh");
    assert!(!client.project_stale() && client.runs_stale());
    client
        .refresh_all_blocking(&mut app, &[json!({"query": "runs"})], None)
        .expect("run refresh");
    assert!(!client.dirty());

    // The hazard is real: a client that seeded its run cursors from a position read after the
    // activity would never have been told.
    let late = attach_blocking(&mut app).watermark;
    let mut careless = Consumer::seeded(&late);
    careless.poll_blocking(&mut app, 100);
    assert!(careless.activity.is_empty());
}

/// Reads until the writer has appended `target` records more than when it started.
fn read_during_writes_blocking(
    run: dpm_model::RunId,
    mut writer: Application,
    target: u64,
    mut read: impl FnMut(),
) {
    let written = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let (started_tx, started_rx) = mpsc::channel();
    let handle = {
        let (written, stop) = (written.clone(), stop.clone());
        thread::spawn(move || {
            let mut sequence = 1_000;
            let mut announced = false;
            while !stop.load(Ordering::Acquire) {
                sequence += 1;
                activity_blocking(&mut writer, run, sequence);
                written.fetch_add(1, Ordering::AcqRel);
                if !announced {
                    announced = true;
                    started_tx.send(()).expect("announce");
                }
            }
        })
    };
    // Reads begin only once records are landing, and continue until more have landed meanwhile.
    started_rx.recv().expect("the writer started");
    let begun = written.load(Ordering::Acquire);
    while written.load(Ordering::Acquire) < begun + target {
        read();
    }
    stop.store(true, Ordering::Release);
    handle.join().expect("the writer finished");
}

#[test]
fn continuous_run_activity_does_not_starve_a_project_query() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, other) = shared_blocking(directory.path());
    let run = start_run_blocking(&mut app);
    let mut views = 0;
    read_during_writes_blocking(run, other, 25, || {
        let view = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
        assert_eq!(project_basis(&view).history_head, 1);
        assert_eq!(view.basis.runs, None);
        views += 1;
    });
    assert!(views > 0);
    assert!(attach_blocking(&mut app).watermark.runs.activity_head >= 25);
}

#[test]
fn continuous_run_activity_does_not_starve_a_run_query_and_its_anchor_is_a_lower_bound() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, other) = shared_blocking(directory.path());
    let run = start_run_blocking(&mut app);
    let mut anchored = Vec::new();
    read_during_writes_blocking(run, other, 25, || {
        let view = view_blocking(&mut app, json!({"query": "run", "id": run}));
        let recorded = view.envelope.data["activity"]["recorded"]
            .as_u64()
            .expect("recorded");
        // The anchor never exceeds what the view holds: a poll from it cannot have a gap.
        assert!(run_basis(&view).activity_head <= recorded);
        anchored.push((run_basis(&view).activity_head, recorded));
    });
    assert!(!anchored.is_empty());

    // Following from the first anchor delivers every record after it exactly once, in order.
    let first = anchored[0].0;
    let mut client = Consumer::seeded(&Watermark {
        project: attach_blocking(&mut app).watermark.project,
        runs: dpm_model::RunFeedHeads {
            activity_head: first,
            ..attach_blocking(&mut app).watermark.runs
        },
    });
    client.drain_blocking(&mut app, 7);
    let expected: Vec<u64> =
        (first + 1..=attach_blocking(&mut app).watermark.runs.activity_head).collect();
    assert_eq!(client.activity, expected);
}

#[test]
fn a_response_delayed_past_a_change_it_lacks_cannot_make_a_stale_consumer_current() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);
    let run = start_run_blocking(&mut app);

    // Old views of the project and of the runs are requested, and their responses are delayed.
    let old_status = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    let old_runs = view_blocking(&mut app, json!({"query": "runs"}));
    let mut client = Consumer::from_views(&[&old_status, &old_runs]);

    // Meanwhile a project operation and run activity arrive, and the client polls them.
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    activity_blocking(&mut other, run, 1);
    client.poll_blocking(&mut app, 100);
    assert!(client.project_stale() && client.runs_stale());

    // The delayed responses arrive and are installed. They were anchored before what the poll saw,
    // so neither store becomes current, and empty polls afterwards do not change that.
    client.install(&[old_status.clone(), old_runs.clone()]);
    assert!(client.project_stale() && client.runs_stale());
    for _ in 0..3 {
        client.poll_blocking(&mut app, 100);
        assert!(client.project_stale() && client.runs_stale());
    }

    // Only views anchored at or beyond what was seen make it current, each store on its own.
    client
        .refresh_all_blocking(
            &mut app,
            &[json!({"query": "status", "probabilistic": false})],
            None,
        )
        .expect("project refresh");
    assert!(!client.project_stale() && client.runs_stale());
    client
        .refresh_all_blocking(&mut app, &[json!({"query": "runs"})], None)
        .expect("run refresh");
    assert!(!client.dirty());
}

#[test]
fn composite_startup_is_seeded_from_the_earliest_anchor_so_no_change_is_skipped() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);

    // The older view lacks an operation that the newer view, read after it, already holds.
    let older = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    let newer = view_blocking(
        &mut app,
        json!({"query": "next", "capabilities": [], "probabilistic": false, "limit": 5}),
    );
    assert_eq!(project_basis(&older).history_head, 1);
    assert_eq!(project_basis(&newer).history_head, 2);

    // Whichever order the client lists them in, it is seeded from the earlier anchor, so the first
    // poll delivers the operation the older view lacks; the newer view's own position would have
    // skipped it and left the older view silently wrong.
    for held in [[&newer, &older], [&older, &newer]] {
        let mut client = Consumer::from_views(&held);
        client.poll_blocking(&mut app, 100);
        assert_eq!(client.project, [2]);
        assert!(client.project_stale());
        // Installing both is not enough either: the older one is behind what the poll saw.
        client.install(&[newer.clone(), older.clone()]);
        assert!(client.project_stale());
        client
            .refresh_all_blocking(
                &mut app,
                &[
                    json!({"query": "status", "probabilistic": false}),
                    json!({"query": "next", "capabilities": [], "probabilistic": false, "limit": 5}),
                ],
                None,
            )
            .expect("refresh");
        assert!(!client.dirty());
    }
}

#[test]
fn views_anchored_under_different_histories_are_not_composed() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, _) = shared_blocking(directory.path());
    let live = view_blocking(&mut app, json!({"query": "status", "probabilistic": false}));
    let archive = directory.path().join("archive.sqlite");
    app.backup_blocking(&archive).expect("backup");
    let mut archived = Application::open_blocking(&archive).expect("archive");
    let mut foreign = view_blocking(
        &mut archived,
        json!({"query": "status", "probabilistic": false}),
    );
    // Pretend the second view was read under another lineage, as after a restore.
    foreign.basis.project = foreign.basis.project.map(|basis| ProjectWatermark {
        lineage_id: Some(dpm_model::LineageId::new()),
        ..basis
    });
    let client = Consumer::from_views(&[&live, &foreign]);
    assert!(
        client.cursors.project.is_none(),
        "no position is invented across histories"
    );
    assert!(client.project_stale());
}

#[test]
fn an_old_response_installed_after_a_fresh_one_makes_the_display_stale_again() {
    let directory = tempfile::tempdir().expect("directory");
    let (mut app, mut other) = shared_blocking(directory.path());
    let task = task_blocking(&app);
    let run = start_run_blocking(&mut app);
    let status = json!({"query": "status", "probabilistic": false});
    let runs = json!({"query": "runs"});

    // Old views are requested; then a project operation and run activity arrive and are polled.
    let old_status = view_blocking(&mut app, status.clone());
    let old_runs = view_blocking(&mut app, runs.clone());
    let mut client = Consumer::from_views(&[&old_status, &old_runs]);
    commit_blocking(
        &mut other,
        &worker(),
        Command::Start {
            work: task,
            occurred_at: None,
        },
    );
    activity_blocking(&mut other, run, 1);
    client.poll_blocking(&mut app, 100);

    // A fresh refresh is installed first and the display is current.
    client
        .refresh_all_blocking(&mut app, &[status, runs], None)
        .expect("refresh");
    assert!(!client.dirty());

    // Then the delayed old responses arrive out of order. The installed set is what is displayed,
    // so the display is stale again; an empty poll does not hide it.
    client.install(&[old_status]);
    assert!(client.project_stale() && !client.runs_stale());
    client.install(&[old_runs]);
    assert!(client.project_stale() && client.runs_stale());
    client.poll_blocking(&mut app, 100);
    assert!(client.project_stale() && client.runs_stale());
}

#[test]
fn two_previews_with_no_lineage_are_told_apart_by_their_workspace() {
    let first = Application::preview(fixture_plan());
    let mut second_plan = fixture_plan();
    second_plan.workspace.id = dpm_model::WorkspaceId::new();
    let second = Application::preview(second_plan);
    let (mut first, mut second) = (first.expect("preview"), second.expect("preview"));
    let query = json!({"query": "status", "probabilistic": false});
    let a = view_blocking(&mut first, query.clone());
    let b = view_blocking(&mut second, query);
    assert_eq!(project_basis(&a).lineage_id, project_basis(&b).lineage_id);
    assert_ne!(
        project_basis(&a).workspace_id,
        project_basis(&b).workspace_id
    );

    // Equal lineages (both none) must not compose two different workspaces.
    let client = Consumer::from_views(&[&a, &b]);
    assert!(client.cursors.project.is_none() && client.project_stale());

    // And a view of one preview never covers what was observed of the other.
    let mut watching = Consumer::from_views(&[&a]);
    watching.poll_blocking(&mut first, 100);
    watching.install(&[b]);
    assert!(watching.project_stale());
}
