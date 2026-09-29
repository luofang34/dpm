//! Work-package applicability follows from the children, because a package has no lifecycle of
//! its own: it completes only when every child a choice did not exclude is complete, and at least
//! one is. Deriving its state from the same children keeps applicability and completion aligned.

use super::Derived;
use crate::{Applicability, Key, Plan, WorkItem, WorkItemId, WorkKind};

/// Refine packages whose own conditions hold, innermost first so nested packages are settled
/// before their containers read them.
///
/// Any applicable child keeps the package applicable. When every child was excluded by a choice
/// the package is excluded too, as of the latest excluding choice. Otherwise a child still waiting
/// for a choice leaves the package awaiting it, and a package left only with stranded children
/// can never complete.
pub(super) fn derive(plan: &Plan, index: &crate::graph_index::GraphIndex, derived: &mut Derived) {
    let mut packages: Vec<_> = plan
        .work_items
        .values()
        .filter(|w| w.kind == WorkKind::WorkPackage)
        .map(|w| (depth(plan, w), w))
        .collect();
    packages.sort_by(|(a, x), (b, y)| b.cmp(a).then_with(|| x.key.cmp(&y.key)));
    for (_, package) in packages {
        if !derived
            .states
            .get(&package.id)
            .is_some_and(Applicability::is_applicable)
        {
            continue;
        }
        let mut children: Vec<_> = index
            .children
            .get(&package.id)
            .into_iter()
            .flatten()
            .filter_map(|id| plan.work_items.get(id))
            .collect();
        children.sort_by(|a, b| a.key.cmp(&b.key));
        if let Some(state) = from_children(&children, derived) {
            if state.is_not_selected() {
                exclusion_time(package.id, &children, derived);
            }
            derived.states.insert(package.id, state);
        }
    }
}

fn from_children(children: &[&WorkItem], derived: &Derived) -> Option<Applicability> {
    let state = |w: &WorkItem| {
        derived
            .states
            .get(&w.id)
            .unwrap_or(&Applicability::Applicable)
    };
    if children.is_empty() || children.iter().any(|w| state(w).is_applicable()) {
        return None;
    }
    if children.iter().all(|w| state(w).is_not_selected()) {
        let mut decisions: Vec<Key> = children
            .iter()
            .flat_map(|w| match state(w) {
                Applicability::NotSelected { decision, .. } => vec![decision.clone()],
                Applicability::AllChildrenExcluded { decisions } => decisions.clone(),
                _ => Vec::new(),
            })
            .collect();
        decisions.sort();
        decisions.dedup();
        return Some(Applicability::AllChildrenExcluded { decisions });
    }
    let pending = children.iter().find(|w| {
        matches!(
            state(w),
            Applicability::Undecided { .. } | Applicability::AwaitingChoice { .. }
        )
    });
    if let Some(child) = pending {
        return Some(Applicability::AwaitingChoice {
            predecessor: child.key.clone(),
        });
    }
    children
        .iter()
        .find(|w| !state(w).is_not_selected())
        .map(|child| Applicability::ChildrenStranded {
            child: child.key.clone(),
        })
}

/// A parent package releases an excluded child package as a skipped branch at this time.
fn exclusion_time(package: WorkItemId, children: &[&WorkItem], derived: &mut Derived) {
    let latest = children
        .iter()
        .filter_map(|w| derived.choice_at.get(&w.id).copied())
        .chain(derived.choice_at.get(&package).copied())
        .reduce(crate::EventTime::latest);
    if let Some(at) = latest {
        derived.choice_at.insert(package, at);
    }
}

fn depth(plan: &Plan, work: &WorkItem) -> usize {
    let mut depth = 0_usize;
    let mut parent = work.parent;
    while let Some(id) = parent {
        depth = depth.wrapping_add(1);
        if depth > plan.work_items.len() {
            break;
        }
        parent = plan.work_items.get(&id).and_then(|w| w.parent);
    }
    depth
}

#[cfg(test)]
mod tests;
