use crate::gantt::dependencies;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, MouseEvent};
use dpm_engine::{
    EngineError, NextWorkCandidate, NextWorkQuery, ProgressProjection, StatusSummary, completion,
    explain_work, next_work, progress, status,
};
use dpm_model::{DecisionStatus, Plan, Timeline, WorkItem, WorkStatus};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::{Modifier, Style},
    widgets::{Block, List, ListItem, ListState, Paragraph},
};

#[derive(Debug, Clone, Copy)]
pub(crate) enum Page {
    Now,
    Work,
    Network,
    Detail,
    Gantt,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Self::Now => "Now",
            Self::Work => "Work",
            Self::Network => "Network",
            Self::Detail => "Detail",
            Self::Gantt => "Gantt",
        }
    }
}

pub(crate) struct View {
    plan: Plan,
    notice: Option<String>,
    pub(crate) preview: bool,
    pub(crate) page: Page,
    work: Vec<WorkItem>,
    state: ListState,
    now: String,
    progress: ProgressProjection,
    timeline: Timeline,
    network: String,
    gantt: crate::gantt::Gantt,
    detail_cache: Option<(usize, String)>,
    text_panel: crate::text_panel::TextPanel,
    /// Clock reading of the last (re)load; every projection in this snapshot uses it.
    clock: DateTime<Utc>,
}

impl View {
    pub(crate) fn new(plan: &Plan, clock: DateTime<Utc>) -> Result<Self, EngineError> {
        let summary = status(plan, true, clock)?;
        let candidates = next_work(plan, &NextWorkQuery::default(), clock)?;
        let now = now_text(plan, &summary, candidates);
        let mut work: Vec<_> = plan.work_items.values().cloned().collect();
        work.sort_by_cached_key(|item| hierarchy_path(plan, item));
        let done = completion(plan, clock);
        for item in &mut work {
            if !item.is_executable() && done.contains(&item.id) {
                item.status = WorkStatus::Verified;
            }
        }
        let network = format!(
            "{}\n\n{}",
            dependencies::LEGEND,
            dependencies::lines(plan, None).join("\n")
        );
        let mut state = ListState::default();
        if !work.is_empty() {
            state.select(Some(0));
        }
        Ok(Self {
            plan: plan.clone(),
            notice: None,
            preview: false,
            page: Page::Now,
            work,
            state,
            now,
            progress: progress(plan, clock)?,
            timeline: Timeline::at(plan, clock),
            network,
            gantt: crate::gantt::Gantt::new(plan, clock)?,
            detail_cache: None,
            text_panel: crate::text_panel::TextPanel::default(),
            clock,
        })
    }

    pub(crate) fn refresh(&mut self, plan: &Plan, clock: DateTime<Utc>) -> Result<(), EngineError> {
        if plan.workspace.id != self.plan.workspace.id {
            return Err(EngineError::InvalidCommand {
                entity: plan.workspace.id.to_string(),
                reason: "workspace identity changed; reopen explicitly".into(),
            });
        }
        let selected = self
            .state
            .selected()
            .and_then(|index| self.work.get(index))
            .map(|w| w.id);
        let mut next = Self::new(plan, clock)?;
        next.page = self.page;
        next.preview = self.preview;
        next.gantt.restore_navigation(&self.gantt);
        next.state.select(
            selected
                .and_then(|id| next.work.iter().position(|w| w.id == id))
                .or_else(|| (!next.work.is_empty()).then_some(0)),
        );
        *self = next;
        Ok(())
    }

    pub(crate) fn reload_failed(&mut self, error: &impl std::fmt::Display) {
        self.notice = Some(format!(
            "Reload failed: {error}. Showing revision {}; [r] retry.",
            self.plan.revision
        ));
    }

    pub(crate) fn set_colors(&mut self, enabled: bool) {
        self.gantt.set_colors(enabled);
    }

