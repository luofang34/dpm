//! Map predecessor links onto dependencies between task and milestone anchors.
//!
//! A summary task starts at its earliest child start and finishes at its latest child finish.
//! A lower bound on that latest finish (a summary predecessor of FS or FF) and a lower bound on
//! that earliest start (a summary successor of FS or SS) hold exactly when they hold for every
//! descendant, so those links expand to the summary's task and milestone descendants. The other
//! combinations bound a minimum from below or a maximum from above and have no exact expansion;
//! they are rejected and reported.

use super::encoding::{TimeBasis, lag_hours, lag_tenths, relation_from_code, time_basis};
use super::items::Outline;
use super::report::{DependencyChange, LinkOutcome, LinkReport, RemovedDependency};
use super::source::{SourceLink, SourceProject, SourceTask};
use dpm_model::{Dependency, DependencyKind, Plan, WorkItemId, WorkKind};
use std::collections::{BTreeMap, BTreeSet};

type Edge = (WorkItemId, WorkItemId, DependencyKind);

/// Candidate dependency list, one report per source link, and the local edges the source drops.
pub(crate) struct MappedLinks {
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) reports: Vec<LinkReport>,
    pub(crate) removed: Vec<RemovedDependency>,
}

pub(crate) fn map(current: &Plan, source: &SourceProject, outline: &Outline) -> MappedLinks {
    let mut edges: BTreeMap<Edge, i64> = BTreeMap::new();
    let mut sources: BTreeMap<Edge, usize> = BTreeMap::new();
    let mut reports = Vec::new();
    let mut produced: Vec<Vec<Edge>> = Vec::new();
    for task in &source.tasks {
        for link in &task.links {
            let mut report = LinkReport {
                predecessor_uid: link.predecessor_uid,
                successor_uid: task.uid,
                relation: link.type_code.map(|code| {
                    relation_from_code(code).map_or(code.to_string(), |k| k.abbreviation().into())
                }),
                link_lag: link.link_lag,
                lag_format: link.lag_format,
                outcome: LinkOutcome::Preserved,
                notes: Vec::new(),
                changes: Vec::new(),
                dependencies: Vec::new(),
            };
            let mapped = match anchors(outline, task, link, &mut report) {
                Ok(mapped) => mapped,
                Err(reason) => {
                    report.outcome = LinkOutcome::Rejected;
                    report.notes.push(reason);
                    Vec::new()
                }
            };
            for edge in &mapped {
                let lag = edges.entry(*edge).or_insert(link.link_lag);
                *lag = (*lag).max(link.link_lag);
                *sources.entry(*edge).or_default() += 1;
            }
            produced.push(mapped);
            reports.push(report);
        }
    }
    let owned: BTreeSet<WorkItemId> = outline.mapped().map(|w| w.id).collect();
    let (dependencies, removed) = merge(current, &owned, edges);
    let keys: BTreeMap<WorkItemId, &str> = current
        .work_items
        .values()
        .chain(outline.mapped())
        .map(|w| (w.id, w.key.0.as_str()))
        .collect();
    for (report, mapped) in reports.iter_mut().zip(produced) {
        for edge in mapped {
            let Some(dependency) = dependencies
                .iter()
                .find(|d| (d.predecessor, d.successor, d.kind) == edge)
            else {
                continue;
            };
            let shared = sources.get(&edge).is_some_and(|n| *n > 1);
            describe(current, &keys, report, dependency, shared);
            report.dependencies.push(dependency.id);
        }
    }
    MappedLinks {
        dependencies,
        reports,
        removed,
    }
}

/// State what the candidate dependency carries when it differs from this link or from the local
/// edge it updates.
fn describe(
    current: &Plan,
    keys: &BTreeMap<WorkItemId, &str>,
    report: &mut LinkReport,
    dependency: &Dependency,
    shared: bool,
) {
    let key = |id: WorkItemId| {
        keys.get(&id)
            .map_or_else(|| id.to_string(), |k| (*k).to_owned())
    };
    let edge = format!(
        "{} {} -> {}",
        dependency.kind.abbreviation(),
        key(dependency.predecessor),
        key(dependency.successor)
    );
    if lag_tenths(dependency.lag_hours) != report.link_lag {
        report.outcome = LinkOutcome::Approximated;
        report.notes.push(format!(
            "{edge} merges with another link on the same relation; the dependency carries the larger lag {} h",
            dependency.lag_hours
        ));
    } else if shared {
        report.notes.push(format!(
            "{edge} repeats another link on the same relation; one dependency carries both"
        ));
    }
    // Merge matches local edges by endpoints and kind and keeps every other field, so the lag is
    // the only field an import can change on an existing dependency.
    if let Some(local) = current.find_dependency(dependency.id)
        && local.lag_hours != dependency.lag_hours
    {
        report.notes.push(format!(
            "{edge} changes the local lag {} h to {} h",
            local.lag_hours, dependency.lag_hours
        ));
        report.changes.push(DependencyChange {
            dependency: dependency.id,
            relation: edge,
            field: "lag".into(),
            before: format!("{} h", local.lag_hours),
            after: format!("{} h", dependency.lag_hours),
        });
        if report.outcome == LinkOutcome::Preserved {
            report.outcome = LinkOutcome::Changed;
        }
    }
}

