//! The local fields a candidate changes on existing work, stated as before/after values so the
//! report describes the mutation itself rather than what the source contained.

use crate::mspdi::report::FieldChange;
use dpm_model::{Key, ThreePointEstimate, WorkItem, WorkItemId};
use std::collections::BTreeMap;

/// Fields an import maps; every other work field is local and never changes.
pub(super) fn between(
    before: &WorkItem,
    after: &WorkItem,
    keys: &BTreeMap<WorkItemId, &Key>,
) -> Vec<FieldChange> {
    let parent = |parent: Option<WorkItemId>| {
        parent.map_or_else(
            || "(top level)".to_owned(),
            |id| {
                keys.get(&id)
                    .map_or_else(|| id.to_string(), |k| k.0.clone())
            },
        )
    };
    let fields = [
        ("title", before.title.clone(), after.title.clone()),
        (
            "objective",
            before.objective.clone(),
            after.objective.clone(),
        ),
        (
            "kind",
            format!("{:?}", before.kind),
            format!("{:?}", after.kind),
        ),
        ("parent", parent(before.parent), parent(after.parent)),
        (
            "priority",
            format!("{:?}", before.priority),
            format!("{:?}", after.priority),
        ),
        (
            "estimate",
            estimate(before.estimate),
            estimate(after.estimate),
        ),
    ];
    fields
        .into_iter()
        .filter(|(_, from, to)| from != to)
        .map(|(field, before, after)| FieldChange {
            field: field.into(),
            before,
            after,
        })
        .collect()
}

fn estimate(estimate: Option<ThreePointEstimate>) -> String {
    estimate.map_or_else(
        || "unestimated".to_owned(),
        |e| {
            format!(
                "O={} M={} P={} h",
                e.optimistic_hours, e.likely_hours, e.pessimistic_hours
            )
        },
    )
}
