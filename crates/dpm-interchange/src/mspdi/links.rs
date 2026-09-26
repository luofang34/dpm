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
use super::report::{LinkOutcome, LinkReport};
use super::source::{SourceLink, SourceProject, SourceTask};
use dpm_model::{Dependency, DependencyKind, Plan, WorkItemId, WorkKind};
use std::collections::{BTreeMap, BTreeSet};

type Edge = (WorkItemId, WorkItemId, DependencyKind);

/// Candidate dependency list and one report per source link.
pub(crate) struct MappedLinks {
    pub(crate) dependencies: Vec<Dependency>,
    pub(crate) reports: Vec<LinkReport>,
}

pub(crate) fn map(current: &Plan, source: &SourceProject, outline: &Outline) -> MappedLinks {
    let mut edges: BTreeMap<Edge, i64> = BTreeMap::new();
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
                if *lag != link.link_lag {
                    report.notes.push(format!(
                        "{} repeats a relation between the same work; the larger lag applies",
                        edge.2.abbreviation()
                    ));
                    *lag = (*lag).max(link.link_lag);
                }
            }
            produced.push(mapped);
            reports.push(report);
        }
    }
    let owned: BTreeSet<WorkItemId> = outline.mapped().map(|w| w.id).collect();
    let dependencies = merge(current, &owned, edges);
    for (report, mapped) in reports.iter_mut().zip(produced) {
        report.dependencies = mapped
            .iter()
            .filter_map(|(p, s, k)| {
                dependencies
                    .iter()
                    .find(|d| d.predecessor == *p && d.successor == *s && d.kind == *k)
                    .map(|d| d.id)
            })
            .collect();
    }
    MappedLinks {
        dependencies,
        reports,
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
    check_lag(link, report)?;
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
    match link.lag_format.and_then(time_basis) {
        Some(TimeBasis::Elapsed) => Ok(()),
        Some(TimeBasis::Working) => {
            report.outcome = LinkOutcome::Approximated;
            report.notes.push(format!(
                "working-time lag {} h treated as elapsed hours (calendar not applied)",
                lag_hours(link.link_lag)
            ));
            Ok(())
        }
        None => Err(match link.lag_format {
            Some(format) => format!(
                "lag format {format} is a percentage or unknown unit with no hour equivalent"
            ),
            None => "nonzero lag without LagFormat has no known unit".into(),
        }),
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
) -> Vec<Dependency> {
    let mut merged = Vec::new();
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
        }
    }
    merged.extend(
        edges
            .into_iter()
            .map(|((p, s, k), tenths)| Dependency::new(p, s, k, lag_hours(tenths))),
    );
    merged
}
