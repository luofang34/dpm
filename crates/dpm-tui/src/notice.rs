//! Notices about the source behind the displayed snapshot: a failed reload or change check, or a
//! source that no longer continues the displayed history. The action row, with the revision still
//! on screen, always gets its own row; the message, often a long path, is shortened in the middle
//! to fit what remains. Notices are plain text, so they read the same without color.

use crate::source::SourceRevision;
use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};
use unicode_width::UnicodeWidthStr;

/// Rows a notice occupies below the header title.
pub(crate) const ROWS: u16 = 3;

/// Shown while the last valid snapshot stays displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReloadNotice {
    /// What happened; elided to fit.
    message: String,
    /// The revision still displayed and the operator's action; never elided.
    action: String,
    /// Word the header puts before "snapshot" while this notice stands.
    pub(crate) label: Option<&'static str>,
}

impl ReloadNotice {
    /// A reload that failed; `revision` is the snapshot still displayed, which is behind its source.
    pub(crate) fn failed(error: &str, revision: u64) -> Self {
        Self {
            message: format!("Reload failed: {error}"),
            action: format!("Showing revision {revision}; [r] retry."),
            label: Some("STALE"),
        }
    }

    /// A change check that failed, so the display may be behind its source.
    pub(crate) fn unchecked(error: &str, revision: u64) -> Self {
        Self {
            message: format!("Cannot check for changes: {error}"),
            action: format!("Showing revision {revision}, possibly out of date; [r] retry."),
            label: Some("UNCHECKED"),
        }
    }

    /// A source holding an older revision or another lineage than the one displayed.
    pub(crate) fn diverged(shown: SourceRevision, found: SourceRevision) -> Self {
        let message = if found.lineage_id == shown.lineage_id {
            format!(
                "Source went back to revision {} of the same history; not shown as current.",
                found.revision
            )
        } else {
            format!(
                "Source now holds a different history: lineage {} at revision {} (displayed: lineage {}).",
                lineage(found),
                found.revision,
                lineage(shown)
            )
        };
        Self {
            message,
            action: format!(
                "Showing revision {}; [r] load revision {} instead.",
                shown.revision, found.revision
            ),
            label: Some("STALE"),
        }
    }

    /// Draw the message rows and the fixed action row into `area`.
    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let message_rows = usize::from(area.height.saturating_sub(1));
        let mut lines: Vec<Line<'_>> = fit(&self.message, usize::from(area.width), message_rows)
            .into_iter()
            .map(Line::from)
            .collect();
        lines.resize(message_rows, Line::default());
        lines.push(Line::from(self.action.as_str()));
        frame.render_widget(Paragraph::new(lines), area);
    }
}

fn lineage(revision: SourceRevision) -> String {
    revision
        .lineage_id
        .map_or_else(|| "none (preview)".into(), |id| id.to_string())
}

/// Lay `text` out in at most `rows` rows of `width` cells, eliding its middle when it is longer:
/// the start names the failing action and the end its cause.
pub(crate) fn fit(text: &str, width: usize, rows: usize) -> Vec<String> {
    let capacity = width.saturating_mul(rows);
    if width == 0 || rows == 0 {
        return Vec::new();
    }
    let line = Line::from(text);
    let graphemes: Vec<(&str, usize)> = line
        .styled_graphemes(Style::default())
        .map(|g| (g.symbol, g.symbol.width()))
        .collect();
    let total: usize = graphemes.iter().map(|g| g.1).sum();
    let mut kept: Vec<(&str, usize)> = graphemes.clone();
    if total > capacity {
        // Leave a cell of slack per row: a wide glyph may not fit in a row's last cell.
        let budget = capacity.saturating_sub(rows).saturating_sub(1);
        let head = take_width(graphemes.iter().copied(), budget / 2);
        let tail = take_width(graphemes.iter().rev().copied(), budget - budget / 2);
        kept = head;
        kept.push(("…", 1));
        kept.extend(tail.into_iter().rev());
    }
    let mut lines = vec![String::new()];
    let mut used = 0;
    for (symbol, cells) in kept {
        if used + cells > width && used > 0 {
            lines.push(String::new());
            used = 0;
        }
        if let Some(line) = lines.last_mut() {
            line.push_str(symbol);
        }
        used += cells;
    }
    lines.truncate(rows);
    lines
}

fn take_width<'a>(
    graphemes: impl Iterator<Item = (&'a str, usize)>,
    budget: usize,
) -> Vec<(&'a str, usize)> {
    let mut used = 0;
    graphemes
        .take_while(|(_, cells)| {
            used += cells;
            used <= budget
        })
        .collect()
}

#[cfg(test)]
mod tests;
