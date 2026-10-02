use super::*;
use crate::{source::SourceRevision, view::Page};
use crossterm::event::{KeyEvent, KeyEventState};
use dpm_model::LineageId;
use ratatui::backend::TestBackend;

pub(super) fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

/// A source whose probe and snapshot the test sets, counting how often each is read.
pub(super) struct Scripted {
    pub(super) probe: Result<SourceRevision, String>,
    pub(super) snapshot: Result<Snapshot, String>,
    pub(super) probes: usize,
    pub(super) loads: usize,
}

impl Scripted {
    pub(super) fn at(snapshot: Snapshot) -> Self {
        Self {
            probe: Ok(snapshot.revision()),
            snapshot: Ok(snapshot),
            probes: 0,
            loads: 0,
        }
    }

    /// The source commits: both the probe and the next load see `snapshot`.
    pub(super) fn commit(&mut self, snapshot: Snapshot) {
        self.probe = Ok(snapshot.revision());
        self.snapshot = Ok(snapshot);
    }
}

impl SnapshotSource for Scripted {
    type Error = String;

    fn revision_blocking(&mut self) -> Result<SourceRevision, String> {
        self.probes += 1;
        self.probe.clone()
    }

    fn snapshot_blocking(&mut self) -> Result<Snapshot, String> {
        self.loads += 1;
        self.snapshot.clone()
    }
}

pub(super) fn snapshot(plan: &Plan, revision: u64, lineage_id: LineageId) -> Snapshot {
    let mut plan = plan.clone();
    plan.revision = revision;
    Snapshot {
        plan,
        lineage_id: Some(lineage_id),
    }
}

pub(super) fn screen(view: &mut View, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| view.render(frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| {
            (0..width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn key(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> Event {
    Event::Key(KeyEvent {
        code,
        modifiers,
        kind,
        state: KeyEventState::NONE,
    })
}

/// Deliver `event` read at the system clock, for tests whose outcome does not depend on time.
fn handle_now(view: &mut View, watch: &mut Watch, source: &mut Scripted, event: Event) -> bool {
    handle_event(view, watch, source, event, chrono::Utc::now())
}

pub(super) fn press(code: KeyCode) -> Event {
    key(code, KeyModifiers::NONE, KeyEventKind::Press)
}

#[test]
fn the_r_key_accepts_another_lineage_that_the_loop_only_reported() {
    let plan = fixture();
    let shown = snapshot(&plan, 3, LineageId::new());
    let mut view = View::new(&shown.plan, chrono::Utc::now()).expect("view");
    let start = Instant::now();
    let mut watch = Watch::new(shown.revision(), start);
    let mut source = Scripted::at(shown);
    let restored = snapshot(&plan, 1, LineageId::new());
    source.commit(restored.clone());
    watch.poll_blocking(
        &mut view,
        &mut source,
        start + watch::INTERVAL,
        chrono::Utc::now(),
    );
    assert_eq!((view.revision(), source.loads), (3, 0));
    assert!(screen(&mut view, 120, 30).contains("STALE snapshot revision 3"));

    // A released key is not a press; only the pressed r reloads.
    let release = key(
        KeyCode::Char('r'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    );
    assert!(!handle_now(&mut view, &mut watch, &mut source, release));
    assert_eq!(source.loads, 0);
    assert!(!handle_now(
        &mut view,
        &mut watch,
        &mut source,
        press(KeyCode::Char('r'))
    ));
    assert_eq!((view.revision(), source.loads), (1, 1));
    assert_eq!(watch.shown(), restored.revision());
    let text = screen(&mut view, 120, 30);
    assert!(
        text.contains("snapshot revision 1") && !text.contains("STALE"),
        "{text}"
    );
}

#[test]
fn keys_navigate_and_quit_without_reading_the_source() {
    let plan = fixture();
    let shown = snapshot(&plan, 0, LineageId::new());
    let mut view = View::new(&shown.plan, chrono::Utc::now()).expect("view");
    let mut watch = Watch::new(shown.revision(), Instant::now());
    let mut source = Scripted::at(shown);
    assert!(!handle_now(
        &mut view,
        &mut watch,
        &mut source,
        press(KeyCode::Char('4'))
    ));
    assert!(matches!(view.page, Page::Detail));
    assert!(!handle_now(
        &mut view,
        &mut watch,
        &mut source,
        Event::Resize(80, 24)
    ));
    let interrupt = key(
        KeyCode::Char('c'),
        KeyModifiers::CONTROL,
        KeyEventKind::Press,
    );
    assert!(handle_now(&mut view, &mut watch, &mut source, interrupt));
    assert!(handle_now(
        &mut view,
        &mut watch,
        &mut source,
        press(KeyCode::Char('q'))
    ));
    assert_eq!((source.probes, source.loads), (0, 0));
}

mod clock;
