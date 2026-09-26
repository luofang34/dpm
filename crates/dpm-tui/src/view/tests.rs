use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn render(view: &mut View<'_>) -> String {
    let mut terminal = Terminal::new(TestBackend::new(120, 35)).expect("test terminal");
    terminal.draw(|frame| view.render(frame)).expect("render");
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}

#[test]
fn all_pages_render_snapshot_and_selection() {
    let plan = fixture();
    let mut view = View::new(&plan).expect("view");
    for (page, text) in [
        (Page::Now, "Recommended ready work"),
        (Page::Work, "TEST-A"),
        (Page::Network, "FinishStart"),
        (Page::Detail, "Objective:"),
    ] {
        view.page = page;
        let text_view = render(&mut view);
        assert!(text_view.contains("snapshot revision 0"));
        assert!(text_view.contains(text), "{text}: {text_view}");
    }
    view.move_selection(1);
    assert_eq!(view.state.selected(), Some(1));
    view.move_selection(-1);
    view.move_selection(-1);
    assert_eq!(view.state.selected(), Some(0));
}

#[test]
fn selected_blocked_work_exposes_reason_and_invalid_plans_fail() {
    let mut plan = fixture();
    let id = plan.find_work_by_key("TEST-A").expect("task").id;
    let task = plan.work_items.get_mut(&id).expect("task");
    task.status = WorkStatus::Blocked;
    task.block_reason = Some("waiting for review".into());
    let mut view = View::new(&plan).expect("view");
    view.state.select(view.work.iter().position(|w| w.id == id));
    view.page = Page::Detail;
    assert!(render(&mut view).contains("waiting for review"));
    plan.workspace.name.clear();
    assert!(View::new(&plan).is_err());
}

#[test]
fn empty_workspace_is_safe_to_navigate() {
    let plan = Plan::empty("Empty");
    let mut view = View::new(&plan).expect("empty view");
    view.move_selection(1);
    view.page = Page::Detail;
    assert!(render(&mut view).contains("No work in this workspace"));
}

#[test]
fn gantt_arrow_keys_pan_time_and_preserve_selection_and_plan() {
    let plan = fixture();
    let before = plan.clone();
    let mut view = View::new(&plan).expect("view");
    let selected_key = view.work[view.state.selected().expect("selection")]
        .key
        .to_string();
    assert!(!view.handle_key(KeyCode::Char('g')));
    let initial = render(&mut view);
    assert!(initial.contains("48.0h"));
    assert!(!view.handle_key(KeyCode::Right));
    let panned = render(&mut view);
    assert!(panned.contains("12.0h") && panned.contains("60.0h"));
    assert!(panned.contains(&format!("Selected {selected_key}")));
    view.handle_key(KeyCode::Left);
    assert_eq!(render(&mut view), initial);
    view.handle_key(KeyCode::End);
    assert!(render(&mut view).contains("76.0h"));
    view.handle_key(KeyCode::Home);
    view.handle_key(KeyCode::Char('+'));
    assert!(render(&mut view).contains("24.0h"));
    view.handle_key(KeyCode::Char('f'));
    assert!(render(&mut view).contains("76.0h"));
    view.handle_key(KeyCode::Down);
    view.handle_key(KeyCode::Enter);
    assert!(render(&mut view).contains("Objective:"));
    assert_eq!(view.state.selected(), Some(1));
    assert!(view.handle_key(KeyCode::Char('q')));
    assert_eq!(plan, before);
}

#[test]
fn partial_progress_and_milestone_status_are_visible() {
    let mut plan = fixture();
    let task = plan.find_work_by_key("TEST-A").expect("task").id;
    let item = plan.work_items.get_mut(&task).expect("task");
    item.owner = Some(dpm_model::ActorId::agent("owner"));
    item.status = WorkStatus::InProgress;
    item.reported_progress_percent = 45;
    let mut view = View::new(&plan).expect("view");
    view.handle_key(KeyCode::Char('g'));
    assert!(render(&mut view).contains(" 45%"));
    view.state
        .select(view.work.iter().position(|w| w.key.0 == "TEST-M1"));
    view.handle_key(KeyCode::End);
    let screen = render(&mut view);
    assert!(screen.contains("Milestone pending"));
    view.handle_key(KeyCode::Enter);
    let screen = render(&mut view);
    assert!(screen.contains("verified=false"));
    assert!(screen.contains("FinishStart"));
}

