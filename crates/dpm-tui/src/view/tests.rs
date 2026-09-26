use super::*;
use ratatui::{Terminal, backend::TestBackend};

fn fixture() -> Plan {
    serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture")
}

fn render(view: &mut View) -> String {
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
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
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
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.state.select(view.work.iter().position(|w| w.id == id));
    view.page = Page::Detail;
    assert!(render(&mut view).contains("waiting for review"));
    plan.workspace.name.clear();
    assert!(View::new(&plan, chrono::Utc::now()).is_err());
}

#[test]
fn empty_workspace_is_safe_to_navigate() {
    let plan = Plan::empty("Empty");
    let mut view = View::new(&plan, chrono::Utc::now()).expect("empty view");
    view.move_selection(1);
    view.page = Page::Detail;
    assert!(render(&mut view).contains("No work in this workspace"));
}

#[test]
fn gantt_arrow_keys_pan_time_and_preserve_selection_and_plan() {
    let plan = fixture();
    let before = plan.clone();
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
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
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
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
    plan.dependencies.push(dpm_model::Dependency::new(
        a,
        reached.id,
        dpm_model::DependencyKind::FinishStart,
        0.0,
    ));
    plan.work_items.insert(reached.id, reached);
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
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
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
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
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
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
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.handle_key(KeyCode::Char('2'));
    let screen = render(&mut view);
    assert!(screen.contains("◇[M] RELEASE"));
    assert!(!screen.contains("[M] M-TASK"));
}

#[test]
fn file_preview_is_labeled_separately_from_database_snapshot() {
    let plan = fixture();
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.preview = true;
    assert!(render(&mut view).contains("PREVIEW read-only revision 0"));
    view.preview = false;
    assert!(render(&mut view).contains("snapshot revision 0"));
}

#[test]
fn detail_exposes_complete_contract_context_and_latest_review_by_scrolling() {
    let mut plan = fixture();
    let work = plan.find_work_by_key_mut("TEST-A").expect("work");
    work.instructions = Some(dpm_model::WorkInstructions {
        steps: vec![dpm_model::ExecutionStep {
            action: "Inspect the calibration sample".into(),
            expected_result: "Recorded sample measurements".into(),
        }],
        in_scope: vec!["Sample calibration".into()],
        out_of_scope: vec!["Hardware redesign".into()],
        verification: vec!["Compare calibration residuals".into()],
    });
    work.owner = Some(dpm_model::ActorId::agent("owner"));
    work.status = WorkStatus::InProgress;
    work.last_rejection = Some(dpm_model::ReviewRejection {
        actor: dpm_model::ActorId::human("reviewer"),
        at: "2026-09-26T00:00:00Z".parse().expect("timestamp"),
        reason: "Missing measured evidence".into(),
    });
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.handle_key(KeyCode::Enter);
    let mut text = render(&mut view);
    for _ in 0..10 {
        view.handle_key(KeyCode::PageDown);
        text.push_str(&render(&mut view));
    }
    for expected in [
        "Inspect the calibration sample",
        "Recorded sample measurements",
        "Sample calibration",
        "Hardware redesign",
        "Compare calibration residuals",
        "Missing measured evidence",
        "TEST-REPO",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
}

#[test]
fn refresh_preserves_selection_and_viewport_and_rejects_source_switches_atomically() {
    let mut plan = fixture();
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.handle_key(KeyCode::Char('g'));
    view.handle_key(KeyCode::Right);
    view.handle_key(KeyCode::Down);
    let selected = view.work[view.state.selected().expect("selection")].id;
    plan.work_items.get_mut(&selected).expect("work").title = "Refreshed task title".into();
    plan.revision = 1;
    view.refresh(&plan, chrono::Utc::now()).expect("refresh");
    assert_eq!(
        view.work[view.state.selected().expect("selection")].id,
        selected
    );
    let updated = render(&mut view);
    assert!(
        updated.contains("revision 1")
            && updated.contains("Refreshed task title")
            && updated.contains("12.0h")
    );
    let mut invalid = plan.clone();
    invalid.workspace.id = dpm_model::WorkspaceId::new();
    assert!(view.refresh(&invalid, chrono::Utc::now()).is_err());
    assert_eq!(view.plan, plan);
    assert_eq!(render(&mut view), updated);
    view.reload_failed(&"missing source");
    assert!(render(&mut view).contains("missing source"));
    view.refresh(&plan, chrono::Utc::now()).expect("retry");
    assert!(view.notice.is_none());
}

#[test]
fn now_exposes_all_completion_percentiles_and_detail_separates_float() {
    let plan = fixture();
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    let now = render(&mut view);
    for marker in ["P50", "P80", "P95"] {
        assert!(now.contains(marker));
    }
    let work = plan.find_work_by_key("TEST-A").expect("work");
    let explanation =
        dpm_engine::explain_work(&plan, work.id, chrono::Utc::now()).expect("explain");
    let detail = crate::detail::text(&plan, &explanation);
    assert!(detail.contains("Free float") && detail.contains("total float"));
}

#[test]
fn detail_shows_recorded_events_and_every_transition_gate() {
    use chrono::TimeZone;
    let mut plan = fixture();
    let work = plan.find_work_by_key("TEST-A").expect("work").id;
    let at = chrono::Utc
        .with_ymd_and_hms(2026, 9, 1, 8, 0, 0)
        .single()
        .expect("time");
    let worker = dpm_model::ActorId::agent("worker");
    for command in [
        dpm_engine::Command::Claim { work },
        dpm_engine::Command::Start { work },
    ] {
        dpm_engine::apply_command(&mut plan, worker.clone(), command, at).expect("execute");
    }
    let explanation = dpm_engine::explain_work(&plan, work, at).expect("explain");
    let detail = crate::detail::text(&plan, &explanation);
    assert!(
        detail.contains("started at 2026-09-01 08:00:00 UTC"),
        "{detail}"
    );
    assert!(detail.contains("Transitions:") && detail.contains("submit: permitted now"));
    assert!(detail.contains("start: work lifecycle state is InProgress"));
    let successor = plan.find_work_by_key("TEST-B").expect("work").id;
    let explanation = dpm_engine::explain_work(&plan, successor, at).expect("explain");
    let detail = crate::detail::text(&plan, &explanation);
    assert!(
        detail.contains("claim: awaits the verified finish of TEST-A"),
        "{detail}"
    );
}

#[test]
fn conditional_work_shows_scenarios_exclusions_and_the_stranded_reason() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture");
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    let now = render(&mut view);
    assert!(now.contains("Open choices DEC-SUPPLIER"), "{now}");
    assert!(now.contains("DEC-SUPPLIER=A") && now.contains("DEC-SUPPLIER=B"));
    let execution = now
        .split("Execution:")
        .nth(1)
        .and_then(|rest| rest.split("Open choices").next())
        .expect("execution line");
    assert!(
        !execution.contains("P50"),
        "no blended percentile while a choice is open: {execution}"
    );

    let decision = plan.decisions.keys().next().copied().expect("decision");
    dpm_engine::apply_command(
        &mut plan,
        dpm_model::ActorId::human("lead"),
        dpm_engine::Command::Decide {
            decision,
            outcome: "B".into(),
        },
        chrono::Utc::now(),
    )
    .expect("decide");
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.page = Page::Work;
    assert!(render(&mut view).contains("[not selected]"));
    view.page = Page::Gantt;
    assert!(render(&mut view).contains("outside the active graph"));
    let audit = plan.find_work_by_key("SUP-A-AUDIT").expect("work").id;
    let explanation = dpm_engine::explain_work(&plan, audit, chrono::Utc::now()).expect("explain");
    let detail = crate::detail::text(&plan, &explanation);
    assert!(detail.contains("never releases from it"), "{detail}");
}

