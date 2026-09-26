use super::*;
use crossterm::event::{KeyCode, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use dpm_model::{ActorId, WorkStatus};
use ratatui::{Terminal, backend::TestBackend, buffer::Buffer};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../../tests/support/execution-plan.json"
    ))
    .expect("graph")
}

fn draw(gantt: &mut Gantt, plan: &Plan, selected: usize, width: u16, height: u16) -> Buffer {
    let work: Vec<_> = plan.work_items.values().cloned().collect();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| gantt.render(frame, frame.area(), plan, &work, Some(selected)))
        .expect("draw");
    terminal.backend().buffer().clone()
}

fn text(buffer: &Buffer) -> String {
    let mut text = String::new();
    for y in buffer.area.top()..buffer.area.bottom() {
        let mut x = buffer.area.left();
        while x < buffer.area.right() {
            let symbol = buffer[(x, y)].symbol();
            text.push_str(symbol);
            x = x.saturating_add(Span::raw(symbol).width().max(1) as u16);
        }
    }
    text
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

#[test]
fn hover_reveals_full_title_and_links_without_moving_selection_or_rows() {
    let mut plan = fixture();
    plan.find_work_by_key_mut("TEST-B").expect("b").title =
        "A full task title that is wider than the chart label column 标题尾部".into();
    let before = plan.clone();
    let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
    draw(&mut gantt, &plan, 0, 120, 35);
    let area = gantt.row_area;
    assert_eq!(
        gantt.handle_mouse(
            mouse(MouseEventKind::Moved, area.x + 2, area.y + 1),
            Some(0),
            7
        ),
        None
    );
    let hovered = draw(&mut gantt, &plan, 0, 120, 35);
    let screen = text(&hovered);
    assert!(screen.contains(
        "Hover TEST-B — A full task title that is wider than the chart label column 标题尾部"
    ));
    assert!(screen.contains("Predecessors ← (1)"));
    assert!(screen.contains("TEST-A --FS+0.0h→TEST-B"));
    assert!(screen.contains("Successors → (1)"));
    assert!(screen.contains("TEST-B --FS+0.0h→TEST-C"));
    assert_eq!(gantt.row_area, area);
    assert!(
        hovered[(area.x, area.y)]
            .modifier
            .contains(Modifier::REVERSED)
    );
    assert_eq!(hovered[(area.x + 3, area.y)].fg, Color::Cyan);
    assert_eq!(hovered[(area.x + 3, area.y + 2)].fg, Color::Magenta);
    assert_eq!(draw(&mut gantt, &plan, 0, 120, 35), hovered);
    assert_eq!(
        gantt.handle_mouse(
            mouse(
                MouseEventKind::Down(MouseButton::Left),
                area.x + 2,
                area.y + 1
            ),
            Some(0),
            7
        ),
        Some(1)
    );
    assert!(text(&draw(&mut gantt, &plan, 1, 120, 35)).contains("Selected TEST-B"));
    assert_eq!(plan, before);
}

#[test]
fn wrapped_unicode_title_and_all_offscreen_links_can_be_read_by_scrolling() {
    let mut plan = fixture();
    plan.find_work_by_key_mut("TEST-A").expect("a").title =
        format!("{} TITLE_END", "长标题与验收范围 ".repeat(60));
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    for index in 0..18 {
        let mut child = plan.find_work_by_key("TEST-B").expect("b").clone();
        child.id = WorkItemId::new();
        child.key = dpm_model::Key::new(format!("EXTRA-{index:02}"));
        child.title = format!("Dependent task {index:02} COMPLETE_NAME");
        plan.dependencies.push(dpm_model::Dependency::new(
            a,
            child.id,
            dpm_model::DependencyKind::FinishStart,
            2.0,
        ));
        plan.work_items.insert(child.id, child);
    }
    let before = plan.clone();
    let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
    let index = plan
        .work_items
        .keys()
        .position(|id| *id == a)
        .expect("index");
    let mut seen = text(&draw(&mut gantt, &plan, index, 58, 22));
    assert!(gantt.navigate(KeyCode::Tab));
    for _ in 0..120 {
        gantt.navigate(KeyCode::PageDown);
        seen.push_str(&text(&draw(&mut gantt, &plan, index, 58, 22)));
    }
    assert!(seen.contains("TITLE_END"));
    for number in 0..18 {
        assert!(seen.contains(&format!("Dependent task {number:02} COMPLETE_NAME")));
    }
    assert!(seen.contains("SF start→finish"));
    gantt.navigate(KeyCode::End);
    assert!(
        text(&draw(&mut gantt, &plan, index, 120, 38)).contains("release at this snapshot's clock")
    );
    gantt.navigate(KeyCode::Home);
    assert!(text(&draw(&mut gantt, &plan, index, 120, 38)).contains("Selected TEST-A"));
    assert_eq!(plan, before);
}

#[test]
fn hit_testing_tracks_vertical_scroll_resize_and_wheel_focus() {
    let plan = fixture();
    let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
    draw(&mut gantt, &plan, 6, 80, 16);
    assert!(gantt.first_row > 0);
    let area = gantt.row_area;
    assert_eq!(
        gantt.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x, area.y),
            Some(6),
            7
        ),
        Some(gantt.first_row)
    );
    let inspector = gantt.inspector.area;
    let selected = gantt.handle_mouse(
        mouse(MouseEventKind::ScrollDown, inspector.x + 2, inspector.y + 2),
        Some(6),
        7,
    );
    assert_eq!(selected, None);
    assert_eq!(
        gantt.handle_mouse(mouse(MouseEventKind::ScrollUp, area.x, area.y), Some(6), 7),
        Some(5)
    );
    draw(&mut gantt, &plan, 6, 30, 8);
    assert_eq!(
        gantt.handle_mouse(
            mouse(MouseEventKind::Down(MouseButton::Left), area.x, area.y),
            Some(6),
            7
        ),
        None
    );
    draw(&mut gantt, &plan, 6, 120, 35);
    assert_eq!(gantt.first_row, 0);
}

