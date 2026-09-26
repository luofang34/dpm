//! Task data outside the supported subset, reported rather than silently dropped.
//!
//! Derived schedule values (dates, slack, criticality, remaining duration) are not listed: DPM
//! recomputes them from the imported graph.

use super::report::Finding;
use super::source::{children, flag, text};
use roxmltree::Node;

const CONSTRAINTS: [&str; 8] = [
    "As Soon As Possible",
    "As Late As Possible",
    "Must Start On",
    "Must Finish On",
    "Start No Earlier Than",
    "Start No Later Than",
    "Finish No Earlier Than",
    "Finish No Later Than",
];

const PROGRESS: &str =
    "not imported; source progress never completes, submits or verifies local work";

pub(crate) fn task_findings(node: Node) -> Vec<Finding> {
    let mut findings = Vec::new();
    let number = |name: &'static str| {
        text(node, name)
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(0.0)
    };
    let constraint = number("ConstraintType");
    if constraint != 0.0 {
        let label = CONSTRAINTS
            .get(constraint as usize)
            .copied()
            .unwrap_or("unknown");
        let date = text(node, "ConstraintDate").unwrap_or("no date");
        findings.push(Finding::new(
            "constraint",
            format!("{label} ({date}) not imported; DPM has no date constraints"),
        ));
    }
    if let Some(deadline) = text(node, "Deadline") {
        findings.push(Finding::new("deadline", format!("{deadline} not imported")));
    }
    if text(node, "CalendarUID").is_some_and(|v| v.trim() != "-1") {
        findings.push(Finding::new(
            "calendar",
            "task calendar not imported; DPM schedules elapsed hours",
        ));
    }
    for field in [
        "PercentComplete",
        "PercentWorkComplete",
        "PhysicalPercentComplete",
    ] {
        let value = number(field);
        if value != 0.0 {
            findings.push(Finding::new(field, format!("{value}% {PROGRESS}")));
        }
    }
    let actuals: Vec<_> = [
        "ActualStart",
        "ActualFinish",
        "ActualDuration",
        "ActualWork",
    ]
    .into_iter()
    .filter(|name| text(node, name).is_some_and(|v| !is_zero_duration(v)))
    .collect();
    if !actuals.is_empty() {
        findings.push(Finding::new(
            "actuals",
            format!("{} {PROGRESS}", actuals.join(", ")),
        ));
    }
    count(&mut findings, node, "Baseline", "baselines");
    count(&mut findings, node, "ExtendedAttribute", "custom_fields");
    count(&mut findings, node, "OutlineCode", "outline_codes");
    count(&mut findings, node, "TimephasedData", "timephased_data");
    if flag(node, "Manual") {
        findings.push(Finding::new(
            "manual_scheduling",
            "manually scheduled dates not imported; DPM derives dates from the graph",
        ));
    }
    if flag(node, "Recurring") {
        findings.push(Finding::new(
            "recurrence",
            "recurring task pattern not imported",
        ));
    }
    if number("FixedCost") != 0.0 || number("Cost") != 0.0 {
        findings.push(Finding::new("cost", "cost not imported"));
    }
    if text(node, "HyperlinkAddress").is_some_and(|v| !v.trim().is_empty()) {
        findings.push(Finding::new("hyperlink", "hyperlink not imported"));
    }
    findings
}

fn count(findings: &mut Vec<Finding>, node: Node, element: &'static str, field: &str) {
    let found = children(node, element).count();
    if found > 0 {
        findings.push(Finding::new(
            field,
            format!("{found} {element} element(s) not imported"),
        ));
    }
}

fn is_zero_duration(value: &str) -> bool {
    super::encoding::parse_duration(value) == Ok(0)
}
