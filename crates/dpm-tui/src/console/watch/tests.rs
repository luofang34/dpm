use super::*;
use crate::console::tests::{Scripted, fixture, screen, snapshot};
use crate::view::Page;
use crossterm::event::KeyCode;
use dpm_model::LineageId;

fn at(revision: u64, lineage_id: Option<LineageId>) -> SourceRevision {
    SourceRevision {
        revision,
        lineage_id,
    }
}

#[test]
fn only_a_later_revision_of_the_displayed_lineage_is_followed() {
    let lineage = Some(LineageId::new());
    let shown = at(4, lineage);
    assert_eq!(classify(shown, at(4, lineage)), Change::Current);
    assert_eq!(classify(shown, at(5, lineage)), Change::Advanced);
    assert_eq!(classify(shown, at(3, lineage)), Change::Diverged);
    assert_eq!(
        classify(shown, at(9, Some(LineageId::new()))),
        Change::Diverged
    );
    assert_eq!(classify(shown, at(9, None)), Change::Diverged);
    assert_eq!(classify(at(0, None), at(1, None)), Change::Advanced);
}

fn caption(text: &str) -> String {
    let start = text.find("lines ").expect("scroll caption");
    text[start..].chars().take_while(|c| *c != '/').collect()
}

#[test]
fn a_committed_change_appears_after_one_interval_keeping_page_selection_and_scroll() {
    let plan = fixture();
    let lineage = LineageId::new();
    let shown = snapshot(&plan, 0, lineage);
    let mut view = View::new(&shown.plan, chrono::Utc::now()).expect("view");
    let start = Instant::now();
    let mut watch = Watch::new(shown.revision(), start);
    let mut source = Scripted::at(shown);
    view.move_selection(1);
    assert!(!view.handle_key(KeyCode::Char('4')));
    screen(&mut view, 70, 16);
    assert!(!view.handle_key(KeyCode::PageDown));
    let before = screen(&mut view, 70, 16);
    assert!(!caption(&before).starts_with("lines 1–"), "{before}");

    let mut changed = snapshot(&plan, 1, lineage);
    let key = view.selected_key().expect("selection");
    let id = changed.plan.find_work_by_key(&key).expect("work").id;
    changed.plan.work_items.get_mut(&id).expect("work").title = "Changed by an agent".into();
    source.commit(changed.clone());
    watch.poll_blocking(&mut view, &mut source, start);
    assert_eq!((source.probes, view.revision()), (0, 0), "not due yet");
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL);
    assert_eq!((source.probes, source.loads, view.revision()), (1, 1, 1));
    assert_eq!(watch.shown(), changed.revision());
    assert!(matches!(view.page, Page::Detail));
    assert_eq!(view.selected_key().as_deref(), Some(key.as_str()));
    let after = screen(&mut view, 70, 16);
    assert_eq!(caption(&after), caption(&before), "{after}");
    view.handle_key(KeyCode::Home);
    assert!(screen(&mut view, 70, 40).contains("Changed by an agent"));

    // Unchanged source: probed again, never reloaded.
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL * 2);
    assert_eq!((source.probes, source.loads), (2, 1));
}

#[test]
fn an_older_revision_is_reported_once_and_the_snapshot_stays() {
    let plan = fixture();
    let lineage = LineageId::new();
    let shown = snapshot(&plan, 5, lineage);
    let mut view = View::new(&shown.plan, chrono::Utc::now()).expect("view");
    let start = Instant::now();
    let mut watch = Watch::new(shown.revision(), start);
    let mut source = Scripted::at(shown);
    source.commit(snapshot(&plan, 2, lineage));
    for tick in 1..=3 {
        watch.poll_blocking(&mut view, &mut source, start + INTERVAL * tick);
    }
    assert_eq!((source.probes, source.loads, view.revision()), (3, 0, 5));
    let text = screen(&mut view, 120, 30);
    assert!(text.contains("STALE snapshot revision 5"), "{text}");
    assert!(text.contains("Source went back to revision 2 of the same history"));
    assert!(text.contains("Showing revision 5; [r] load revision 2 instead."));

    // The source returns to the displayed revision: the report no longer holds.
    source.commit(snapshot(&plan, 5, lineage));
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL * 4);
    let text = screen(&mut view, 120, 30);
    assert!(
        !text.contains("STALE") && !text.contains("went back"),
        "{text}"
    );
}

#[test]
fn another_lineage_found_while_loading_is_reported_rather_than_displayed() {
    let plan = fixture();
    let lineage = LineageId::new();
    let shown = snapshot(&plan, 1, lineage);
    let mut view = View::new(&shown.plan, chrono::Utc::now()).expect("view");
    let start = Instant::now();
    let mut watch = Watch::new(shown.revision(), start);
    let mut source = Scripted::at(shown);
    // The probe sees a forward commit, then a restore replaces the source before the load.
    source.probe = Ok(at(2, Some(lineage)));
    let restored = snapshot(&plan, 7, LineageId::new());
    source.snapshot = Ok(restored.clone());
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL);
    assert_eq!((source.loads, view.revision()), (1, 1));
    source.probe = Ok(restored.revision());
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL * 2);
    assert_eq!(source.loads, 1, "a reported source is not loaded again");
    let text = screen(&mut view, 160, 30);
    assert!(text.contains("STALE snapshot revision 1"), "{text}");
    assert!(
        text.contains("Source now holds a different history"),
        "{text}"
    );
    assert!(text.contains(&restored.lineage_id.expect("lineage").to_string()));
}

#[test]
fn failures_are_shown_in_text_and_retried_only_when_the_source_moves() {
    let plan = fixture();
    let lineage = LineageId::new();
    let shown = snapshot(&plan, 0, lineage);
    let mut view = View::new(&shown.plan, chrono::Utc::now()).expect("view");
    let start = Instant::now();
    let mut watch = Watch::new(shown.revision(), start);
    let mut source = Scripted::at(shown);
    source.probe = Err("database is locked".into());
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL);
    let text = screen(&mut view, 120, 30);
    assert!(text.contains("UNCHECKED snapshot revision 0"), "{text}");
    assert!(text.contains("Cannot check for changes: database is locked"));

    let mut other = snapshot(&plan, 1, lineage);
    other.plan.workspace.id = dpm_model::WorkspaceId::new();
    source.commit(other);
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL * 2);
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL * 3);
    assert_eq!((source.loads, view.revision()), (1, 0));
    let text = screen(&mut view, 120, 30);
    assert!(text.contains("Reload failed:") && text.contains("workspace identity changed"));

    source.commit(snapshot(&plan, 2, lineage));
    watch.poll_blocking(&mut view, &mut source, start + INTERVAL * 4);
    assert_eq!((source.loads, view.revision()), (2, 2));
    assert!(!screen(&mut view, 120, 30).contains("Reload failed"));
}
