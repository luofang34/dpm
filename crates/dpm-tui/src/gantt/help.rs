//! The `?` overlay: the key table the footer hints come from, the glyph legend and what the chart's
//! symbols and colours mean.

use super::*;
use crate::theme::Role;
use ratatui::{
    Frame,
    style::Modifier,
    text::{Line, Text},
};

impl Gantt {
    pub(super) fn render_help(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let heading = self.theme.style(Role::Text).add_modifier(Modifier::BOLD);
        let mut lines = vec![Line::styled("Gantt keyboard and mouse", heading)];
        lines.extend(crate::shortcuts::help().map(Line::from));
        lines.extend([
            Line::from(
                "  mouse      hover a row: temporary title/link preview. Click a row: select it. Wheel over chart: select rows; over inspector: scroll text.",
            ),
            Line::from(
                "Use keyboard selection if the terminal does not report mouse motion. Terminal text selection usually requires Shift while mouse capture is active.",
            ),
            Line::from(""),
            Line::styled("Legend", heading),
        ]);
        lines.extend(palette::legend(self.theme));
        lines.extend([
            Line::from(
                "Name badge [M] means Milestone: ◇[M] pending, ◆[M] reached. It remains visible when the time point is outside the chart window.",
            ),
            Line::from(
                "Status wins over the critical shape: verified, blocked, submitted for review, then critical. The inspector states critical=true/false separately.",
            ),
            Line::styled(
                "<P cyan labels: direct predecessors of the inspected task.",
                self.theme.style(Role::RelationPredecessor),
            ),
            Line::styled(
                ">S magenta labels: direct successors of the inspected task.",
                self.theme.style(Role::RelationSuccessor),
            ),
            Line::from(
                "❯ reversed/bold: keyboard selection. ~ underline: mouse preview. Bar colors describe status; label colors describe relationships.",
            ),
            Line::from(
                "Every relationship remains listed even when its task row or time lies outside the chart window. Scroll the inspector to read all links and titles.",
            ),
            Line::from(dependencies::LEGEND),
            Line::from(
                "+lag delays / -lag leads the temporal bound. Dependencies are schedule constraints; they do not grant execution authorization.",
            ),
            Line::from(
                "NO_COLOR=1 disables colors. Bar shapes, symbols, direction arrows, explicit states and selection emphasis remain available.",
            ),
            Line::from("Navigation is read-only. Press r to reload a fresh snapshot."),
            Line::styled(
                "End of help · Esc or ? returns to the chart",
                self.theme.style(Role::Dim),
            ),
        ]);
        self.help_panel
            .render(frame, area, Text::from(lines), "Help", true, self.theme);
    }
}
