use crate::gantt::dependencies;
use chrono::{DateTime, Utc};
use crossterm::event::{KeyCode, MouseEvent};
use dpm_engine::{
    EngineError, NextWorkQuery, ProgressProjection, completion, explain_work, next_work, progress,
    status,
};
use dpm_model::{Plan, Timeline, WorkItem, WorkStatus};
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
    notice: Option<crate::notice::ReloadNotice>,
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
    /// Clock reading of the last evaluation; every projection in this snapshot uses it.
    clock: DateTime<Utc>,
    window: evaluation::Window,
}

impl View {
    pub(crate) fn new(plan: &Plan, clock: DateTime<Utc>) -> Result<Self, EngineError> {
        let summary = status(plan, true, clock)?;
        let candidates = next_work(plan, &NextWorkQuery::default(), clock)?;
        let timeline = Timeline::at(plan, clock);
        let now = crate::now::text(plan, &summary, &timeline, candidates);
        let mut work: Vec<_> = plan.work_items.values().cloned().collect();
        let mut paths: Vec<_> = work
            .drain(..)
            .map(|item| (hierarchy_path(plan, &item), item))
            .collect();
        paths.sort_by(|(a, _), (b, _)| a.cmp(b));
        work.extend(paths.into_iter().map(|(_, item)| item));
        let done = completion(plan, clock);
        for item in &mut work {
            if !item.is_executable() && done.contains(&item.id) {
                item.execution.status = WorkStatus::Verified;
            }
        }
        let network = format!(
            "{}\n\n{}",
            dependencies::LEGEND,
            dependencies::lines(plan, None).join("\n")
        );
        let window = evaluation::Window::evaluated(plan, &timeline, clock);
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
            timeline,
            network,
            gantt: crate::gantt::Gantt::new(plan, clock)?,
            detail_cache: None,
            text_panel: crate::text_panel::TextPanel::default(),
            clock,
            window,
        })
    }

    pub(crate) fn refresh(&mut self, plan: &Plan, clock: DateTime<Utc>) -> Result<(), EngineError> {
        if plan.workspace.id != self.plan.workspace.id {
            return Err(EngineError::InvalidCommand {
                entity: plan.workspace.id.to_string(),
                reason: "workspace identity changed; reopen explicitly".into(),
            });
        }
        self.rebuild(plan, clock)
    }

    /// Replace every projection with `plan` evaluated at `clock`, keeping page, selection, scroll
    /// and Gantt navigation; on failure nothing changes. Any notice is cleared.
    fn rebuild(&mut self, plan: &Plan, clock: DateTime<Utc>) -> Result<(), EngineError> {
        let selected = self
            .state
            .selected()
            .and_then(|index| self.work.get(index))
            .map(|w| w.id);
        let mut next = Self::new(plan, clock)?;
        next.page = self.page;
        next.preview = self.preview;
        next.gantt.restore_navigation(&mut self.gantt);
        let kept = selected.and_then(|id| next.work.iter().position(|w| w.id == id));
        next.state
            .select(kept.or_else(|| (!next.work.is_empty()).then_some(0)));
        *next.state.offset_mut() = self.state.offset();
        // Text scroll belongs to the selected work's Detail, or to the page, and survives a
        // refresh; rendering clamps it to the new text. Another selection starts at the top.
        if kept.is_some() || selected.is_none() {
            next.text_panel = std::mem::take(&mut self.text_panel);
        }
        *self = next;
        Ok(())
    }

    pub(crate) fn reload_failed(&mut self, error: &impl std::fmt::Display) {
        self.notice = Some(crate::notice::ReloadNotice::failed(
            &error.to_string(),
            self.plan.revision,
        ));
    }

    /// Stand or clear a notice about the source; the displayed snapshot stays unchanged.
    pub(crate) fn set_notice(&mut self, notice: Option<crate::notice::ReloadNotice>) {
        self.notice = notice;
    }

    #[cfg(test)]
    pub(crate) fn revision(&self) -> u64 {
        self.plan.revision
    }

    #[cfg(test)]
    pub(crate) fn selected_key(&self) -> Option<String> {
        let index = self.state.selected()?;
        self.work.get(index).map(|work| work.key.to_string())
    }

    #[cfg(test)]
    pub(crate) fn inspector_area(&self) -> ratatui::layout::Rect {
        self.gantt.inspector_area()
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

    fn render_header(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let block = Block::bordered();
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let [title, notice_area] =
            Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).areas(inner);
        frame.render_widget(
            Paragraph::new(format!(
                "DPM · {} · {} revision {} at {}  [1–5] Pages [↑↓/jk] Navigate [r] Reload [q] Quit",
                self.page.title(),
                match (self.preview, self.notice.as_ref().and_then(|n| n.label)) {
                    (true, None) => "PREVIEW read-only".into(),
                    (true, Some(label)) => format!("PREVIEW read-only {label}"),
                    (false, None) => "snapshot".into(),
                    (false, Some(label)) => format!("{label} snapshot"),
                },
                self.plan.revision,
                self.evaluated_at().format("%Y-%m-%d %H:%M:%SZ"),
            )),
            title,
        );
        if let Some(notice) = &self.notice {
            notice.render(frame, notice_area);
        }
    }

    pub(crate) fn render(&mut self, frame: &mut Frame<'_>) {
        let header = if self.notice.is_some() {
            3 + crate::notice::ROWS
        } else {
            3
        };
        let [top, body] =
            Layout::vertical([Constraint::Length(header), Constraint::Min(1)]).areas(frame.area());
        self.render_header(frame, top);
        if matches!(self.page, Page::Gantt) {
            self.gantt
                .render(frame, body, &self.plan, &self.work, self.state.selected());
            return;
        }
        if matches!(self.page, Page::Work) {
            let items = self.work.iter().map(|w| {
                let progress = self.progress.work.get(&w.id).copied().unwrap_or_default();
                ListItem::new(format!(
                    "{}{}{}  {:?}  {:.0}%  {}{}",
                    "  ".repeat(depth(&self.plan, w)),
                    crate::work_label::milestone_badge(w, progress.verified),
                    w.key,
                    w.execution.status,
                    progress.percent_complete,
                    w.title,
                    crate::open_choices::scope_tag(
                        progress.scope,
                        self.timeline.applicability(w.id)
                    )
                ))
            });
            let list = List::new(items)
                .block(Block::bordered().title("Work · select then [4] Detail"))
                .highlight_symbol("> ")
                .highlight_style(Style::default().add_modifier(Modifier::BOLD));
            frame.render_stateful_widget(list, body, &mut self.state);
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
            body,
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

fn hierarchy_path(
    plan: &Plan,
    work: &WorkItem,
) -> Vec<(dpm_model::SiblingOrder, dpm_model::WorkItemId)> {
    let mut path = vec![(work.order.clone(), work.id)];
    let mut parent = work.parent;
    while let Some(item) = parent.and_then(|id| plan.work_items.get(&id)) {
        path.push((item.order.clone(), item.id));
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

mod evaluation;

#[cfg(test)]
mod tests;
