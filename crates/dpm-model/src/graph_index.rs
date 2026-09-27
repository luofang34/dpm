//! Query-local adjacency preserves source order without rescanning the graph per item.
use crate::{DecisionId, Plan, WorkItemId};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct GraphIndex {
    pub(crate) incoming: BTreeMap<WorkItemId, Vec<usize>>,
    pub(crate) children: BTreeMap<WorkItemId, Vec<WorkItemId>>,
    pub(crate) decisions: BTreeMap<WorkItemId, Vec<DecisionId>>,
}
impl GraphIndex {
    pub(crate) fn new(plan: &Plan) -> Self {
        let mut index = Self::default();
        for (position, edge) in plan.dependencies.iter().enumerate() {
            if edge.waiver.is_none() {
                index
                    .incoming
                    .entry(edge.successor)
                    .or_default()
                    .push(position);
            }
        }
        for work in plan.work_items.values() {
            if let Some(parent) = work.parent {
                index.children.entry(parent).or_default().push(work.id);
            }
        }
        for decision in plan.decisions.values() {
            for id in &decision.blocks {
                index.decisions.entry(*id).or_default().push(decision.id);
            }
        }
        index
    }
}
