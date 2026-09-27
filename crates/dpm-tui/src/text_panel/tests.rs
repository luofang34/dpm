use crate::view::{Page, View};
use dpm_model::Plan;
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

const WIDE: &str = "部分進捗タスク 🧪 naïve café é — a long title that must be truncated in narrow terminals ⟨TAIL⟩";

fn gantt_at_38x15(selected_steps: usize) -> (Buffer, ratatui::layout::Rect) {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    for item in plan.work_items.values_mut() {
        item.title = WIDE.into();
    }
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.page = Page::Gantt;
    for _ in 0..selected_steps {
        view.move_selection(1);
    }
    let mut terminal = Terminal::new(TestBackend::new(38, 15)).expect("terminal");
    terminal.draw(|frame| view.render(frame)).expect("draw");
    (terminal.backend().buffer().clone(), view.inspector_area())
}

#[test]
fn wide_glyphs_at_the_wrap_edge_never_reach_the_inspector_scrollbar_column() {
    for step in 0..7 {
        let (buffer, area) = gantt_at_38x15(step);
        assert!(
            area.width > 2 && area.height > 2,
            "inspector visible: {area:?}"
        );
        let scrollbar = area.right() - 1;
        let last_text = scrollbar - 1;
        for y in area.top() + 1..area.bottom() - 1 {
            let edge = buffer[(scrollbar, y)].symbol();
            assert!(
                ["║", "█", "│", "▲", "▼"].contains(&edge),
                "row {y} scrollbar column holds {edge:?} (step {step})"
            );
            let symbol = buffer[(last_text, y)].symbol();
            assert!(
                unicode_width::UnicodeWidthStr::width(symbol) <= 1,
                "row {y} starts a wide glyph {symbol:?} in the last text column (step {step})"
            );
        }
    }
}
