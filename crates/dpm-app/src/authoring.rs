//! Plan authoring aids: the JSON Schema of the portable plan and a starter graph for an empty
//! workspace, so an agent can write a proposal from the contract instead of from Rust source.

use crate::AppError;
use dpm_model::Plan;
use serde_json::{Value, json};

const SCHEMA: &str = include_str!("authoring/plan.schema.json");

/// JSON Schema (draft 2020-12) of the plan accepted by `plan diff` / `propose_change` and
/// `plan apply` / `apply_change`, and produced by `export`.
pub fn plan_schema() -> Result<Value, AppError> {
    Ok(serde_json::from_str(SCHEMA)?)
}

/// A minimal valid proposal for the open workspace: its identity and revision, one project, a
/// package, two Proposed tasks with complete contracts, a milestone, a requirement, a decision
/// gate and a risk. Identifiers derive from the workspace identity, so every adapter returns the
/// same template.
pub(crate) fn plan_template(current: &Plan) -> Result<Value, AppError> {
    // Apply treats omitted entities as reviewed deletions; a template must never stand in for them.
    if !current.projects.is_empty() || !current.work_items.is_empty() {
        return Err(AppError::InvalidRequest(
            "the plan template is only for a workspace without projects or work; edit the export instead".into(),
        ));
    }
    let seed = u128::from_str_radix(&current.workspace.id.to_string().replace('-', ""), 16)
        .map_err(|error| AppError::InvalidRequest(format!("workspace identity: {error}")))?;
    let id = |salt: u128| derived_id(seed, salt);
    let (project, package, design, build, done) = (id(1), id(2), id(3), id(4), id(5));
    let (requirement, decision, risk) = (id(6), id(7), id(8));
    let template = json!({
        "format_version": current.format_version,
        "workspace": current.workspace,
        "revision": current.revision,
        "assets": {},
        "artifacts": {},
        "projects": {project.clone(): {
            "id": project, "key": "TEMPLATE", "parent": null,
            "title": "Replace with the project title",
            "objective": "State the outcome this project must deliver and why it matters."
        }},
        "requirements": {requirement.clone(): {
            "id": requirement, "key": "TEMPLATE-REQ", "project": project,
            "title": "Replace with a constraint", "statement": "State a condition every result must satisfy."
        }},
        "work_items": {
            package.clone(): container(&package, &project, "TEMPLATE-WP", "WorkPackage", "Group the delivery work", 100),
            design.clone(): task(&design, &project, &package, &requirement, "TEMPLATE-DESIGN", "Write the design", 100),
            build.clone(): task(&build, &project, &package, &requirement, "TEMPLATE-BUILD", "Build the result", 200),
            done.clone(): container(&done, &project, "TEMPLATE-DONE", "Milestone", "Delivery reached", 200),
        },
        "decisions": {decision.clone(): {
            "id": decision, "key": "TEMPLATE-DEC", "project": project,
            "question": "Which approach does the build follow?", "status": "Open", "outcome": null,
            "blocks": [build],
            "options": [{"key": "simple", "label": "Simplest approach"}, {"key": "extensible", "label": "Extensible approach"}]
        }},
        "risks": {risk.clone(): {
            "id": risk, "key": "TEMPLATE-RISK", "project": project,
            "description": "Replace with an uncertainty that could delay the work.",
            "probability": 0.2, "impact": "Medium", "related_work": [build],
            "mitigation": "Replace with the planned response."
        }},
        "dependencies": [
            {"id": id(9), "predecessor": design, "successor": build, "kind": "FinishStart", "lag_hours": 0.0},
            {"id": id(10), "predecessor": build, "successor": done, "kind": "FinishStart", "lag_hours": 0.0}
        ]
    });
    let plan: Plan = serde_json::from_value(template)?;
    plan.validate().map_err(dpm_store::StoreError::from)?;
    Ok(serde_json::to_value(plan)?)
}

fn task(
    id: &str,
    project: &str,
    parent: &str,
    requirement: &str,
    key: &str,
    title: &str,
    order: u16,
) -> Value {
    json!({
    "id": id,
    "key": key,
    "project": project,
    "parent": parent,
    "kind": "Task",
    "title": title,
    "order": [order],
    "contract": {"objective": "State why this work matters and what it unblocks.",
    "acceptance": [{"text": "State an observable, independently checkable result."}],
    "instructions": {
                "steps": [{"action": "Describe the first action.", "expected_result": "Describe what it produces."}],
                "in_scope": ["Name what this task changes."],
                "out_of_scope": ["Name what it must not change."],
                "verification": ["Name the command or review that checks the acceptance."]
            },
    "capabilities": ["replace-with-a-capability"],
    "requirement_ids": [requirement],
    "assets": []},
    "execution": {"status": "Proposed",
    "artifact_ids": [],
    "owner": null,
    "block_reason": null},
    "schedule": {"priority": "P2",
    "estimate": {"optimistic_hours": 2.0, "likely_hours": 4.0, "pessimistic_hours": 8.0}}
    })
}

fn container(id: &str, project: &str, key: &str, kind: &str, title: &str, order: u16) -> Value {
    json!({
    "id": id,
    "key": key,
    "project": project,
    "parent": null,
    "kind": kind,
    "title": title,
    "order": [order],
    "contract": {"objective": "State the outcome this groups or marks.",
    "acceptance": [{"text": "Every contained or preceding task is verified."}],
    "capabilities": [],
    "requirement_ids": [],
    "assets": []},
    "execution": {"status": "Planned",
    "artifact_ids": [],
    "owner": null,
    "block_reason": null},
    "schedule": {"priority": "P2",
    "estimate": null}
    })
}

/// A random-format (version 4, RFC 4122 variant) UUID fixed by the workspace and a salt.
fn derived_id(seed: u128, salt: u128) -> String {
    let mixed = seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15_f39c_c060_5ced_c834);
    let bits = (mixed & !(0xf << 76) & !(0x3 << 62)) | (0x4 << 76) | (0x2 << 62);
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        bits >> 96,
        (bits >> 80) & 0xffff,
        (bits >> 64) & 0xffff,
        (bits >> 48) & 0xffff,
        bits & 0xffff_ffff_ffff
    )
}

#[cfg(test)]
mod tests;
