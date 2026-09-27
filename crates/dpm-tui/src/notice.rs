//! Reload failure notice. The retry instruction and the revision still on screen always get their
//! own row; the error text, often a long path, is shortened in the middle to fit what remains.

use ratatui::{Frame, layout::Rect, style::Style, text::Line, widgets::Paragraph};
use unicode_width::UnicodeWidthStr;

/// Rows a notice occupies below the header title.
pub(crate) const ROWS: u16 = 3;

/// A failed reload, shown while the last valid snapshot stays displayed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReloadNotice {
    /// Why the reload failed.
    pub(crate) error: String,
    /// Revision of the snapshot still displayed.
    pub(crate) revision: u64,
}

impl ReloadNotice {
    /// Draw the error rows and the fixed retry row into `area`.
    pub(crate) fn render(&self, frame: &mut Frame<'_>, area: Rect) {
        let error_rows = usize::from(area.height.saturating_sub(1));
        let mut lines: Vec<Line<'_>> = fit(
            &format!("Reload failed: {}", self.error),
            usize::from(area.width),
            error_rows,
        )
        .into_iter()
        .map(Line::from)
        .collect();
        lines.resize(error_rows, Line::default());
        lines.push(Line::from(format!(
            "Showing revision {}; [r] retry.",
            self.revision
        )));
        frame.render_widget(Paragraph::new(lines), area);
    }
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
#[allow(clippy::expect_used)]
mod tests;