/// An unconditional package whose only task applies to supplier A, with supplier B selected.
fn excluded_package_plan() -> Plan {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/conditional-plan.json"
    ))
    .expect("fixture");
    let decision = plan.decisions.keys().next().copied().expect("decision");
    let mut package = plan.find_work_by_key("SUP-PKG-A").expect("package").clone();
    package.id = dpm_model::WorkItemId::new();
    package.key = dpm_model::Key::new("X-PKG");
    package.condition = None;
    let mut task = plan.find_work_by_key("SUP-A-QUOTE").expect("task").clone();
    task.id = dpm_model::WorkItemId::new();
    task.key = dpm_model::Key::new("X-A1");
    task.parent = Some(package.id);
    task.condition = Some(dpm_model::WorkCondition {
        decision,
        option: "A".into(),
    });
    plan.work_items.insert(package.id, package);
    plan.work_items.insert(task.id, task);
    dpm_engine::apply_command(
        &mut plan,
        dpm_model::ActorId::human("lead"),
        dpm_engine::Command::Decide {
            decision,
            outcome: "B".into(),
        },
        chrono::Utc::now(),
    )
    .expect("decide");
    plan
}

/// The rendered row that mentions `key`, at a width that keeps the whole Gantt bar text.
fn row_of(view: &mut View, key: &str) -> String {
    let mut terminal = Terminal::new(TestBackend::new(240, 40)).expect("test terminal");
    terminal.draw(|frame| view.render(frame)).expect("render");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(240)
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .find(|row| row.contains(key))
        .expect("row")
}

#[test]
fn a_package_of_only_unselected_work_reads_its_exclusion_from_the_timeline() {
    let plan = excluded_package_plan();
    let mut view = View::new(&plan, chrono::Utc::now()).expect("view");
    view.page = Page::Gantt;
    let bar = row_of(&mut view, "X-PKG");
    assert!(
        bar.contains("outside the active graph: all children excluded"),
        "{bar}"
    );
    view.page = Page::Work;
    let listed = row_of(&mut view, "X-PKG");
    assert!(listed.contains("[not selected]"), "{listed}");
    let id = plan.find_work_by_key("X-PKG").expect("package").id;
    let explanation = dpm_engine::explain_work(&plan, id, chrono::Utc::now()).expect("explain");
    let detail = crate::detail::text(&plan, &explanation);
    assert!(
        detail.contains("Applicability: all children excluded"),
        "{detail}"
    );
}

fn screen_rows(view: &mut View, width: u16, height: u16) -> String {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("test terminal");
    terminal.draw(|frame| view.render(frame)).expect("render");
    terminal
        .backend()
        .buffer()
        .content
        .chunks(usize::from(width))
        .map(|row| row.iter().map(|cell| cell.symbol()).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn gantt_minimum_size_message_states_the_terminal_size_it_needs() {
    let mut view = View::new(&fixture(), chrono::Utc::now()).expect("view");
    view.page = Page::Gantt;
    for (width, height) in [(37, 15), (38, 14)] {
        let screen = screen_rows(&mut view, width, height);
        assert!(
            screen.contains("38 columns") && screen.contains("15 rows"),
            "{screen}"
        );
    }
    assert!(!screen_rows(&mut view, 38, 15).contains("15 rows"));
    view.reload_failed(&"disk unavailable");
    let screen = screen_rows(&mut view, 38, 17);
    assert!(screen.contains("18 rows"), "{screen}");
}
