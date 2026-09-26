use super::*;
use ratatui::{
    Frame,
    style::{Color, Modifier},
    text::{Line, Text},
};

impl Gantt {
    pub(super) fn render_inspection(
        &mut self,
        frame: &mut Frame<'_>,
        area: Rect,
        plan: &Plan,
        work: &WorkItem,
    ) {
        if self.inspected != Some(work.id) {
            self.inspected = Some(work.id);
            self.inspector.reset();
        }
        let heading = if self.hovered.is_some() {
            "Hover"
        } else {
            "Selected"
        };
        let mut lines = vec![Line::styled(
            format!("{heading} {} — {}", work.key, work.title),
            self.palette
                .style(Color::Reset)
                .add_modifier(Modifier::BOLD),
        )];
        let progress = self.progress[&work.id];
        let state = if work.kind == WorkKind::Milestone {
            if progress.verified {
                "Milestone reached".into()
            } else {
                "Milestone pending".into()
            }
        } else {
            format!("{:?}", work.status)
        };
        lines.push(Line::from(format!(
            "{state} · {:.0}% · verified={} · critical={}",
            progress.percent_complete,
            progress.verified,
            self.bounds(plan, work).is_some_and(|b| b.2)
        )));
        let applicability = self.timeline.applicability(work.id);
        if !applicability.is_applicable() {
            lines.push(Line::from(format!(
                "Outside the active graph — {}",
                crate::open_choices::applicability_text(applicability)
            )));
        }
        self.relationships(&mut lines, plan, work, true);
        self.relationships(&mut lines, plan, work, false);
        lines.extend(release::decision_gates(plan, &self.timeline, work.id).map(Line::from));
        lines.push(Line::from(
            "FS finish→start · SS start→start · FF finish→finish · SF start→finish",
        ));
        lines.push(Line::from(
            "+lag delay / -lag lead (hours). Each link shows its release at this snapshot's clock.",
        ));
        let title = if self.inspecting {
            "Inspector FOCUS · Tab chart · ↑↓ scroll"
        } else {
            "Inspector · Tab focus · PgUp/PgDn scroll"
        };
        self.inspector
            .render(frame, area, Text::from(lines), title, self.inspecting);
    }

    fn relationships(
        &self,
        lines: &mut Vec<Line<'static>>,
        plan: &Plan,
        work: &WorkItem,
        incoming: bool,
    ) {
        let links: Vec<_> = plan
            .dependencies
            .iter()
            .filter(|d| {
                if incoming {
                    d.successor == work.id
                } else {
                    d.predecessor == work.id
                }
            })
            .collect();
        let color = if incoming {
            Color::Cyan
        } else {
            Color::Magenta
        };
        let label = if incoming {
            "Predecessors ←"
        } else {
            "Successors →"
        };
        lines.push(Line::styled(
            format!(
                "{label} ({}){}",
                links.len(),
                if links.is_empty() { ": none" } else { "" }
            ),
            self.palette.style(color),
        ));
        for link in links {
            let related = &plan.work_items[&if incoming {
                link.predecessor
            } else {
                link.successor
            }];
            let from = &plan.work_items[&link.predecessor].key;
            let to = &plan.work_items[&link.successor].key;
            lines.push(Line::styled(
                format!(
                    "  {from} --{}{:+.1}h→{to}{}",
                    dependencies::abbreviation(link.kind),
                    link.lag_hours,
                    dependencies::tags(link)
                ),
                self.palette.style(color),
            ));
            lines.push(Line::from(format!(
                "    {}",
                release::state(plan, &self.timeline, link)
            )));
            lines.push(Line::from(format!("  {} — {}", related.key, related.title)));
        }
    }

    pub(super) fn render_help(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let mut lines = vec![
            Line::from("Gantt keyboard and mouse"),
            Line::from(
                "↑/↓ or j/k: select task. ←/→: pan timeline. +/-: zoom. f: fit. Home/End: timeline start/end.",
            ),
            Line::from(
                "Tab: focus inspector or chart. Inspector ↑/↓, j/k, Home/End: scroll full title and links. PgUp/PgDn: inspector pages from either focus.",
            ),
            Line::from(
                "Hover a row: temporary title/link preview. Click a row: select it. Wheel over chart: select rows; over inspector: scroll text.",
            ),
            Line::from(
                "Enter/4: full task Detail. 5/g: Gantt. ? toggles help. Esc: close help or return focus to chart; otherwise quit. q: quit.",
            ),
            Line::from(
                "Use keyboard selection if the terminal does not report mouse motion. Terminal text selection usually requires Shift while mouse capture is active.",
            ),
            Line::from(""),
            self.palette.legend(),
            Line::from(
                "Name badge [M] means Milestone: ◇[M] pending, ◆[M] reached. It remains visible when the time point is outside the chart window.",
            ),
            Line::from(
                "Status wins over critical color: verified, blocked, submitted for review, then critical. The inspector states critical=true/false separately.",
            ),
            Line::styled(
                "<P cyan labels: direct predecessors of the inspected task.",
                self.palette.style(Color::Cyan),
            ),
            Line::styled(
                ">S magenta labels: direct successors of the inspected task.",
                self.palette.style(Color::Magenta),
            ),
            Line::from(
                "@ reversed/bold: keyboard selection. ~ underline: mouse preview. Bar colors describe status; label colors describe relationships.",
            ),
            Line::from(
                "Every relationship remains listed even when its task row or time lies outside the chart window. Scroll the inspector to read all links and titles.",
            ),
            Line::from(dependencies::LEGEND),
            Line::from(
                "+lag delays / -lag leads the temporal bound. Dependencies are schedule constraints; they do not grant execution authorization.",
            ),
            Line::from(
                "NO_COLOR=1 disables colors. Symbols, direction arrows, explicit states and selection emphasis remain available.",
            ),
            Line::from("Navigation is read-only. Press r to reload a fresh snapshot."),
        ];
        lines.push(Line::from("End of help · Esc or ? returns to the chart"));
        self.help_panel.render(
            frame,
            area,
            Text::from(lines),
            "Help · ↑↓/PgUp/PgDn scroll · Esc close",
            true,
        );
    }
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests;