/// Validate one link and return the task/milestone pairs it constrains.
fn anchors(
    outline: &Outline,
    task: &SourceTask,
    link: &SourceLink,
    report: &mut LinkReport,
) -> Result<Vec<Edge>, String> {
    let kind = match link.type_code {
        None => return Err("link has no relation type".into()),
        Some(code) => relation_from_code(code)
            .ok_or_else(|| format!("relation type {code} is not FF, FS, SF or SS"))?,
    };
    if link.cross_project {
        return Err("cross-project links are not imported".into());
    }
    let predecessor_uid = link
        .predecessor_uid
        .ok_or_else(|| "link has no PredecessorUID".to_string())?;
    let successor = outline.work(task.uid)?;
    let predecessor = outline.work(predecessor_uid)?;
    let starts_bound = matches!(
        kind,
        DependencyKind::FinishStart | DependencyKind::StartStart
    );
    let finish_based = matches!(
        kind,
        DependencyKind::FinishStart | DependencyKind::FinishFinish
    );
    let from = endpoint(
        outline,
        predecessor.id,
        predecessor.kind,
        finish_based,
        "predecessor",
        kind,
    )?;
    let to = endpoint(
        outline,
        successor.id,
        successor.kind,
        starts_bound,
        "successor",
        kind,
    )?;
    if from.iter().any(|id| to.contains(id)) {
        return Err(
            "link joins a task with itself or a summary task with its own descendant".into(),
        );
    }
    check_lag(link, report)?;
    let expanded = from.len() > 1
        || to.len() > 1
        || predecessor.kind == WorkKind::WorkPackage
        || successor.kind == WorkKind::WorkPackage;
    if expanded {
        report.notes.push(format!(
            "summary link expanded to {} predecessor and {} successor task/milestone anchor(s)",
            from.len(),
            to.len()
        ));
    }
    Ok(from
        .iter()
        .flat_map(|p| to.iter().map(move |s| (*p, *s, kind)))
        .collect())
}

fn check_lag(link: &SourceLink, report: &mut LinkReport) -> Result<(), String> {
    if link.link_lag == 0 {
        return Ok(());
    }
    // `LinkLag` always counts tenths of a minute; `LagFormat` only selects the display unit and
    // elapsed versus working time, so an absent format is Microsoft Project's default, working
    // time. OmniPlan writes every lag that way.
    let (basis, origin) = match link.lag_format {
        None => (Some(TimeBasis::Working), "no LagFormat, so "),
        Some(format) => (time_basis(format), ""),
    };
    match basis {
        Some(TimeBasis::Elapsed) => Ok(()),
        Some(TimeBasis::Working) => {
            report.outcome = LinkOutcome::Approximated;
            report.notes.push(format!(
                "{origin}working-time lag {} h treated as elapsed hours (calendar not applied)",
                lag_hours(link.link_lag)
            ));
            Ok(())
        }
        None => Err(format!(
            "lag format {} is a percentage or unknown unit with no hour equivalent",
            link.lag_format.unwrap_or_default()
        )),
    }
}

fn endpoint(
    outline: &Outline,
    id: WorkItemId,
    kind: WorkKind,
    exact_for_summary: bool,
    role: &str,
    relation: DependencyKind,
) -> Result<Vec<WorkItemId>, String> {
    if kind != WorkKind::WorkPackage {
        return Ok(vec![id]);
    }
    if !exact_for_summary {
        return Err(format!(
            "a summary {role} of an {} link has no exact task or milestone anchors",
            relation.abbreviation()
        ));
    }
    let leaves = outline.leaves(id);
    if leaves.is_empty() {
        return Err(format!("summary {role} has no imported task or milestone"));
    }
    Ok(leaves)
}

/// Replace the dependencies among imported work with the source relations, keeping every other
/// edge and each surviving edge's identity, policy, rationale and waiver.
fn merge(
    current: &Plan,
    owned: &BTreeSet<WorkItemId>,
    mut edges: BTreeMap<Edge, i64>,
) -> (Vec<Dependency>, Vec<RemovedDependency>) {
    let mut merged = Vec::new();
    let mut removed = Vec::new();
    for edge in &current.dependencies {
        if !(owned.contains(&edge.predecessor) && owned.contains(&edge.successor)) {
            merged.push(edge.clone());
            continue;
        }
        if let Some(tenths) = edges.remove(&(edge.predecessor, edge.successor, edge.kind)) {
            let mut kept = edge.clone();
            if lag_tenths(edge.lag_hours) != tenths {
                kept.lag_hours = lag_hours(tenths);
            }
            merged.push(kept);
        } else {
            let key = |id| current.work_items.get(&id).map(|w| w.key.clone());
            if let (Some(predecessor), Some(successor)) =
                (key(edge.predecessor), key(edge.successor))
            {
                removed.push(RemovedDependency {
                    id: edge.id,
                    predecessor,
                    successor,
                    kind: edge.kind,
                    lag_hours: edge.lag_hours,
                });
            }
        }
    }
    merged.extend(
        edges
            .into_iter()
            .map(|((p, s, k), tenths)| Dependency::new(p, s, k, lag_hours(tenths))),
    );
    (merged, removed)
}

#[cfg(test)]
mod tests;
