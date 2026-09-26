use dpm_model::{WorkItem, WorkKind, WorkStatus};
use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};

#[derive(Clone, Copy)]
pub(super) struct Palette {
    pub(super) enabled: bool,
}
impl Palette {
    pub(super) fn style(self, color: Color) -> Style {
        Style::default().fg(if self.enabled { color } else { Color::Reset })
    }
    pub(super) fn activity(self, work: &WorkItem, verified: bool, critical: bool) -> (char, Style) {
        let (symbol, color) = match work.kind {
            WorkKind::Milestone if verified => ('◆', Color::Green),
            WorkKind::Milestone => ('◇', Color::Yellow),
            WorkKind::WorkPackage => ('-', Color::Gray),
            WorkKind::Task if verified => ('.', Color::Green),
            WorkKind::Task if work.status == WorkStatus::Blocked => ('!', Color::Yellow),
            WorkKind::Task if work.status == WorkStatus::Submitted => ('?', Color::Magenta),
            WorkKind::Task if critical => ('#', Color::Red),
            WorkKind::Task => ('=', Color::Cyan),
        };
        (symbol, self.style(color))
    }
    pub(super) fn legend(self) -> Line<'static> {
        Line::from(vec![
            Span::styled("# Critical  ", self.style(Color::Red)),
            Span::styled("! Blocked  ", self.style(Color::Yellow)),
            Span::styled("? Review  ", self.style(Color::Magenta)),
            Span::styled(". Verified  ", self.style(Color::Green)),
            Span::styled("= Task  ", self.style(Color::Cyan)),
            Span::raw("- Package  ◇/◆ [M] Milestone"),
        ])
    }
}
