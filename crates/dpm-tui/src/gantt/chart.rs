use super::*;

use crate::theme::Role;
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Modifier,
    text::Line,
    widgets::Paragraph,
};

const MIN_INNER_WIDTH: u16 = 36;
/// The inspector's five rows, the chart's axis row and three work rows.
const MIN_INNER_HEIGHT: u16 = 9;
/// Row label prefix of the keyboard selection.
const SELECTED: &str = "❯ ";
/// Row label prefix of the row under the pointer: a preview, never a selection.
const HOVER: &str = "~ ";

impl Gantt {
    pub(crate) fn render(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        plan: &Plan,
        work: &[WorkItem],
        selected: Option<usize>,
    ) {
        self.row_area = Rect::default();
        self.inspector.area = Rect::default();
        let block = self
            .theme
            .pane(!self.inspecting && !self.help)
            .title(" Gantt · hours from now ");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if work.is_empty() && inner.width >= 36 && inner.height > 0 {
            frame.render_widget(Paragraph::new("No work in this workspace."), inner);
            return;
        }
        if inner.width < MIN_INNER_WIDTH || inner.height < MIN_INNER_HEIGHT {
            // The pane shares the terminal with the header, so the size an operator must set is
            // the pane minimum plus whatever surrounds the pane now.
            let frame_area = frame.area();
            let columns = frame_area
                .width
                .saturating_sub(inner.width)
                .saturating_add(MIN_INNER_WIDTH);
            let rows = frame_area
                .height
                .saturating_sub(inner.height)
                .saturating_add(MIN_INNER_HEIGHT);
            frame.render_widget(
                Paragraph::new(format!(
                    "Gantt needs a terminal of at least {columns} columns × {rows} rows."
                ))
                .wrap(ratatui::widgets::Wrap { trim: true }),
                inner,
            );
            return;
        }
        if work.is_empty() {
            frame.render_widget(Paragraph::new("No work in this workspace."), inner);
            return;
        }
        if self.help {
            self.render_help(frame, inner);
            return;
        }
        let [chart, inspection] = Layout::vertical([
            Constraint::Min(2),
            Constraint::Length((inner.height / 3).clamp(5, 12)),
        ])
        .areas(inner);
        let last = work.len().saturating_sub(1);
        let current = selected.unwrap_or(0).min(last);
        let focus = self.hovered.unwrap_or(current).min(last);
        let Some(focused) = work.get(focus) else {
            return;
        };
        self.render_chart(frame, chart, plan, work, current, focused);
        self.render_inspection(frame, inspection, plan, focused);
    }

    fn render_chart(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        plan: &Plan,
        work: &[WorkItem],
        current: usize,
        focused: &WorkItem,
    ) {
        let label_width = (usize::from(area.width) / 3).clamp(14, 40);
        let width = usize::from(area.width).saturating_sub(label_width + 9);
        let visible = usize::from(area.height.saturating_sub(1));
        if current < self.first_row {
            self.first_row = current;
        }
        if current >= self.first_row + visible {
            self.first_row = current.saturating_sub(visible.saturating_sub(1));
        }
        self.first_row = self.first_row.min(work.len().saturating_sub(visible));
        let count = work.len().saturating_sub(self.first_row).min(visible);
        self.row_area = Rect::new(area.x, area.y + 1, area.width, count as u16);
        let label = truncate(&format!("Work {}/{}", current + 1, work.len()), label_width);
        let mut lines = vec![Line::styled(
            format!("{label:<label_width$}    % |{}", self.axis(width)),
            self.theme.style(Role::Dim),
        )];
        for (index, item) in work.iter().enumerate().skip(self.first_row).take(count) {
            let relation = dependencies::relation(plan, focused.id, item.id);
            let marker = if self.hovered == Some(index) {
                HOVER
            } else if index == current {
                SELECTED
            } else {
                relation.marker()
            };
            lines.push(self.row(plan, item, marker, relation.role(), label_width, width));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn row(
        &self,
        plan: &Plan,
        item: &WorkItem,
        marker: &str,
        relation: Role,
        label_width: usize,
        width: usize,
    ) -> Line<'static> {
        let bounds = self.bounds(plan, item);
        let critical = bounds.is_some_and(|b| b.2);
        let progress = self.progress.get(&item.id).copied().unwrap_or_default();
        let (symbol, bar_style) = palette::activity(self.theme, item, progress.verified, critical);
        let selected = marker == SELECTED;
        let emphasis = if selected {
            Modifier::BOLD | Modifier::REVERSED
        } else if marker == HOVER {
            Modifier::UNDERLINED
        } else {
            Modifier::empty()
        };
        let label = truncate(
            &format!(
                "{marker}{}{} {}",
                crate::work_label::milestone_badge(item, progress.verified),
                item.key,
                item.title
            ),
            label_width,
        );
        let label_style = self.theme.style(relation).add_modifier(emphasis);
        let label = format!(
            "{label}{} {:>3.0}% |",
            " ".repeat(label_width.saturating_sub(Span::raw(&label).width())),
            progress.percent_complete
        );
        Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(
                match bounds {
                    Some((start, end, _)) => {
                        self.row_bar(item, (start, end), progress.verified, width, symbol)
                    }
                    None => format!("{:<width$}", truncate(&self.outside_graph(item), width)),
                },
                bar_style.add_modifier(if selected || marker == HOVER {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::raw("|"),
        ])
    }
}

impl Gantt {
    /// The hour axis: the window's first and last hour, and `now` where the clock reading falls
    /// inside it with history before, so the recorded and the remaining sides are told apart.
    fn axis(&self, width: usize) -> String {
        let left = format!("{:.1}h", self.viewport.start);
        let right = format!("{:.1}h", self.viewport.end());
        let mut axis: Vec<char> = format!(
            "{left}{}{right}",
            " ".repeat(width.saturating_sub(left.len() + right.len()))
        )
        .chars()
        .collect();
        if self.viewport.start < 0.0 && self.viewport.end() > 0.0 {
            let at = self.viewport.cell(0.0, width);
            let free = |cell: usize| axis.get(cell).is_some_and(|c| *c == ' ');
            if (at.saturating_sub(1)..at + 4).all(free) {
                for (cell, mark) in axis.iter_mut().skip(at).zip("now".chars()) {
                    *cell = mark;
                }
            }
        }
        axis.into_iter().collect()
    }
    /// Bar text naming why the timeline keeps work out of the remaining schedule.
    fn outside_graph(&self, item: &WorkItem) -> String {
        let state = self.timeline.applicability(item.id);
        format!(
            " outside the active graph: {}; see detail",
            crate::open_choices::applicability_label(state)
        )
    }
}
