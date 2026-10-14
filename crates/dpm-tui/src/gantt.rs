use crate::{shortcuts::Context, text_panel::TextPanel, theme::Theme};
use dpm_engine::{EngineError, ProgressSummary, actual_spans, progress, schedule_span};
use dpm_model::{Plan, Timeline, WorkItem, WorkItemId, WorkKind};
use dpm_schedule::{Schedule, deterministic_remaining};
use ratatui::{layout::Rect, text::Span};
use std::collections::BTreeMap;

mod chart;
pub(crate) mod dependencies;
mod help;
mod inspection;
mod interaction;
mod palette;
mod release;
mod viewport;
use palette::RECORDED;
use viewport::Viewport;

/// Read-only timeline of recorded history before the clock reading and remaining work after it;
/// dates and progress never enter the stored graph.
pub(crate) struct Gantt {
    schedule: Schedule,
    /// Recorded range of work that started or finished, in hours from the clock reading.
    history: BTreeMap<WorkItemId, (f64, f64)>,
    timeline: Timeline,
    progress: BTreeMap<WorkItemId, ProgressSummary>,
    viewport: Viewport,
    theme: Theme,
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
        let timeline = Timeline::at(plan, clock);
        // Outstanding work ran up to the clock reading; a milestone is reached at one instant.
        let history: BTreeMap<_, _> = actual_spans(plan, &timeline)
            .into_iter()
            .filter_map(|(id, actual)| {
                let finish = actual.finish_hours.unwrap_or(0.0);
                Some((id, (actual.start_hours.or(actual.finish_hours)?, finish)))
            })
            .collect();
        let earliest = history.values().map(|range| range.0).fold(0.0, f64::min);
        let viewport = Viewport::new(earliest, schedule.project_finish_hours);
        Ok(Self {
            schedule,
            history,
            timeline,
            progress: progress(plan, clock)?.work,
            viewport,
            theme: Theme { enabled: true },
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
        self.theme = previous.theme;
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
        self.theme.enabled = enabled;
    }
    /// Where keyboard focus is inside the Gantt, for the footer's hints.
    pub(crate) fn shortcut_context(&self) -> Context {
        if self.help {
            Context::Help
        } else if self.inspecting {
            Context::Inspector
        } else {
            Context::Chart
        }
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
    /// A row's bar: the recorded range drawn with `RECORDED` (a milestone with its own symbol),
    /// then the remaining range over it. Complete work with a recorded range draws only that, so
    /// its zero-length remaining bar at the clock reading does not hide when it happened.
    fn row_bar(
        &self,
        item: &WorkItem,
        remaining: (f64, f64),
        complete: bool,
        width: usize,
        symbol: char,
    ) -> String {
        let recorded = self.history.get(&item.id).copied();
        let (start, end) = match recorded {
            Some(range) if complete => range,
            Some(range) => (range.0.min(remaining.0), range.1.max(remaining.1)),
            None => remaining,
        };
        let mut cells: Vec<char> = self
            .bar(start, end, width, ' ', item.kind)
            .chars()
            .collect();
        if let Some((from, to)) = recorded.filter(|r| width > 0 && self.viewport.contains(r.0, r.1))
        {
            let past = if item.kind == WorkKind::Milestone {
                symbol
            } else {
                RECORDED
            };
            let (left, right) = (
                self.viewport.cell(from, width),
                self.viewport.cell(to, width),
            );
            for cell in cells.iter_mut().take(right + 1).skip(left) {
                if *cell == ' ' {
                    *cell = past;
                }
            }
        }
        if !(complete && recorded.is_some()) && self.viewport.contains(remaining.0, remaining.1) {
            let (left, right) = (
                self.viewport.cell(remaining.0, width),
                self.viewport.cell(remaining.1, width),
            );
            for cell in cells.iter_mut().take(right + 1).skip(left) {
                if *cell == ' ' || *cell == RECORDED {
                    *cell = symbol;
                }
            }
        }
        cells.into_iter().collect()
    }
    /// Scheduled range of work, or `None` for work the timeline places outside the active graph;
    /// packages span their applicable descendants.
    fn bounds(&self, plan: &Plan, item: &WorkItem) -> Option<(f64, f64, bool)> {
        schedule_span(plan, &self.timeline, &self.schedule, item)
            .map(|span| (span.start_hours, span.finish_hours, span.critical))
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