    pub(crate) fn handle_mouse(&mut self, event: MouseEvent) {
        if matches!(self.page, Page::Gantt)
            && let Some(index) =
                self.gantt
                    .handle_mouse(event, self.state.selected(), self.work.len())
        {
            self.state.select(Some(index));
            self.text_panel.reset();
        }
    }

    pub(crate) fn clear_hover(&mut self) {
        self.gantt.clear_hover();
    }

    pub(crate) fn handle_key(&mut self, key: KeyCode) -> bool {
        self.clear_hover();
        if matches!(self.page, Page::Gantt) && self.gantt.navigate(key) {
            return false;
        }
        if matches!(self.page, Page::Now | Page::Network | Page::Detail)
            && matches!(
                key,
                KeyCode::PageUp | KeyCode::PageDown | KeyCode::Home | KeyCode::End
            )
        {
            self.text_panel.navigate(key);
            return false;
        }
        if matches!(key, KeyCode::Char('1'..='5') | KeyCode::Enter) {
            self.text_panel.reset();
        }
        match key {
            KeyCode::Char('q') | KeyCode::Esc => return true,
            KeyCode::Char('1') => self.page = Page::Now,
            KeyCode::Char('2') => self.page = Page::Work,
            KeyCode::Char('3') => self.page = Page::Network,
            KeyCode::Char('5') | KeyCode::Char('g') => self.page = Page::Gantt,
            KeyCode::Char('4') | KeyCode::Enter => self.page = Page::Detail,
            KeyCode::Char('j') | KeyCode::Down => self.move_selection(1),
            KeyCode::Char('k') | KeyCode::Up => self.move_selection(-1),
            _ => {}
        }
        false
    }