#[test]
fn status_colors_and_symbols_win_over_criticality_and_monochrome_retains_meaning() {
    for (status, expected, color) in [
        (WorkStatus::Planned, "#", Color::Red),
        (WorkStatus::Blocked, "!", Color::Yellow),
        (WorkStatus::Submitted, "?", Color::Magenta),
        (WorkStatus::Verified, ".", Color::Green),
    ] {
        let mut plan = fixture();
        let work = plan.find_work_by_key_mut("TEST-A").expect("a");
        work.status = status;
        if status != WorkStatus::Planned {
            work.owner = Some(ActorId::agent("owner"));
        }
        if status == WorkStatus::Blocked {
            work.block_reason = Some("waiting".into());
        }
        let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
        let buffer = draw(&mut gantt, &plan, 0, 100, 30);
        let y = gantt.row_area.y;
        let empty_time = &buffer[(96, y)];
        assert_eq!(empty_time.symbol(), " ");
        assert_eq!(empty_time.bg, Color::Reset);
        assert!(!empty_time.modifier.contains(Modifier::REVERSED));
        assert!(
            (0..100).any(|x| buffer[(x, y)].symbol() == expected && buffer[(x, y)].fg == color)
        );
        gantt.set_colors(false);
        let mono = draw(&mut gantt, &plan, 0, 100, 30);
        assert!(
            mono.content
                .iter()
                .all(|cell| cell.fg == Color::Reset && cell.bg == Color::Reset)
        );
        assert!((0..100).any(|x| mono[(x, y)].symbol() == expected));
        assert!(text(&mono).contains("<P Predecessor"));
    }
}

#[test]
fn help_scroll_and_escape_preserve_chart_focus_and_plan() {
    let plan = fixture();
    let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
    let initial = draw(&mut gantt, &plan, 0, 80, 24);
    assert!(gantt.navigate(KeyCode::Char('?')));
    assert!(text(&draw(&mut gantt, &plan, 0, 80, 24)).contains("keyboard and mouse"));
    gantt.navigate(KeyCode::End);
    assert!(text(&draw(&mut gantt, &plan, 0, 80, 24)).contains("NO_COLOR"));
    assert!(gantt.navigate(KeyCode::Esc));
    assert_eq!(draw(&mut gantt, &plan, 0, 80, 24), initial);
    assert!(gantt.navigate(KeyCode::Tab));
    assert!(gantt.navigate(KeyCode::Esc));
    assert!(!gantt.navigate(KeyCode::Esc));
}

#[test]
fn milestone_name_badge_survives_panning_and_monochrome_and_tracks_completion() {
    for complete in [false, true] {
        let mut plan = fixture();
        if complete {
            for work in plan.work_items.values_mut().filter(|w| w.is_executable()) {
                work.status = WorkStatus::Verified;
                work.owner = Some(ActorId::agent("worker"));
            }
            for decision in plan.decisions.values_mut() {
                decision.status = dpm_model::DecisionStatus::Decided;
                decision.outcome = Some("Accepted".into());
            }
        }
        let before = plan.clone();
        let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
        gantt.set_colors(false);
        let selected = plan
            .work_items
            .values()
            .position(|w| w.key.0 == "TEST-M1")
            .expect("milestone");
        let badge = if complete {
            "◆[M] TEST-M1"
        } else {
            "◇[M] TEST-M1"
        };
        for key in [KeyCode::Home, KeyCode::Right, KeyCode::End] {
            gantt.navigate(key);
            let buffer = draw(&mut gantt, &plan, selected, 100, 30);
            let y = gantt.row_area.y + (selected - gantt.first_row) as u16;
            let label: String = (0..36).map(|x| buffer[(x, y)].symbol()).collect();
            assert!(label.contains(badge), "{label}");
            if !complete && key == KeyCode::Home {
                assert!((42..96).all(|x| buffer[(x, y)].symbol() != "◇"));
            }
            assert!(buffer.content.iter().all(|cell| cell.fg == Color::Reset));
        }
        assert_eq!(plan, before);
    }
}

