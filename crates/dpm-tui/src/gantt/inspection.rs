use super::*;
use crate::theme::Role;
use ratatui::{
    Frame,
    style::Modifier,
    text::{Line, Text},
};

/// Dim tree connector that introduces each group of relations under the inspected work.
const BRANCH: &str = "  ⎿  ";
/// Indent of the values listed under a branch, aligned with its label.
const VALUE: &str = "     ";

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
            self.theme.style(Role::Text).add_modifier(Modifier::BOLD),
        )];
        let progress = self.progress.get(&work.id).copied().unwrap_or_default();
        let state = if work.kind == WorkKind::Milestone {
            if progress.verified {
                "Milestone reached".into()
            } else {
                "Milestone pending".into()
            }
        } else {
            format!("{:?}", work.execution.status)
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
        let gates = release::decision_gates(plan, &self.timeline, work.id);
        if !gates.is_empty() {
            lines.push(Line::from(vec![
                self.label("decision gates"),
                Span::styled(gates.join(", "), self.theme.style(Role::Gate)),
            ]));
        }
        for note in [
            "FS finish→start · SS start→start · FF finish→finish · SF start→finish",
            "+lag delay / -lag lead (hours). Each link shows its release at this snapshot's clock.",
        ] {
            lines.push(Line::styled(note, self.theme.style(Role::Dim)));
        }
        let title = if self.inspecting {
            "Inspector · focus"
        } else {
            "Inspector"
        };
        self.inspector.render(
            frame,
            area,
            Text::from(lines),
            title,
            self.inspecting,
            self.theme,
        );
    }

    /// A branch label: the tree connector and its name in the dim role, then a gap before values.
    fn label(&self, name: &str) -> Span<'static> {
        Span::styled(format!("{BRANCH}{name}  "), self.theme.style(Role::Dim))
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
        let (role, name) = if incoming {
            (Role::RelationPredecessor, "predecessors ←")
        } else {
            (Role::RelationSuccessor, "successors →")
        };
        lines.push(Line::from(vec![
            self.label(name),
            Span::raw(if links.is_empty() {
                "none".to_owned()
            } else {
                format!("({})", links.len())
            }),
        ]));
        for link in links {
            let Some(related) = plan.work_items.get(&if incoming {
                link.predecessor
            } else {
                link.successor
            }) else {
                continue;
            };
            let from = dependencies::label(plan, link.predecessor);
            let to = dependencies::label(plan, link.successor);
            lines.push(Line::styled(
                format!(
                    "{VALUE}{from} --{}{:+.1}h→{to}{}",
                    dependencies::abbreviation(link.kind),
                    link.lag_hours,
                    dependencies::tags(link)
                ),
                self.theme.style(role),
            ));
            lines.push(Line::from(format!(
                "{VALUE}  {}",
                release::state(plan, &self.timeline, link)
            )));
            lines.push(Line::from(format!(
                "{VALUE}{} — {}",
                related.key, related.title
            )));
        }
    }
}

#[cfg(test)]
mod tests;
