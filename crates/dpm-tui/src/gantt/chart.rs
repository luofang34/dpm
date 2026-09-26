use super::*;

/// Bar text for work the remaining schedule omits because it is outside the active graph.
const OUTSIDE_GRAPH: &str = " outside the active graph; see detail";
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Color, Modifier},
    text::Line,
    widgets::{Block, Paragraph},
};

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
        let block = Block::bordered().title("Gantt · remaining hours · [?] Help");
        let inner = block.inner(area);
        frame.render_widget(block, area);
        if work.is_empty() && inner.width >= 36 && inner.height > 0 {
            frame.render_widget(Paragraph::new("No work in this workspace."), inner);
            return;
        }
        if inner.width < 36 || inner.height < 10 {
            frame.render_widget(
                Paragraph::new("Gantt preview needs at least 38 columns and 12 rows."),
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
        let parts = Layout::vertical([
            Constraint::Min(2),
            Constraint::Length((inner.height / 3).clamp(5, 12)),
            Constraint::Length(3),
        ])
        .split(inner);
        let current = selected.unwrap_or(0).min(work.len() - 1);
        let focus = self.hovered.unwrap_or(current).min(work.len() - 1);
        self.render_chart(frame, parts[0], plan, work, current, focus);
        self.render_inspection(frame, parts[1], plan, &work[focus]);
        let footer = vec![
            self.palette.legend(),
            Line::from(vec![
                Span::styled("<P Predecessor  ", self.palette.style(Color::Cyan)),
                Span::styled(">S Successor  ", self.palette.style(Color::Magenta)),
                Span::raw("@ Selected  ~ Hover  · direct links to inspected work"),
            ]),
            Line::from(
                "↑↓ Select  ←→ Pan  +/- Zoom  f Fit  Tab Inspect  PgUp/PgDn Scroll  Enter Detail  ? Help",
            ),
        ];
        frame.render_widget(Paragraph::new(footer), parts[2]);
    }

    fn render_chart(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        plan: &Plan,
        work: &[WorkItem],
        current: usize,
        focus: usize,
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
        let left = format!("{:.1}h", self.viewport.start);
        let right = format!("{:.1}h", self.viewport.end());
        let space = width.saturating_sub(left.len() + right.len());
        let label = truncate(&format!("Work {}/{}", current + 1, work.len()), label_width);
        let mut lines = vec![Line::from(format!(
            "{label:<label_width$}    % |{left}{}{right}",
            " ".repeat(space)
        ))];
        for (index, item) in work.iter().enumerate().skip(self.first_row).take(count) {
            let relation = dependencies::relation(plan, work[focus].id, item.id);
            let marker = if self.hovered == Some(index) {
                "~ "
            } else if index == current {
                "@ "
            } else {
                relation.marker()
            };
            lines.push(self.row(plan, item, marker, relation.color(), label_width, width));
        }
        frame.render_widget(Paragraph::new(lines), area);
    }

    fn row(
        &self,
        plan: &Plan,
        item: &WorkItem,
        marker: &str,
        relation: Color,
        label_width: usize,
        width: usize,
    ) -> Line<'static> {
        let bounds = self.bounds(plan, item);
        let critical = bounds.is_some_and(|b| b.2);
        let progress = self.progress[&item.id];
        let (symbol, bar_style) = self.palette.activity(item, progress.verified, critical);
        let selected = marker == "@ ";
        let emphasis = if selected {
            Modifier::BOLD | Modifier::REVERSED
        } else if marker == "~ " {
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
        let label_style = self.palette.style(relation).add_modifier(emphasis);
        let label = format!(
            "{label}{} {:>3.0}% |",
            " ".repeat(label_width.saturating_sub(Span::raw(&label).width())),
            progress.percent_complete
        );
        Line::from(vec![
            Span::styled(label, label_style),
            Span::styled(
                match bounds {
                    Some((start, end, _)) => self.bar(start, end, width, symbol, item.kind),
                    None => format!("{:<width$}", truncate(OUTSIDE_GRAPH, width)),
                },
                bar_style.add_modifier(if selected || marker == "~ " {
                    Modifier::BOLD
                } else {
                    Modifier::empty()
                }),
            ),
            Span::raw("|"),
        ])
    }
}
