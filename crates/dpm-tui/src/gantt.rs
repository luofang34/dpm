use crate::text_panel::TextPanel;
use dpm_engine::{EngineError, ProgressSummary, progress};
use dpm_model::{Plan, Timeline, WorkItem, WorkItemId, WorkKind};
use dpm_schedule::{Schedule, deterministic_remaining};
use ratatui::{layout::Rect, text::Span};
use std::collections::BTreeMap;

mod chart;
pub(crate) mod dependencies;
mod inspection;
mod interaction;
mod palette;
mod release;
mod viewport;
use palette::Palette;
use viewport::Viewport;

/// Read-only remaining-work timeline; dates and progress never enter the stored graph.
pub(crate) struct Gantt {
    schedule: Schedule,
    timeline: Timeline,
    progress: BTreeMap<WorkItemId, ProgressSummary>,
    viewport: Viewport,
    palette: Palette,
    inspector: TextPanel,
    help_panel: TextPanel,
    inspecting: bool,
    help: bool,
    hovered: Option<usize>,
    inspected: Option<WorkItemId>,
    row_area: Rect,
    first_row: usize,
}
impl Gantt {
    pub(crate) fn new(
        plan: &Plan,
        clock: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self, EngineError> {
        let schedule = deterministic_remaining(plan, clock)?;
        let viewport = Viewport::new(schedule.project_finish_hours);
        Ok(Self {
            schedule,
            timeline: Timeline::at(plan, clock),
            progress: progress(plan, clock)?.work,
            viewport,
            palette: Palette { enabled: true },
            inspector: TextPanel::default(),
            help_panel: TextPanel::default(),
            inspecting: false,
            help: false,
            hovered: None,
            inspected: None,
            row_area: Rect::default(),
            first_row: 0,
        })
    }
    /// Carry the operator's place over from the projection this one replaces. Hover is not kept:
    /// it is temporary inspection that the next pointer event re-establishes.
    pub(crate) fn restore_navigation(&mut self, previous: &mut Self) {
        self.viewport.restore(&previous.viewport);
        self.palette.enabled = previous.palette.enabled;
        self.inspecting = previous.inspecting;
        self.help = previous.help;
        self.first_row = previous.first_row;
        // The inspector scroll belongs to the inspected work, so the two move together.
        self.inspected = previous.inspected;
        self.inspector = std::mem::take(&mut previous.inspector);
        self.help_panel = std::mem::take(&mut previous.help_panel);
    }
    #[cfg(test)]
    pub(crate) fn inspector_area(&self) -> Rect {
        self.inspector.area
    }
    pub(crate) fn set_colors(&mut self, enabled: bool) {
        self.palette.enabled = enabled;
    }
    fn bar(&self, start: f64, end: f64, width: usize, symbol: char, kind: WorkKind) -> String {
        let mut bar = vec![' '; width];
        if width == 0 || !self.viewport.contains(start, end) {
            return bar.into_iter().collect();
        }
        let left = self.viewport.cell(start, width);
        let right = self.viewport.cell(end, width);
        for cell in bar.iter_mut().take(right + 1).skip(left) {
            *cell = symbol;
        }
        if kind != WorkKind::Milestone {
            if start < self.viewport.start
                && let Some(first) = bar.first_mut()
            {
                *first = '<';
            }
            if end > self.viewport.end()
                && let Some(last) = bar.last_mut()
            {
                *last = '>';
            }
        }
        bar.into_iter().collect()
    }
    /// Scheduled range of work, or `None` for work the timeline places outside the active graph;
    /// packages span their applicable descendants.
    fn bounds(&self, plan: &Plan, item: &WorkItem) -> Option<(f64, f64, bool)> {
        if !self.timeline.applicability(item.id).is_applicable() {
            return None;
        }
        if item.kind != WorkKind::WorkPackage {
            let activity = self.schedule.activities.get(&item.id)?;
            return Some((
                activity.earliest_start_hours,
                activity.earliest_finish_hours,
                activity.critical,
            ));
        }
        let children: Vec<_> = plan
            .work_items
            .values()
            .filter(|child| child.parent == Some(item.id))
            .collect();
        if children.is_empty() {
            return Some((0.0, 0.0, false));
        }
        children
            .into_iter()
            .filter_map(|child| self.bounds(plan, child))
            .reduce(|(a, b, c), (x, y, z)| (a.min(x), b.max(y), c || z))
    }
}

fn truncate(text: &str, width: usize) -> String {
    if Span::raw(text).width() <= width {
        return text.into();
    }
    let mut result = String::new();
    let mut used: usize = 0;
    for character in text.chars() {
        let next = Span::raw(character.to_string()).width();
        if used.saturating_add(next) > width.saturating_sub(1) {
            break;
        }
        result.push(character);
        used = used.saturating_add(next);
    }
    if width > 0 {
        result.push('…');
    }
    result
}

#[cfg(test)]
mod tests;