fn at(hour: u32) -> chrono::DateTime<chrono::Utc> {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(2026, 9, 1, hour, 0, 0)
        .single()
        .expect("time")
}

/// The fixture reduced to one edge TEST-A → TEST-B and no decision gates.
fn single_edge(kind: dpm_model::DependencyKind, lag: f64) -> (Plan, WorkItemId, WorkItemId) {
    let mut plan = fixture();
    plan.decisions.clear();
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.dependencies = vec![dpm_model::Dependency::new(a, b, kind, lag)];
    (plan, a, b)
}

fn run(plan: &mut Plan, commands: Vec<dpm_engine::Command>, hour: u32) {
    for command in commands {
        dpm_engine::apply_command(plan, ActorId::agent("worker"), command, at(hour))
            .expect("execute");
    }
}

fn begin(work: WorkItemId) -> Vec<dpm_engine::Command> {
    vec![
        dpm_engine::Command::Claim { work },
        dpm_engine::Command::Start { work },
    ]
}

fn inspect(plan: &Plan, work: WorkItemId, clock: chrono::DateTime<chrono::Utc>) -> String {
    let mut gantt = Gantt::new(plan, clock).expect("gantt");
    let index = plan
        .work_items
        .keys()
        .position(|id| *id == work)
        .expect("index");
    text(&draw(&mut gantt, plan, index, 170, 60))
}

#[test]
fn inspector_tags_soft_and_waived_edges_like_network_and_detail() {
    let (mut plan, a, b) = single_edge(dpm_model::DependencyKind::FinishStart, 5.0);
    plan.dependencies[0].policy = dpm_model::DependencyPolicy::Soft;
    let soft = inspect(&plan, b, at(1));
    assert!(soft.contains("TEST-A --FS+5.0h→TEST-B [soft]"), "{soft}");
    plan.dependencies[0].waiver = Some(dpm_model::DependencyWaiver {
        actor: ActorId::human("lead"),
        at: at(1),
        reason: "overlap accepted".into(),
    });
    let waived = inspect(&plan, b, at(2));
    assert!(
        waived.contains("TEST-A --FS+5.0h→TEST-B [soft, waived]"),
        "{waived}"
    );
    assert!(waived.contains("waived, not enforced"), "{waived}");
    assert!(inspect(&plan, a, at(2)).contains("[soft, waived]"));
}

#[test]
fn inspector_reports_start_event_releases_for_ss_edges_instead_of_verification() {
    let (mut plan, a, b) = single_edge(dpm_model::DependencyKind::StartStart, 2.0);
    let waiting = inspect(&plan, b, at(1));
    assert!(
        waiting.contains("gates start: awaiting start of TEST-A"),
        "{waiting}"
    );
    assert!(!waiting.contains("Execution waits for verified prerequisites"));
    run(&mut plan, begin(a), 1);
    let elapsing = inspect(&plan, b, at(2));
    assert!(
        elapsing.contains("gates start: lag elapses at 2026-09-01 03:00 UTC"),
        "{elapsing}"
    );
    let released = inspect(&plan, b, at(4));
    assert!(released.contains("gates start: released"), "{released}");
}

#[test]
fn inspector_reports_that_ff_edges_gate_submission() {
    let (plan, _, b) = single_edge(dpm_model::DependencyKind::FinishFinish, 0.0);
    let screen = inspect(&plan, b, at(1));
    assert!(
        screen.contains("gates submit/verify: awaiting verified finish of TEST-A"),
        "{screen}"
    );
}

#[test]
fn inspector_shows_a_provisional_start_released_on_a_pending_attempt() {
    let (mut plan, a, b) = single_edge(dpm_model::DependencyKind::FinishStart, 0.0);
    plan.dependencies[0].start_basis = dpm_model::StartBasis::Provisional;
    let waiting = inspect(&plan, b, at(1));
    assert!(waiting.contains("[provisional start]"), "{waiting}");
    assert!(
        waiting.contains("awaiting a submitted attempt or verified finish of TEST-A"),
        "{waiting}"
    );
    let mut steps = begin(a);
    steps.push(dpm_engine::Command::Submit {
        work: a,
        note: None,
    });
    run(&mut plan, steps, 1);
    run(&mut plan, begin(b), 2);
    let started = inspect(&plan, b, at(3));
    assert!(
        started.contains("gates start: released provisionally on attempt #1"),
        "{started}"
    );
    assert!(
        started.contains("gates verify: awaiting verified finish of TEST-A"),
        "{started}"
    );
}

#[test]
fn inspector_lists_decision_gates_with_their_state() {
    let mut plan = fixture();
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    plan.dependencies.retain(|d| d.successor == b);
    let screen = inspect(&plan, b, at(1));
    assert!(
        screen.contains("Decision gates: TEST-GATE open"),
        "{screen}"
    );
}
