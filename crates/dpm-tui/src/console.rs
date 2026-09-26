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
    let mut view = View::new(plan)?;
    view.set_colors(std::env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()));
    enable_raw_mode()?;
    let guard = TerminalGuard;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    terminal.clear()?;
    let result = event_loop_blocking(&mut terminal, &mut view);
    drop(guard);
    result
}

fn event_loop_blocking(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    view: &mut View,
) -> Result<(), TuiError> {
    loop {
        terminal.draw(|frame| view.render(frame))?;
        if !event::poll(Duration::from_millis(200))? {
            continue;
        }
        match event::read()? {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                if view.handle_key(key.code) {
                    return Ok(());
                }
            }
            Event::Mouse(mouse) => view.handle_mouse(mouse),
            Event::Resize(_, _) => view.clear_hover(),
            _ => {}
        }
    }
}