#[test]
fn reached_pending_and_partial_progress_are_visible() {
    let mut plan = fixture();
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    let d = plan.find_work_by_key("TEST-D").expect("d").id;
    for (id, percent) in [(a, 100), (b, 40), (d, 20)] {
        let item = plan.work_items.get_mut(&id).expect("task");
        item.owner = Some(dpm_model::ActorId::agent("owner"));
        item.status = if id == a {
            WorkStatus::Verified
        } else {
            WorkStatus::InProgress
        };
        item.reported_progress_percent = percent;
    }
    let mut reached = plan.find_work_by_key("TEST-M1").expect("milestone").clone();
    reached.id = dpm_model::WorkItemId::new();
    reached.key = dpm_model::Key::new("TEST-REACHED");
    plan.dependencies.push(dpm_model::Dependency {
        predecessor: a,
        successor: reached.id,
        kind: dpm_model::DependencyKind::FinishStart,
        lag_hours: 0.0,
    });
    plan.work_items.insert(reached.id, reached);
    let mut view = View::new(&plan).expect("view");
    view.handle_key(KeyCode::Char('g'));
    let screen = render(&mut view);
    assert!(screen.contains(" 40%"));
    assert!(screen.contains(" 20%"));
    view.state
        .select(view.work.iter().position(|w| w.key.0 == "TEST-REACHED"));
    assert!(render(&mut view).contains("Milestone reached"));
    view.state
        .select(view.work.iter().position(|w| w.key.0 == "TEST-M1"));
    view.handle_key(KeyCode::End);
    let screen = render(&mut view);
    assert!(screen.contains("Milestone pending"));
    assert!(screen.contains("72.0h"));
}

#[test]
fn self_host_roadmap_renders_without_starting_its_contracts() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../examples/self-host/dpm-alpha.json"
    ))
    .expect("self-host");
    let before = plan.clone();
    let mut view = View::new(&plan).expect("view");
    assert!(render(&mut view).contains("DEC-EXECUTE"));
    view.handle_key(KeyCode::Char('g'));
    let screen = render(&mut view);
    assert!(screen.contains("M0-MVP") && screen.contains("MVP-10"));
    assert!(screen.contains(" 0%"));
    view.state
        .select(view.work.iter().position(|w| w.key.0 == "MVP-10"));
    view.handle_key(KeyCode::Enter);
    assert!(render(&mut view).contains("DEC-EXECUTE"));
    assert_eq!(plan, before);
}

#[test]
fn inspector_focus_routes_arrows_and_escape_without_changing_work() {
    let plan = fixture();
    let mut view = View::new(&plan).expect("view");
    view.handle_key(KeyCode::Char('g'));
    render(&mut view);
    let selected = view.state.selected();
    assert!(!view.handle_key(KeyCode::Tab));
    assert!(!view.handle_key(KeyCode::Down));
    assert_eq!(view.state.selected(), selected);
    assert!(render(&mut view).contains("Inspector FOCUS"));
    assert!(!view.handle_key(KeyCode::Esc));
    assert!(!view.handle_key(KeyCode::Down));
    assert_eq!(view.state.selected(), Some(1));
    view.handle_key(KeyCode::Char('?'));
    assert!(!view.handle_key(KeyCode::Esc));
    assert!(view.handle_key(KeyCode::Esc));
}

#[test]
fn work_list_marks_milestone_kind_without_inferring_it_from_the_key() {
    let mut plan = fixture();
    let milestone = plan.find_work_by_key_mut("TEST-M1").expect("milestone");
    milestone.key = dpm_model::Key::new("RELEASE");
    plan.find_work_by_key_mut("TEST-A").expect("task").key = dpm_model::Key::new("M-TASK");
    let mut view = View::new(&plan).expect("view");
    view.handle_key(KeyCode::Char('2'));
    let screen = render(&mut view);
    assert!(screen.contains("◇[M] RELEASE"));
    assert!(!screen.contains("[M] M-TASK"));
}

#[test]
fn file_preview_is_labeled_separately_from_database_snapshot() {
    let plan = fixture();
    let mut view = View::new(&plan).expect("view");
    view.preview = true;
    assert!(render(&mut view).contains("PREVIEW read-only revision 0"));
    view.preview = false;
    assert!(render(&mut view).contains("snapshot revision 0"));
}
