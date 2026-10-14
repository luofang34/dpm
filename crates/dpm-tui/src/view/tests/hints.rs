use super::fixture;
use crate::view::{Page, View};
use crossterm::event::KeyCode;
use ratatui::{
    Terminal,
    backend::TestBackend,
    buffer::Buffer,
    style::{Color, Modifier},
};

fn draw(view: &mut View, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| view.render(frame)).expect("draw");
    terminal.backend().buffer().clone()
}

fn row(buffer: &Buffer, y: u16) -> String {
    (0..buffer.area.width)
        .map(|x| buffer[(x, y)].symbol())
        .collect()
}

/// The footer row, which is the last row of the screen.
fn footer(view: &mut View) -> String {
    let buffer = draw(view, 120, 35);
    row(&buffer, 34).trim().to_owned()
}

#[test]
fn file_preview_is_labeled_separately_from_database_snapshot() {
    let mut view = View::new(&fixture(), chrono::Utc::now()).expect("view");
    view.preview = true;
    let preview = row(&draw(&mut view, 120, 35), 1);
    assert!(preview.contains("snapshot revision 0"), "{preview}");
    // The label is right-aligned against the header's border.
    assert!(
        preview
            .trim_end_matches('│')
            .ends_with("⏸ read-only preview "),
        "{preview}"
    );
    view.preview = false;
    let live = row(&draw(&mut view, 120, 35), 1);
    assert!(live.contains("snapshot revision 0") && !live.contains("read-only"));
}

#[test]
fn header_names_the_page_in_bold_and_the_snapshot_in_the_dim_role() {
    let mut view = View::new(&fixture(), chrono::Utc::now()).expect("view");
    view.page = Page::Gantt;
    let buffer = draw(&mut view, 120, 35);
    let header = row(&buffer, 1);
    assert!(header.starts_with("│ DPM · Gantt   snapshot revision 0 at "));
    // Column of the first cell of `text`; every header glyph is one cell wide.
    let column = |text: &str| header.split(text).next().map_or(0, |s| s.chars().count()) as u16;
    assert!(
        buffer[(column("Gantt"), 1)]
            .modifier
            .contains(Modifier::BOLD)
    );
    assert_eq!(buffer[(column("DPM"), 1)].fg, Color::LightBlue);
    assert_eq!(buffer[(column("snapshot"), 1)].fg, Color::DarkGray);
    assert!(
        !buffer[(column("snapshot"), 1)]
            .modifier
            .contains(Modifier::BOLD)
    );
    // A stale snapshot's label is a caution and keeps its colour while the metadata is dim.
    view.reload_failed(&"disk unavailable");
    let buffer = draw(&mut view, 120, 35);
    let header = row(&buffer, 1);
    assert!(header.contains("STALE snapshot revision 0"), "{header}");
    let stale = header
        .split("STALE")
        .next()
        .map_or(0, |s| s.chars().count()) as u16;
    assert_eq!(buffer[(stale, 1)].fg, Color::Yellow);
}

#[test]
fn footer_hints_follow_focus_and_end_with_the_help_key_on_the_gantt() {
    let mut view = View::new(&fixture(), chrono::Utc::now()).expect("view");
    view.handle_key(KeyCode::Char('g'));
    assert_eq!(
        footer(&mut view),
        "↑↓ select · ←→ pan · +/- zoom · f fit · tab inspect · ? for shortcuts"
    );
    view.handle_key(KeyCode::Tab);
    assert_eq!(
        footer(&mut view),
        "↑↓ scroll · tab chart · pgup/pgdn page · ? for shortcuts"
    );
    view.handle_key(KeyCode::Char('?'));
    assert_eq!(footer(&mut view), "↑↓ scroll · pgup/pgdn page · esc close");
    view.handle_key(KeyCode::Esc);
    view.handle_key(KeyCode::Esc);
    assert!(footer(&mut view).starts_with("↑↓ select · ←→ pan"));
    view.handle_key(KeyCode::Char('2'));
    assert_eq!(
        footer(&mut view),
        "↑↓ select · enter detail · 1–5 pages · r reload · q quit"
    );
    view.handle_key(KeyCode::Char('1'));
    assert!(footer(&mut view).contains("pgup/pgdn page"));
}

#[test]
fn selection_marker_and_no_color_hold_on_every_page() {
    let mut view = View::new(&fixture(), chrono::Utc::now()).expect("view");
    view.page = Page::Work;
    let work = draw(&mut view, 120, 35);
    assert!(row(&work, 4).contains("❯ "), "{}", row(&work, 4));
    view.set_colors(false);
    view.preview = true;
    for page in [
        Page::Now,
        Page::Work,
        Page::Network,
        Page::Detail,
        Page::Gantt,
    ] {
        view.page = page;
        let buffer = draw(&mut view, 120, 35);
        assert!(
            buffer
                .content
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset),
            "{page:?}"
        );
        assert!(row(&buffer, 1).contains("⏸ read-only preview"));
        assert!(row(&buffer, 3).starts_with('╭'));
    }
}
