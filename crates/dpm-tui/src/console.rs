use crate::{
    source::{FixedPlan, Snapshot, SnapshotSource},
    view::View,
};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dpm_model::Plan;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{
    io,
    time::{Duration, Instant},
};

mod interrupts;
use interrupts::Interrupts;
mod watch;
use watch::Watch;

/// Longest wait for input before the loop redraws, checks signals and probes the source.
const INPUT_WAIT: Duration = Duration::from_millis(200);

/// Failure to prepare a projection or operate the terminal.
#[derive(Debug, thiserror::Error)]
pub enum TuiError {
    /// A terminal operation failed.
    #[error("terminal operation failed: {0}")]
    Terminal(#[from] io::Error),
    /// The plan cannot produce a consistent projection.
    #[error(transparent)]
    Projection(#[from] dpm_engine::EngineError),
}

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        disable_raw_mode().ok();
        execute!(
            io::stdout(),
            DisableMouseCapture,
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )
        .ok();
    }
}

/// Open a read-only snapshot console until the operator quits.
pub fn run_blocking(plan: &Plan) -> Result<(), TuiError> {
    run_fixed_blocking(plan, false)
}

/// Open a plan-file preview whose source also rejects operations through other adapters.
pub fn run_preview_blocking(plan: &Plan) -> Result<(), TuiError> {
    run_fixed_blocking(plan, true)
}

fn run_fixed_blocking(plan: &Plan, preview: bool) -> Result<(), TuiError> {
    let snapshot = Snapshot {
        plan: plan.clone(),
        lineage_id: None,
    };
    run_following_blocking(snapshot.clone(), preview, &mut FixedPlan(snapshot))
}

/// Run a read-only console on `initial` that follows `source`: forward changes on the displayed
/// lineage appear without a key press; an older revision or another lineage is reported and
/// displayed only after the operator reloads with `r`.
pub fn run_following_blocking(
    initial: Snapshot,
    preview: bool,
    source: &mut impl SnapshotSource,
) -> Result<(), TuiError> {
    let mut view = View::new(&initial.plan, chrono::Utc::now())?;
    view.preview = preview;
    view.set_colors(std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()));
    let mut watch = Watch::new(initial.revision(), Instant::now());
    let interrupts = Interrupts::register()?;
    enable_raw_mode()?;
    let guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    let result = event_loop_blocking(&mut terminal, &mut view, &mut watch, source, &interrupts);
    drop(guard);
    drop(interrupts);
    result
}

fn event_loop_blocking<S: SnapshotSource>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    view: &mut View,
    watch: &mut Watch,
    source: &mut S,
    interrupts: &Interrupts,
) -> Result<(), TuiError> {
    loop {
        // A signal ends the console like the q key, so the guard restores the terminal.
        if interrupts.received() {
            return Ok(());
        }
        tick_blocking(view, watch, source, Instant::now(), chrono::Utc::now());
        terminal.draw(|frame| view.render(frame))?;
        match event::poll(INPUT_WAIT) {
            Ok(true) => {}
            Ok(false) => continue,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        }
        if handle_event(view, watch, source, event::read()?, chrono::Utc::now()) {
            return Ok(());
        }
    }
}

/// Work due between input events at monotonic `now` and wall `clock`: follow the source first,
/// then re-evaluate the displayed snapshot if the clock alone has made its projections stale. A
/// reload just evaluated at `clock`, so the second step then has nothing to do.
fn tick_blocking<S: SnapshotSource>(
    view: &mut View,
    watch: &mut Watch,
    source: &mut S,
    now: Instant,
    clock: chrono::DateTime<chrono::Utc>,
) {
    watch.poll_blocking(view, source, now, clock);
    view.reevaluate_if_due(clock);
}

/// Apply one terminal event read at `clock`; `true` ends the console.
fn handle_event<S: SnapshotSource>(
    view: &mut View,
    watch: &mut Watch,
    source: &mut S,
    event: Event,
    clock: chrono::DateTime<chrono::Utc>,
) -> bool {
    match event {
        Event::Key(key) if key.kind != KeyEventKind::Release => {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return true;
            }
            if key.code == KeyCode::Char('r') {
                watch.accept_blocking(view, source, clock);
                return false;
            }
            view.handle_key(key.code)
        }
        Event::Mouse(mouse) => {
            view.handle_mouse(mouse);
            false
        }
        Event::Resize(_, _) => {
            view.clear_hover();
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests;