    pub(crate) fn move_selection(&mut self, direction: i8) {
        if self.work.is_empty() {
            return;
        }
        let current = self.state.selected().unwrap_or(0);
        let index = if direction > 0 {
            current.saturating_add(1).min(self.work.len() - 1)
        } else {
            current.saturating_sub(1)
        };
        self.state.select(Some(index));
        self.text_panel.reset();
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>) {
        let areas = Layout::vertical([
            Constraint::Length(if self.notice.is_some() { 6 } else { 3 }),
            Constraint::Min(1),
        ])
        .split(frame.area());
        frame.render_widget(
            Paragraph::new(format!(
                "DPM · {} · {} revision {}  [1–5] Pages [↑↓/jk] Navigate [r] Reload [q] Quit{}",
                self.page.title(),
                if self.preview {
                    "PREVIEW read-only"
                } else {
                    "snapshot"
                },
                self.plan.revision,
                self.notice
                    .as_ref()
                    .map(|s| format!("\n{s}"))
                    .unwrap_or_default()
            ))
            .wrap(ratatui::widgets::Wrap { trim: false })
            .block(Block::bordered()),
            areas[0],
        );
        if matches!(self.page, Page::Gantt) {
            self.gantt.render(
                frame,
                areas[1],
                &self.plan,
                &self.work,
                self.state.selected(),
            );
            return;
        }
        if matches!(self.page, Page::Work) {
            let items = self.work.iter().map(|w| {
                ListItem::new(format!(
                    "{}{}{}  {:?}  {:.0}%  {}{}",
                    "  ".repeat(depth(&self.plan, w)),
                    crate::work_label::milestone_badge(w, self.progress.work[&w.id].verified),
                    w.key,
                    w.status,
                    self.progress.work[&w.id].percent_complete,
                    w.title,
                    crate::open_choices::scope_tag(
                        self.progress.work[&w.id].scope,
                        self.timeline.applicability(w.id)
                    )
                ))
            });
            let list = List::new(items)
                .block(Block::bordered().title("Work · select then [4] Detail"))
                .highlight_symbol("> ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(list, areas[1], &mut self.state);
            return;
        }
        let text = match self.page {
            Page::Now => self.now.clone(),
            Page::Network => self.network.clone(),
            Page::Detail => self.detail(),
            Page::Work | Page::Gantt => String::new(),
        };
        self.text_panel.render(
            frame,
            areas[1],
            text.into(),
            &format!("{} · PgUp/PgDn scroll", self.page.title()),
            false,
        );
    }

    fn detail(&mut self) -> String {
        if let Some((index, text)) = &self.detail_cache
            && Some(*index) == self.state.selected()
        {
            return text.clone();
        }
        let Some(work) = self.state.selected().and_then(|i| self.work.get(i)) else {
            return "No work in this workspace.".into();
        };
        let text = match explain_work(&self.plan, work.id, self.clock) {
            Ok(explanation) => crate::detail::text(&self.plan, &explanation),
            Err(error) => format!("Cannot explain work: {error}"),
        };
        if let Some(index) = self.state.selected() {
            self.detail_cache = Some((index, text.clone()));
        }
        text
    }
}

fn now_text(plan: &Plan, summary: &StatusSummary, candidates: Vec<NextWorkCandidate>) -> String {
    let mut now = format!(
        "Ready {} · Blocked {} · In flight {} · Awaiting review {} · Complete {} / {} · Decisions {}\nExpected remaining: {:.1}h",
        summary.ready,
        summary.blocked,
        summary.in_flight,
        summary.awaiting_verification,
        summary.complete,
        summary.total_work,
        summary.open_decisions,
        summary.expected_finish_hours
    );
    now.push_str(&format!(
        "\nExecution: {:.1}% · verified={}",
        summary.progress.percent_complete, summary.progress.verified
    ));
    if let (Some(p50), Some(p80)) = (summary.p50_finish_hours, summary.p80_finish_hours) {
        now.push_str(&format!(" · P50 {p50:.1}h · P80 {p80:.1}h"));
    }
    if let Some(p95) = summary.p95_finish_hours {
        now.push_str(&format!(" · P95 {p95:.1}h"));
    }
    now.push_str(&crate::open_choices::summary_text(summary));
    now.push_str("\n\nNeeds decision:\n");
    for decision in plan
        .decisions
        .values()
        .filter(|d| d.status == DecisionStatus::Open)
    {
        now.push_str(&format!("{}: {}\n", decision.key, decision.question));
    }
    for (title, state) in [
        ("Blocked work", WorkStatus::Blocked),
        ("Needs review", WorkStatus::Submitted),
    ] {
        now.push_str(&format!("\n{title}:\n"));
        for work in plan.work_items.values().filter(|w| w.status == state) {
            now.push_str(&format!(
                "{} — {} {}\n",
                work.key,
                work.title,
                work.block_reason.as_deref().unwrap_or("")
            ));
        }
    }
    now.push_str("\nRisks:\n");
    for risk in plan.risks.values() {
        now.push_str(&format!(
            "{} ({:?}): {}\n",
            risk.key, risk.impact, risk.description
        ));
    }
    now.push_str("\nRecommended ready work:\n");
    if candidates.is_empty() {
        now.push_str("No ready work. Inspect Work or Detail for blockers and gates.\n");
    }
    for candidate in candidates {
        now.push_str(&format!(
            "{}  {}  score {:.1}{}\n",
            candidate.work.key,
            candidate.work.title,
            candidate.score,
            if candidate.critical {
                " [critical]"
            } else {
                ""
            }
        ));
    }
    now
}

fn hierarchy_path(plan: &Plan, work: &WorkItem) -> Vec<String> {
    let mut path = vec![work.key.to_string()];
    let mut parent = work.parent;
    while let Some(item) = parent.and_then(|id| plan.work_items.get(&id)) {
        path.push(item.key.to_string());
        parent = item.parent;
    }
    path.reverse();
    path
}

fn depth(plan: &Plan, work: &WorkItem) -> usize {
    let mut parent = work.parent;
    let mut depth: usize = 0;
    while let Some(id) = parent {
        depth = depth.saturating_add(1);
        parent = plan.work_items.get(&id).and_then(|w| w.parent);
    }
    depth
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
