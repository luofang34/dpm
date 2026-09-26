use crate::view::View;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind},
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use dpm_model::Plan;
use ratatui::{Terminal, backend::CrosstermBackend};
use std::{io, time::Duration};

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
    run_reloading_blocking(plan, false, || {
        Ok::<_, std::convert::Infallible>(plan.clone())
    })
}

/// Open a plan-file preview whose source also rejects operations through other adapters.
pub fn run_preview_blocking(plan: &Plan) -> Result<(), TuiError> {
    run_reloading_blocking(plan, true, || {
        Ok::<_, std::convert::Infallible>(plan.clone())
    })
}

/// Run a read-only console whose reload action obtains a fresh snapshot through its adapter.
pub fn run_reloading_blocking<E: std::fmt::Display>(
    plan: &Plan,
    preview: bool,
    mut reload: impl FnMut() -> Result<Plan, E>,
) -> Result<(), TuiError> {
    let mut view = View::new(plan, chrono::Utc::now())?;
    view.preview = preview;
    view.set_colors(std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()));
    enable_raw_mode()?;
    let guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    let result = event_loop_blocking(&mut terminal, &mut view, &mut reload);
    drop(guard);
    result
}

fn event_loop_blocking<E: std::fmt::Display>(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    view: &mut View,
    reload: &mut impl FnMut() -> Result<Plan, E>,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| view.render(frame))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if key.code == crossterm::event::KeyCode::Char('c')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    return Ok(());
                }
                if key.code == crossterm::event::KeyCode::Char('r') {
                    match reload() {
                        Ok(plan) => {
                            if let Err(error) = view.refresh(&plan, chrono::Utc::now()) {
                                view.reload_failed(&error);
                            }
                        }
                        Err(error) => view.reload_failed(&error),
                    }
                } else if view.handle_key(key.code) {
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => view.handle_mouse(mouse),
            Event::Resize(_, _) => view.clear_hover(),
            _ => {}
        }
    }
}
