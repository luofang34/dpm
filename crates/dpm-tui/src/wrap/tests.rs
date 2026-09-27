use super::*;
use ratatui::style::{Color, Stylize};

const TITLE: &str = "Selected TEST-B — 部分進捗タスク 🧪 naïve café e\u{301} — a long title ⟨TAIL⟩";

fn plain(row: &Line<'_>) -> String {
    row.spans.iter().map(|s| s.content.as_ref()).collect()
}

fn visible(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn rows_never_exceed_the_width_and_keep_every_visible_grapheme() {
    for width in 2..=40 {
        let rows = rows(Text::from(TITLE), width);
        for row in &rows {
            assert!(row.width() <= usize::from(width), "{width}: {row:?}");
        }
        let joined: String = rows.iter().map(plain).collect();
        assert_eq!(visible(&joined), visible(TITLE), "{width}");
        assert!(
            !rows.iter().skip(1).any(|r| plain(r).starts_with('\u{301}')),
            "a combining mark stays with its base"
        );
    }
}

#[test]
fn a_wide_glyph_that_does_not_fit_moves_to_the_next_row() {
    let rows = rows(Text::from("abcdefghi部"), 10);
    assert_eq!(
        rows.iter().map(plain).collect::<Vec<_>>(),
        ["abcdefghi", "部"]
    );
}

#[test]
fn words_wrap_at_blanks_and_long_words_break_by_cells() {
    let rows = rows(Text::from("alpha beta gamma"), 7);
    assert_eq!(
        rows.iter().map(plain).collect::<Vec<_>>(),
        ["alpha ", "beta ", "gamma"]
    );
    let rows = super::rows(Text::from("  indented supercalifragilistic"), 8);
    assert_eq!(plain(&rows[0]), "  ");
    assert!(rows.iter().all(|r| r.width() <= 8));
    assert_eq!(
        super::rows(Text::from(""), 5).len(),
        1,
        "an empty line is one row"
    );
}

#[test]
fn styles_of_lines_and_spans_survive_wrapping() {
    let line = Line::from(vec!["plain ".into(), "cyan words here".fg(Color::Cyan)])
        .style(Style::default().bold());
    let rows = rows(Text::from(line), 8);
    assert!(rows.iter().all(|r| r.style == Style::default().bold()));
    let cyan: String = rows
        .iter()
        .flat_map(|r| r.spans.iter())
        .filter(|s| s.style.fg == Some(Color::Cyan))
        .map(|s| s.content.as_ref())
        .collect();
    assert_eq!(visible(&cyan), "cyanwordshere");
}
