//! The operation log indexed by the work and decisions each operation names.

use crate::{Command, Operation};
use chrono::{DateTime, Utc};
use dpm_model::{DecisionId, WorkItemId};
use std::collections::BTreeMap;

/// Operations of one log, in append order per work item and decision.
pub(crate) struct HistoryIndex<'a> {
    work: BTreeMap<WorkItemId, Vec<&'a Operation>>,
    ordered: Vec<&'a Operation>,
    opened: BTreeMap<DecisionId, &'a Operation>,
    decided: BTreeMap<DecisionId, &'a Operation>,
}

impl<'a> HistoryIndex<'a> {
    pub(crate) fn new(history: &'a [Operation]) -> Self {
        let mut index = Self {
            work: BTreeMap::new(),
            ordered: Vec::new(),
            opened: BTreeMap::new(),
            decided: BTreeMap::new(),
        };
        for operation in history {
            if let Some(work) = target(&operation.command) {
                index.work.entry(work).or_default().push(operation);
                index.ordered.push(operation);
            }
            match &operation.command {
                Command::Decide { decision, .. } => {
                    index.decided.insert(*decision, operation);
                }
                Command::ApplyChange { changes, .. } => {
                    for added in changes.iter().filter(|c| {
                        c.collection == "decisions" && c.before.is_null() && !c.after.is_null()
                    }) {
                        let id = added.id.clone().map(serde_json::Value::String);
                        if let Some(decision) = id.and_then(|id| serde_json::from_value(id).ok()) {
                            index.opened.entry(decision).or_insert(operation);
                        }
                    }
                }
                _ => {}
            }
        }
        index
    }

    /// Operations naming a work item, in append order.
    pub(crate) fn of(&self, work: WorkItemId) -> &[&'a Operation] {
        self.work.get(&work).map_or(&[], Vec::as_slice)
    }

    /// The latest operation on a work item matching a predicate.
    pub(crate) fn last(
        &self,
        work: WorkItemId,
        matches: impl Fn(&Command) -> bool,
    ) -> Option<&'a Operation> {
        self.of(work)
            .iter()
            .rev()
            .find(|op| matches(&op.command))
            .copied()
    }

    /// Every operation in the log that names some work item, in append order.
    pub(crate) fn work_operations(&self) -> &[&'a Operation] {
        &self.ordered
    }

    /// The reviewed change that added a decision.
    pub(crate) fn opened(&self, decision: DecisionId) -> Option<&'a Operation> {
        self.opened.get(&decision).copied()
    }

    /// The latest `decide` operation on a decision.
    pub(crate) fn decided(&self, decision: DecisionId) -> Option<&'a Operation> {
        self.decided.get(&decision).copied()
    }
}

/// The work item a lifecycle or ownership command names.
pub(crate) fn target(command: &Command) -> Option<WorkItemId> {
    match command {
        Command::Claim { work }
        | Command::Release { work, .. }
        | Command::Handoff { work, .. }
        | Command::Start { work, .. }
        | Command::Submit { work, .. }
        | Command::Verify { work, .. }
        | Command::Reject { work, .. }
        | Command::Block { work, .. }
        | Command::Unblock { work } => Some(*work),
        _ => None,
    }
}

/// When a lifecycle event occurred: its backfilled occurrence time, else the commit time.
pub(crate) fn occurred(operation: &Operation) -> DateTime<Utc> {
    backfilled(&operation.command).unwrap_or(operation.timestamp)
}

/// The occurrence time a start, submit or verify named, if it was recorded after the fact.
pub(crate) fn backfilled(command: &Command) -> Option<DateTime<Utc>> {
    match command {
        Command::Start { occurred_at, .. }
        | Command::Submit { occurred_at, .. }
        | Command::Verify { occurred_at, .. } => *occurred_at,
        _ => None,
    }
}
