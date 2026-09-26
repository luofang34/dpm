#![allow(clippy::expect_used)]
use super::*;
use ratatui::{Terminal, backend::TestBackend};
fn render(plan: &Plan, width: u16, height: u16) -> String {
    let mut gantt = Gantt::new(plan, chrono::Utc::now()).expect("schedule");
    let work = plan.work_items.values().cloned().collect::<Vec<_>>();
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| gantt.render(frame, frame.area(), plan, &work, Some(0)))
        .expect("draw");
    terminal
        .backend()
        .buffer()
        .content
        .iter()
        .map(|cell| cell.symbol())
        .collect()
}
#[test]
fn renders_critical_bars_milestones_and_units_without_changing_plan() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let before = plan.clone();
    let text = render(&plan, 110, 20);
    assert!(text.contains("Gantt"));
    assert!(text.contains("TEST-A"));
    assert!(text.contains("TEST-M1"));
    assert!(text.contains("####"));
    assert!(text.contains("remaining hours"));
    assert!(text.contains("Selected TEST-A"));
    assert_eq!(plan, before);
}
#[test]
fn narrow_and_empty_workspaces_render_safely() {
    let plan = Plan::empty("empty");
    assert!(render(&plan, 80, 10).contains("No work"));
    assert!(render(&plan, 34, 10).contains("columns"));
    for width in 0..40 {
        for height in 0..6 {
            render(&plan, width, height);
        }
    }
}

#[test]
fn unicode_labels_fit_and_packages_roll_up_children() {
    assert!(Span::raw(truncate("传感器验收任务", 7)).width() <= 7);
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let child = plan.find_work_by_key("TEST-A").expect("child").id;
    let mut package = plan.work_items[&child].clone();
    package.id = dpm_model::WorkItemId::new();
    package.key = dpm_model::Key::new("WP");
    package.kind = WorkKind::WorkPackage;
    package.estimate = None;
    let id = package.id;
    plan.work_items.insert(id, package);
    plan.work_items.get_mut(&child).expect("child").parent = Some(id);
    let gantt = Gantt::new(&plan, chrono::Utc::now()).expect("schedule");
    assert_eq!(
        gantt.bounds(&plan, &plan.work_items[&id]),
        gantt.bounds(&plan, &plan.work_items[&child])
    );
}

#[test]
fn panning_clips_bars_without_pinning_offscreen_milestones() {
    let plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let mut gantt = Gantt::new(&plan, chrono::Utc::now()).expect("gantt");
    gantt.viewport.start = 12.0;
    gantt.viewport.span = 24.0;
    assert_eq!(
        gantt.bar(0.0, 10.0, 12, '#', WorkKind::Task),
        "            "
    );
    assert_eq!(
        gantt.bar(50.0, 50.0, 12, '◇', WorkKind::Milestone),
        "            "
    );
    assert_eq!(
        gantt.bar(0.0, 50.0, 12, '#', WorkKind::Task),
        "<##########>"
    );
    assert_eq!(
        gantt.bar(12.0, 12.0, 12, '◇', WorkKind::Milestone),
        "◇           "
    );
    assert_eq!(
        gantt.bar(36.0, 36.0, 12, '◆', WorkKind::Milestone),
        "           ◆"
    );
}

#[test]
fn dependency_indications_cover_all_relationships_and_signed_lag() {
    use dpm_model::{Dependency, DependencyKind};
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    let a = plan.find_work_by_key("TEST-A").expect("a").id;
    let b = plan.find_work_by_key("TEST-B").expect("b").id;
    for (kind, short, lag) in [
        (DependencyKind::FinishStart, "FS", 2.0),
        (DependencyKind::StartStart, "SS", -1.0),
        (DependencyKind::FinishFinish, "FF", 3.0),
        (DependencyKind::StartFinish, "SF", 4.0),
    ] {
        plan.dependencies = vec![Dependency::new(a, b, kind, lag)];
        let text = render(&plan, 120, 32);
        assert!(
            text.contains(&format!("{short}{lag:+.1}h→TEST-B")),
            "{text}"
        );
        assert!(dependencies::lines(&plan, Some(b))[0].contains(&format!("{short} ({kind:?})")));
    }
}

#[test]
fn viewport_navigation_is_bounded_for_zero_and_large_horizons() {
    for finish in [0.0, 0.5, 200.0, f64::MAX] {
        let mut view = Viewport::new(finish);
        for _ in 0..10 {
            view.pan(true);
            view.zoom(false);
        }
        view.end_of_plan();
        assert!(view.start >= 0.0 && view.end().is_finite());
        assert!(view.end() <= finish.max(1.0));
        view.fit();
        view.pan(true);
        assert_eq!(view.start, 0.0);
        view.zoom(true);
        view.home();
        view.pan(false);
        assert_eq!(view.start, 0.0);
    }
}
#[test]
fn dependency_lines_mark_soft_and_waived_edges() {
    let mut plan: Plan = serde_json::from_str(include_str!(
        "../../../../tests/support/execution-plan.json"
    ))
    .expect("fixture");
    plan.dependencies[0].policy = dpm_model::DependencyPolicy::Soft;
    plan.dependencies[1].policy = dpm_model::DependencyPolicy::Soft;
    plan.dependencies[1].waiver = Some(
        serde_json::from_value(serde_json::json!({
            "actor": {"kind": "Human", "name": "lead"},
            "at": "2026-01-01T00:00:00Z",
            "reason": "overlap accepted"
        }))
        .expect("waiver"),
    );
    let lines = dependencies::lines(&plan, None);
    assert!(lines[0].ends_with(" [soft]"), "{}", lines[0]);
    assert!(lines[1].ends_with(" [soft, waived]"), "{}", lines[1]);
    assert!(!lines[2].contains('['), "{}", lines[2]);
}
