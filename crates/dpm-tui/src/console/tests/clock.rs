//! Clock-only changes reach the screen through the console's own tick and event path.

use super::*;
use chrono::{DateTime, TimeDelta, TimeZone, Utc};
use dpm_engine::{Command, apply_command, explain_work};
use dpm_model::{ActorId, DependencyKind, OperationId, WorkItemId};

fn t0() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 9, 1, 9, 0, 0)
        .single()
        .expect("fixed time")
}

fn at(seconds: i64) -> DateTime<Utc> {
    t0() + TimeDelta::seconds(seconds)
}

/// TEST-B may start half an hour after TEST-A starts; TEST-A started at `t0`, revision 2.
fn lagged() -> (Plan, WorkItemId) {
    let mut plan = fixture();
    plan.decisions.clear();
    let a = plan.find_work_by_key("TEST-A").expect("work").id;
    let b = plan.find_work_by_key("TEST-B").expect("work").id;
    let edge = plan
        .dependencies
        .iter_mut()
        .find(|e| e.predecessor == a && e.successor == b)
        .expect("edge");
    edge.kind = DependencyKind::StartStart;
    edge.lag_hours = 0.5;
    let agent = ActorId::agent("maintainer");
    for command in [
        Command::Claim { work: a },
        Command::Start {
            work: a,
            occurred_at: None,
        },
    ] {
        apply_command(&mut plan, agent.clone(), command, t0(), OperationId::new())
            .expect("accepted");
    }
    (plan, b)
}

/// A console showing the lagged plan, evaluated 10 s before TEST-B's release.
struct Console {
    view: View,
    watch: Watch,
    source: Scripted,
    start: Instant,
    shown: Snapshot,
}

impl Console {
    fn open() -> Self {
        let (plan, b) = lagged();
        assert!(!explain_work(&plan, b, at(1799)).expect("explain").ready);
        assert!(explain_work(&plan, b, at(1800)).expect("explain").ready);
        let shown = snapshot(&plan, plan.revision, LineageId::new());
        let start = Instant::now();
        Self {
            view: View::new(&shown.plan, at(1790)).expect("view"),
            watch: Watch::new(shown.revision(), start),
            source: Scripted::at(shown.clone()),
            start,
            shown,
        }
    }

    /// One loop iteration `probes` intervals after opening, with the wall clock at `seconds`.
    fn tick(&mut self, probes: u32, seconds: i64) {
        let now = self.start + watch::INTERVAL * probes;
        tick_blocking(
            &mut self.view,
            &mut self.watch,
            &mut self.source,
            now,
            at(seconds),
        );
    }

    fn key(&mut self, code: KeyCode, seconds: i64) {
        let event = press(code);
        let quit = handle_event(
            &mut self.view,
            &mut self.watch,
            &mut self.source,
            event,
            at(seconds),
        );
        assert!(!quit);
    }

    fn now_page(&mut self) -> String {
        let page = self.view.page;
        self.view.page = Page::Now;
        let text = screen(&mut self.view, 120, 40);
        self.view.page = page;
        text
    }
}

fn lines_from(text: &str, title: &str) -> String {
    let (_, rest) = text.split_once(title).expect("panel title");
    let (_, caption) = rest.split_once("lines ").expect("scroll caption");
    caption.split('–').next().unwrap_or_default().to_owned()
}

#[test]
fn elapsed_lag_becomes_ready_at_its_release_without_a_new_revision() {
    let mut console = Console::open();
    assert_eq!(console.view.evaluation_due(), at(1800));
    assert!(console.now_page().contains("Ready 0 "));
    console.key(KeyCode::Char('2'), 1791);
    while console.view.selected_key().as_deref() != Some("TEST-B") {
        console.key(KeyCode::Down, 1791);
    }
    console.key(KeyCode::Char('4'), 1792);
    screen(&mut console.view, 70, 12);
    console.key(KeyCode::PageDown, 1793);
    let before = screen(&mut console.view, 70, 12);
    assert_ne!(lines_from(&before, "Detail"), "1", "{before}");

    console.tick(0, 1799);
    assert_eq!(
        console.view.evaluated_at(),
        at(1790),
        "not due before the release"
    );
    console.tick(1, 1800);
    assert_eq!(console.view.evaluated_at(), at(1800));
    // Same place: page, selection and Detail scroll survive; Now shares the text panel, so it is
    // read only after.
    assert!(matches!(console.view.page, Page::Detail));
    assert_eq!(console.view.selected_key().as_deref(), Some("TEST-B"));
    let after = screen(&mut console.view, 70, 12);
    assert_eq!(lines_from(&after, "Detail"), lines_from(&before, "Detail"));
    let now = console.now_page();
    assert!(
        now.contains("Ready 1 ") && now.contains("TEST-B  "),
        "{now}"
    );
    assert!(now.contains("revision 2 at 2026-09-01 09:30:00Z"), "{now}");

    console.tick(1, 1801);
    assert_eq!(
        console.view.evaluated_at(),
        at(1800),
        "bounded: no rerun every tick"
    );
    assert!(console.now_page().contains("Ready 1 "));
    assert_eq!(console.view.evaluation_due(), at(1860));

    // Same snapshot: nothing was loaded and no lifecycle moved.
    assert_eq!((console.source.probes, console.source.loads), (1, 0));
    assert_eq!(console.view.revision(), console.shown.plan.revision);
    console.view.page = Page::Work;
    let work = screen(&mut console.view, 120, 20);
    assert!(
        work.contains("TEST-A  InProgress") && work.contains("TEST-B  Planned"),
        "{work}"
    );
}

