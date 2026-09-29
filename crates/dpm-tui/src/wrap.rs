//! Word wrapping measured in terminal cells, so a wide glyph never starts in the last column of a
//! pane and spills into the border or scrollbar beside it.

use ratatui::{
    style::Style,
    text::{Line, Span, StyledGrapheme, Text},
};
use unicode_width::UnicodeWidthStr;

/// Wrap every line of `text` into rows at most `width` cells wide, keeping styles.
pub(crate) fn rows(text: Text<'_>, width: u16) -> Vec<Line<'static>> {
    let width = usize::from(width.max(1));
    text.lines
        .iter()
        .flat_map(|line| wrap_line(line, text.style, width))
        .collect()
}

struct Cell<'a> {
    grapheme: StyledGrapheme<'a>,
    width: usize,
    blank: bool,
}

fn wrap_line(line: &Line<'_>, base: Style, width: usize) -> Vec<Line<'static>> {
    let cells: Vec<Cell<'_>> = line
        .styled_graphemes(base)
        .map(|grapheme| Cell {
            width: grapheme.symbol.width(),
            blank: grapheme.symbol.chars().all(char::is_whitespace),
            grapheme,
        })
        .collect();
    let mut rows: Vec<Vec<&Cell<'_>>> = vec![Vec::new()];
    let mut used = 0;
    let mut index = 0;
    while index < cells.len() {
        let end = word_end(&cells, index);
        let word = cells.get(index..end).unwrap_or_default();
        let word_width: usize = word.iter().map(|c| c.width).sum();
        if used + word_width > width && used > 0 {
            rows.push(Vec::new());
            used = 0;
            if word.iter().all(|c| c.blank) {
                index = end;
                continue;
            }
        }
        for cell in word {
            if used + cell.width > width && used > 0 {
                rows.push(Vec::new());
                used = 0;
            }
            if let Some(row) = rows.last_mut() {
                row.push(cell);
            }
            used += cell.width;
        }
        index = end;
    }
    rows.into_iter()
        .map(|row| {
            let mut wrapped = Line::from(spans(&row)).style(line.style);
            wrapped.alignment = line.alignment;
            wrapped
        })
        .collect()
}

/// A word is a run of non-blank graphemes; each blank is its own break opportunity.
fn word_end(cells: &[Cell<'_>], start: usize) -> usize {
    if cells.get(start).is_some_and(|c| c.blank) {
        return start + 1;
    }
    cells
        .iter()
        .skip(start)
        .position(|c| c.blank)
        .map_or(cells.len(), |offset| start + offset)
}

fn spans(row: &[&Cell<'_>]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for cell in row {
        match spans.last_mut() {
            Some(span) if span.style == cell.grapheme.style => {
                span.content.to_mut().push_str(cell.grapheme.symbol);
            }
            _ => spans.push(Span::styled(
                cell.grapheme.symbol.to_owned(),
                cell.grapheme.style,
            )),
        }
    }
    spans
}

#[cfg(test)]
mod tests;
