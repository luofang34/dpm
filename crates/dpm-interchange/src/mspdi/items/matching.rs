//! Opt-in match of GUID-less source tasks to existing work by title path.
//!
//! A tool that drops GUIDs (OmniPlan) cannot say which local work a task came from, so this match
//! runs only when the import asks for it. A title path is the chain of titles from the project
//! root to the item: the source outline's names, and the local parent chain's titles within the
//! target project. A task matches only when exactly one local item and no other GUID-less task
//! share its path; every other overlap is an ambiguity that refuses the import.

use crate::mspdi::source::SourceProject;
use dpm_model::{Plan, ProjectId, WorkItemId};
use std::collections::BTreeMap;

pub(super) struct Matcher {
    /// Title path of every GUID-less source task that needs an identity.
    paths: BTreeMap<i64, Path>,
    matches: BTreeMap<i64, WorkItemId>,
}

impl Matcher {
    /// The matcher and every ambiguity found; the caller refuses the import when any exist.
    pub(super) fn new(
        current: &Plan,
        source: &SourceProject,
        project: ProjectId,
    ) -> (Self, Vec<String>) {
        let paths = source_paths(source);
        let local = local_paths(current, project);
        let mut by_path: BTreeMap<&Path, Vec<i64>> = BTreeMap::new();
        for (uid, path) in &paths {
            by_path.entry(path).or_default().push(*uid);
        }
        let mut matches = BTreeMap::new();
        let mut ambiguities = Vec::new();
        for (path, uids) in &by_path {
            let Some(candidates) = local.get(*path) else {
                continue;
            };
            let path = display(path);
            let keys = || {
                candidates
                    .iter()
                    .filter_map(|id| current.work_items.get(id))
                    .map(|w| w.key.0.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            match (uids.as_slice(), candidates.as_slice()) {
                ([uid], [id]) => {
                    matches.insert(*uid, *id);
                }
                ([uid], _) => ambiguities.push(format!(
                    "task UID {uid} has title path {path:?}, shared by existing work {}",
                    keys()
                )),
                _ => ambiguities.push(format!(
                    "tasks UID {} share title path {path:?} with existing work {}",
                    uids.iter()
                        .map(i64::to_string)
                        .collect::<Vec<_>>()
                        .join(", "),
                    keys()
                )),
            }
        }
        (Self { paths, matches }, ambiguities)
    }

    /// The one existing work item a task matches.
    pub(super) fn find(&self, uid: i64) -> Option<WorkItemId> {
        self.matches.get(&uid).copied()
    }

    /// The task's title path as the report shows it.
    pub(super) fn path(&self, uid: i64) -> Option<String> {
        self.paths.get(&uid).map(display)
    }
}

/// Titles from the project root down; compared title by title, so a title containing the
/// display separator never matches a deeper path.
type Path = Vec<String>;

fn display(path: &Path) -> String {
    path.join(" / ")
}

/// Title paths of GUID-less, importable tasks in outline order; the UID 0 / level 0 project
/// summary stands for the project root and contributes no title.
fn source_paths(source: &SourceProject) -> BTreeMap<i64, Path> {
    let mut stack: Vec<(u32, &str)> = Vec::new();
    let mut paths = BTreeMap::new();
    for task in &source.tasks {
        if task.is_project_summary() {
            continue;
        }
        while stack
            .last()
            .is_some_and(|(level, _)| *level >= task.outline_level)
        {
            stack.pop();
        }
        stack.push((task.outline_level, task.name.as_str()));
        if task.guid.is_none() && task.exclusion.is_none() {
            let titles = stack.iter().map(|(_, name)| (*name).to_owned()).collect();
            paths.insert(task.uid, titles);
        }
    }
    paths
}

/// Title paths of every work item in the target project, each with every item that has it.
fn local_paths(current: &Plan, project: ProjectId) -> BTreeMap<Path, Vec<WorkItemId>> {
    let mut found: BTreeMap<Path, Vec<WorkItemId>> = BTreeMap::new();
    for work in current.work_items.values().filter(|w| w.project == project) {
        let mut titles = vec![work.title.clone()];
        let mut parent = work.parent;
        // A parent chain never exceeds the number of work items; the bound guards a malformed plan.
        while let Some(id) = parent.filter(|_| titles.len() <= current.work_items.len()) {
            let Some(ancestor) = current.work_items.get(&id) else {
                break;
            };
            titles.push(ancestor.title.clone());
            parent = ancestor.parent;
        }
        titles.reverse();
        found.entry(titles).or_default().push(work.id);
    }
    found
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
