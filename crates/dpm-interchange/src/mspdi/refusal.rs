//! Source context for a refused import candidate.
//!
//! Review names only local work; an importer needs the source task and the change it attempted
//! to fix the document or plan follow-up work.

use super::ImportResult;
use dpm_model::{Dependency, Key, Plan, WorkItemId};
use serde::{Deserialize, Serialize};
use std::fmt;

/// A source task and every change the candidate makes at the work it maps to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceChange {
    /// Source task UID.
    pub uid: i64,
    /// Source task GUID, when present.
    pub guid: Option<String>,
    /// Local work key.
    pub key: Key,
    /// Field and dependency changes the candidate makes at this work.
    pub attempted: Vec<String>,
}

impl fmt::Display for SourceChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "MSPDI task UID {} (GUID {}) maps to {}",
            self.uid,
            self.guid.as_deref().unwrap_or("none"),
            self.key
        )?;
        if self.attempted.is_empty() {
            write!(
                f,
                " and changes none of its fields or incoming dependencies"
            )
        } else {
            write!(f, " and would change {}", self.attempted.join("; "))
        }
    }
}

impl ImportResult {
    /// Source task and attempted changes at local work `key`, when an imported task maps to it.
    #[must_use]
    pub fn source_change(&self, current: &Plan, key: &str) -> Option<SourceChange> {
        let item = self
            .report
            .items
            .iter()
            .find(|i| i.work.as_ref().is_some_and(|w| w.key.0 == key))?;
        let id = item.work.as_ref()?.id;
        let mut attempted: Vec<String> = item
            .changes
            .iter()
            .map(|c| format!("{} {:?} -> {:?}", c.field, c.before, c.after))
            .collect();
        attempted.extend(edge_changes(current, &self.candidate, id));
        Some(SourceChange {
            uid: item.uid,
            guid: item.guid.clone(),
            key: item.work.as_ref()?.key.clone(),
            attempted,
        })
    }
}

/// Added, removed and re-lagged dependencies into `id`: the prerequisite basis review protects.
fn edge_changes(current: &Plan, candidate: &Plan, id: WorkItemId) -> Vec<String> {
    let into = |plan: &'_ Plan| -> Vec<Dependency> {
        plan.dependencies
            .iter()
            .filter(|d| d.successor == id)
            .cloned()
            .collect()
    };
    let key = |id: WorkItemId| {
        candidate
            .work_items
            .get(&id)
            .or_else(|| current.work_items.get(&id))
            .map_or_else(|| id.to_string(), |w| w.key.0.clone())
    };
    let describe = |d: &Dependency| {
        format!(
            "{} {} -> {} lag {} h",
            d.kind.abbreviation(),
            key(d.predecessor),
            key(d.successor),
            d.lag_hours
        )
    };
    let (before, after) = (into(current), into(candidate));
    let mut found = Vec::new();
    for edge in &after {
        match before.iter().find(|d| d.id == edge.id) {
            None => found.push(format!("add dependency {}", describe(edge))),
            Some(local) if local != edge => found.push(format!(
                "change dependency {} to {}",
                describe(local),
                describe(edge)
            )),
            Some(_) => {}
        }
    }
    for edge in before
        .iter()
        .filter(|d| !after.iter().any(|a| a.id == d.id))
    {
        found.push(format!("remove dependency {}", describe(edge)));
    }
    found
}
