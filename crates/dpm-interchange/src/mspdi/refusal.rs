//! Source context for a refused import candidate.
//!
//! Review names only local work or a dependency identity; an importer needs the source task or
//! link and the change it attempted to fix the document or plan follow-up work.

use super::ImportResult;
use super::report::ItemReport;
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

/// One end of a source link: the source task and the local work it maps to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkEndpoint {
    /// Source task UID.
    pub uid: i64,
    /// Source task GUID, when present.
    pub guid: Option<String>,
    /// Local work key.
    pub key: Key,
}

impl fmt::Display for LinkEndpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "task UID {} (GUID {}, {})",
            self.uid,
            self.guid.as_deref().unwrap_or("none"),
            self.key
        )
    }
}

/// A source link, or its absence, and the change the candidate makes to one local dependency.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceLinkChange {
    /// Source predecessor task.
    pub predecessor: LinkEndpoint,
    /// Source successor task: the task holding the link.
    pub successor: LinkEndpoint,
    /// Whether the document contains this link; `false` when the candidate removes the dependency
    /// because the document omits it.
    pub in_source: bool,
    /// Change the candidate makes to the dependency.
    pub attempted: String,
}

impl fmt::Display for SourceLinkChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.in_source {
            write!(
                f,
                "MSPDI link from {} to {} would {}",
                self.predecessor, self.successor, self.attempted
            )
        } else {
            write!(
                f,
                "MSPDI document has no link from {} to {}, so the import would {}",
                self.predecessor, self.successor, self.attempted
            )
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

    /// Source link and attempted change at local dependency `dependency` (its identity as text),
    /// when the candidate keeps it for a source link or removes it between imported work.
    #[must_use]
    pub fn source_link_change(&self, current: &Plan, dependency: &str) -> Option<SourceLinkChange> {
        let local = current
            .dependencies
            .iter()
            .find(|d| d.id.to_string() == dependency)?;
        let describe = |d: &Dependency| describe(current, &self.candidate, d);
        let Some(after) = self.candidate.find_dependency(local.id) else {
            let by_work =
                |id: WorkItemId| self.endpoint(|i| i.work.as_ref().is_some_and(|w| w.id == id));
            return Some(SourceLinkChange {
                predecessor: by_work(local.predecessor)?,
                successor: by_work(local.successor)?,
                in_source: false,
                attempted: format!("remove dependency {}", describe(local)),
            });
        };
        let link = self
            .report
            .links
            .iter()
            .find(|l| l.dependencies.contains(&local.id))?;
        let predecessor_uid = link.predecessor_uid?;
        let attempted = if local == after {
            format!("keep dependency {} unchanged", describe(local))
        } else {
            format!(
                "change dependency {} to {}",
                describe(local),
                describe(after)
            )
        };
        Some(SourceLinkChange {
            predecessor: self.endpoint(|i| i.uid == predecessor_uid)?,
            successor: self.endpoint(|i| i.uid == link.successor_uid)?,
            in_source: true,
            attempted,
        })
    }

    fn endpoint(&self, matches: impl Fn(&ItemReport) -> bool) -> Option<LinkEndpoint> {
        let item = self.report.items.iter().find(|i| matches(i))?;
        Some(LinkEndpoint {
            uid: item.uid,
            guid: item.guid.clone(),
            key: item.work.as_ref()?.key.clone(),
        })
    }
}

/// Relation, local work keys and lag of one dependency.
fn describe(current: &Plan, candidate: &Plan, d: &Dependency) -> String {
    let key = |id: WorkItemId| {
        candidate
            .work_items
            .get(&id)
            .or_else(|| current.work_items.get(&id))
            .map_or_else(|| id.to_string(), |w| w.key.0.clone())
    };
    format!(
        "{} {} -> {} lag {} h",
        d.kind.abbreviation(),
        key(d.predecessor),
        key(d.successor),
        d.lag_hours
    )
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
    let describe = |d: &Dependency| describe(current, candidate, d);
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
