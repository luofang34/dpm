use super::*;
use crate::view::{Page, View};
use dpm_model::Plan;
use ratatui::{Terminal, backend::TestBackend};

fn long_error() -> String {
    format!(
        "open existing database in /Users/operator/{}/state.sqlite: unable to open database file",
        "very-long-directory-name/".repeat(12)
    )
}

fn screen(view: &mut View, width: u16, height: u16) -> Vec<String> {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal.draw(|frame| view.render(frame)).expect("draw");
    let buffer = terminal.backend().buffer().clone();
    (0..height)
        .map(|y| (0..width).map(|x| buffer[(x, y)].symbol()).collect())
        .collect()
}

#[test]
fn a_long_reload_error_never_hides_the_revision_and_retry_hint() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    for (width, page) in [(40, Page::Gantt), (60, Page::Now), (120, Page::Detail)] {
        let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
        view.page = page;
        view.reload_failed(&long_error());
        let rows = screen(&mut view, width, 20);
        let text = rows.join("\n");
        assert!(
            text.contains("Showing revision 0; [r] retry."),
            "{width}:\n{text}"
        );
        assert!(
            text.contains("Reload failed: open existing"),
            "{width}:\n{text}"
        );
        assert!(
            text.contains("…") && text.contains("database file"),
            "{width}:\n{text}"
        );
        assert!(
            rows[0].starts_with('┌') && rows[5].starts_with('└'),
            "{width}:\n{text}"
        );
    }
}

#[test]
fn fitting_keeps_short_text_whole_and_elides_the_middle_of_long_text() {
    assert_eq!(fit("Reload failed: gone", 30, 2), ["Reload failed: gone"]);
    let long = format!("start {} end", "x".repeat(200));
    let rows = fit(&long, 20, 2);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|r| r.width() <= 20));
    assert!(rows[0].starts_with("start") && rows[1].ends_with("end"));
    assert!(rows.concat().contains('…'));
    let wide = fit(&"部分進捗".repeat(20), 9, 2);
    assert!(
        wide.len() <= 2 && wide.iter().all(|r| r.width() <= 9),
        "{wide:?}"
    );
    assert!(fit("anything", 0, 2).is_empty());
}
