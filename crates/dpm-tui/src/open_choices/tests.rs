use crate::view::{Page, View};
use dpm_model::Plan;
use ratatui::{Terminal, backend::TestBackend};

fn conditional() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture")
}

fn decide_b(plan: &mut Plan) {
    let decision = plan.decisions.keys().next().copied().expect("decision");
    dpm_engine::apply_command(
        plan,
        dpm_model::ActorId::human("lead"),
        dpm_engine::Command::Decide {
            decision,
            outcome: "B".into(),
        },
        chrono::Utc::now(),
        dpm_model::OperationId::new(),
    )
    .expect("decide");
}

/// Screen rows, so assertions can name the row a key is listed on.
fn rows(view: &mut View, page: Page) -> Vec<String> {
    view.page = page;
    let mut terminal = Terminal::new(TestBackend::new(240, 60)).expect("test terminal");
    terminal.draw(|frame| view.render(frame)).expect("render");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(240)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect()
}

fn row<'a>(rows: &'a [String], key: &str) -> &'a str {
    rows.iter()
        .find(|row| row.contains(&format!("{key} ")))
        .map(String::as_str)
        .unwrap_or_default()
}

#[test]
fn now_names_why_work_is_outside_the_active_graph_in_words_and_keys() {
    let mut plan = conditional();
    decide_b(&mut plan);
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    let now = rows(&mut view, Page::Now).concat();
    assert!(!now.contains("DependencyId("), "{now}");
    assert!(!now.contains("Key(\""), "{now}");
    assert!(
        now.contains("SUP-A-AUDIT — stranded: prerequisite SUP-A-QUOTE was not selected"),
        "{now}"
    );
    assert!(
        now.contains("SUP-A-QUOTE — not selected: work applies only if DEC-SUPPLIER selects A, but it selected B"),
        "{now}"
    );
}

#[test]
fn work_list_tags_stranded_and_awaiting_choice_work() {
    let mut plan = conditional();
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    let open = rows(&mut view, Page::Work);
    assert!(
        row(&open, "SUP-A-AUDIT").contains("[awaiting a choice]"),
        "{open:#?}"
    );
    assert!(
        row(&open, "SUP-A-QUOTE").contains("[undecided]"),
        "{open:#?}"
    );
    assert!(!row(&open, "SUP-DESIGN").contains('['), "{open:#?}");
    decide_b(&mut plan);
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    let decided = rows(&mut view, Page::Work);
    assert!(
        row(&decided, "SUP-A-AUDIT").contains("[stranded]"),
        "{decided:#?}"
    );
    assert!(
        row(&decided, "SUP-A-QUOTE").contains("[not selected]"),
        "{decided:#?}"
    );
}

#[test]
fn now_shows_every_percentile_of_each_open_choice_scenario() {
    let plan = conditional();
    let clock = chrono::Utc::now();
    let summary = dpm_engine::status(&plan, true, clock).expect("status");
    let scenarios = summary.open_choices.expect("open choices").scenarios;
    assert!(!scenarios.is_empty());
    let mut view = View::new(&plan, clock).expect("view");
    let now = rows(&mut view, Page::Now);
    for scenario in scenarios {
        let pick = scenario
            .choices
            .iter()
            .map(|(decision, option)| format!("{decision}={option}"))
            .collect::<Vec<_>>()
            .join(" ");
        let line = row(&now, &format!("{pick}:"));
        for (label, value) in [
            ("P50", scenario.p50_finish_hours),
            ("P80", scenario.p80_finish_hours),
            ("P95", scenario.p95_finish_hours),
        ] {
            let value = value.expect("sampled percentile");
            assert!(line.contains(&format!("{label} {value:.1}h")), "{line}");
        }
    }
    let execution = now
        .iter()
        .find(|row| row.contains("Execution:"))
        .expect("execution row");
    assert!(
        !execution.contains("P50"),
        "no blended percentile: {execution}"
    );
}

#[test]
fn now_names_unestimated_work_in_text_for_the_headline_and_each_scenario() {
    let mut plan = conditional();
    for key in ["SUP-DESIGN", "SUP-A-QUOTE"] {
        let id = plan.find_work_by_key(key).expect("work").id;
        plan.work_items
            .get_mut(&id)
            .expect("work")
            .schedule
            .estimate = None;
    }
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    let now = rows(&mut view, Page::Now);
    let headline = row(&now, "Unestimated");
    assert!(
        headline.contains("Unestimated 1: counted as 0 h, forecast optimistic: SUP-DESIGN"),
        "{now:#?}"
    );
    assert!(row(&now, "DEC-SUPPLIER=A:").contains("unestimated (0 h) SUP-A-QUOTE"));
    assert!(!row(&now, "DEC-SUPPLIER=B:").contains("SUP-A-QUOTE"));
}

#[test]
fn key_lists_name_the_count_they_leave_out() {
    let keys: Vec<_> = ["A", "B", "C", "D", "E", "F"]
        .into_iter()
        .map(dpm_model::Key::new)
        .collect();
    assert_eq!(super::key_list(&keys[..2]), "A, B");
    assert_eq!(super::key_list(&keys), "A, B, C, D, E (+1 more)");
}