#[test]
fn the_gantt_inspector_keeps_focus_and_scroll_across_a_clock_refresh() {
    let mut console = Console::open();
    console.key(KeyCode::Char('5'), 1791);
    console.key(KeyCode::Tab, 1791);
    screen(&mut console.view, 100, 30);
    console.key(KeyCode::Down, 1792);
    console.key(KeyCode::Down, 1792);
    let before = screen(&mut console.view, 100, 30);
    assert_eq!(lines_from(&before, "Inspector FOCUS"), "3", "{before}");
    console.tick(1, 1800);
    assert_eq!(console.view.evaluated_at(), at(1800));
    let after = screen(&mut console.view, 100, 30);
    assert_eq!(lines_from(&after, "Inspector FOCUS"), "3", "{after}");
    console.key(KeyCode::Up, 1801);
    let moved = screen(&mut console.view, 100, 30);
    assert_eq!(lines_from(&moved, "Inspector FOCUS"), "2", "{moved}");
}

/// Ways the source can stop matching the display; each must outlive a clock refresh.
fn unsettle(case: usize, source: &mut Scripted, shown: &Snapshot) -> &'static str {
    let lineage = shown.lineage_id.expect("lineage");
    match case {
        0 => {
            source.probe = Err("database is locked".into());
            "Cannot check for changes: database is locked"
        }
        1 => {
            source.commit(snapshot(&shown.plan, 1, lineage));
            "Source went back to revision 1 of the same history"
        }
        2 => {
            source.commit(snapshot(&shown.plan, 9, LineageId::new()));
            "Source now holds a different history"
        }
        _ => {
            let mut other = snapshot(&shown.plan, 3, lineage);
            other.plan.workspace.id = dpm_model::WorkspaceId::new();
            source.commit(other);
            "workspace identity changed"
        }
    }
}

#[test]
fn a_clock_refresh_keeps_every_source_notice_and_loads_nothing() {
    for case in 0..4 {
        let mut console = Console::open();
        let expected = unsettle(case, &mut console.source, &console.shown);
        console.tick(1, 1795);
        let loads = console.source.loads;
        let reported = console.now_page();
        assert!(reported.contains(expected), "{reported}");
        assert!(reported.contains("Ready 0 "));

        console.tick(2, 1800);
        let refreshed = console.now_page();
        assert!(refreshed.contains("Ready 1 "), "{refreshed}");
        assert!(refreshed.contains(expected), "notice kept: {refreshed}");
        let label = ["UNCHECKED", "STALE", "STALE", "STALE"][case];
        assert!(refreshed.contains(&format!("{label} snapshot revision 2 at")));
        assert_eq!(
            console.source.loads, loads,
            "the clock never reads the source"
        );
        assert_eq!(console.view.revision(), 2);
    }
}

#[test]
fn a_commit_due_with_the_release_is_followed_once_and_evaluated_at_that_time() {
    let mut console = Console::open();
    let mut next = snapshot(
        &console.shown.plan,
        3,
        console.shown.lineage_id.expect("id"),
    );
    let b = next.plan.find_work_by_key("TEST-B").expect("work").id;
    next.plan.work_items.get_mut(&b).expect("work").title = "Retitled elsewhere".into();
    console.source.commit(next);
    console.tick(1, 1800);
    assert_eq!((console.source.loads, console.view.revision()), (1, 3));
    assert_eq!(console.view.evaluated_at(), at(1800));
    assert!(
        !console.view.reevaluate_if_due(at(1800)),
        "the reload already evaluated"
    );
    let now = console.now_page();
    assert!(
        now.contains("Ready 1 ") && now.contains("Retitled elsewhere"),
        "{now}"
    );
    assert!(
        !now.contains("STALE") && !now.contains("UNCHECKED"),
        "{now}"
    );
}

#[test]
fn an_explicit_reload_and_a_backward_clock_use_the_injected_time() {
    let mut console = Console::open();
    console.key(KeyCode::Char('r'), 1805);
    assert_eq!(console.source.loads, 1);
    assert_eq!(console.view.evaluated_at(), at(1805));
    assert!(console.now_page().contains("Ready 1 "));
    // A clock set back before the evaluation is not trusted to still match it.
    console.tick(0, 1700);
    assert_eq!(console.view.evaluated_at(), at(1700));
    assert!(console.now_page().contains("Ready 0 "));
    assert_eq!(console.view.evaluation_due(), at(1760));
}
