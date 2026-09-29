//! Query-only workspace scope for `next`.
//!
//! Scope narrows which globally ranked candidates are returned; it never changes readiness,
//! gates, scores or claims. Eligible work outside the scope stays visible in `outside_scope`
//! so a higher-ranked blocker in another project or repository cannot be hidden by a filter.

use crate::{EngineError, NextWorkCandidate, NextWorkQuery, next_work};
use dpm_model::{AssetAccess, AssetId, Key, Plan, ProjectId, WorkItem};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use thiserror::Error;

/// Shape version of [`NextWorkResult`]; incremented on any incompatible field change.
pub const NEXT_RESULT_VERSION: u32 = 1;

/// A scope key that does not name an entity in the current plan.
#[derive(Debug, Error)]
pub enum ScopeError {
    /// Project scope key is not in the plan.
    #[error("unknown project key {0}")]
    UnknownProject(String),
    /// WorkspaceAsset scope key is not in the plan.
    #[error("unknown asset key {0}")]
    UnknownResource(String),
}

/// A resolved scope entry, echoed so callers see exactly which entities were applied.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeMember<I> {
    /// Stable identity of the entity.
    pub id: I,
    /// Human-readable key the caller supplied.
    pub key: Key,
}

/// Project and asset membership filter; an empty axis does not filter.
///
/// A work item is in scope when it passes every non-empty axis:
/// - projects: its project is one of the listed projects or a descendant of one;
/// - assets: it names at least one listed asset, and every asset it writes is listed.
///   Work naming no assets never positively matches, so it is outside an asset scope.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkScope {
    /// Project subtrees to include.
    pub projects: Vec<ScopeMember<ProjectId>>,
    /// Assets the returned work must fit.
    pub assets: Vec<ScopeMember<AssetId>>,
}

impl WorkScope {
    /// Resolve human keys against the plan, rejecting unknown keys rather than matching nothing.
    pub fn resolve<'a>(
        plan: &Plan,
        project_keys: impl IntoIterator<Item = &'a str>,
        asset_keys: impl IntoIterator<Item = &'a str>,
    ) -> Result<Self, ScopeError> {
        let projects = unique(project_keys)
            .into_iter()
            .map(|key| {
                plan.projects
                    .values()
                    .find(|p| p.key.0 == key)
                    .map(|p| ScopeMember {
                        id: p.id,
                        key: p.key.clone(),
                    })
                    .ok_or_else(|| ScopeError::UnknownProject(key.into()))
            })
            .collect::<Result<_, _>>()?;
        let assets = unique(asset_keys)
            .into_iter()
            .map(|key| {
                plan.assets
                    .values()
                    .find(|r| r.key.0 == key)
                    .map(|r| ScopeMember {
                        id: r.id,
                        key: r.key.clone(),
                    })
                    .ok_or_else(|| ScopeError::UnknownResource(key.into()))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self { projects, assets })
    }

    /// Whether neither axis restricts membership.
    pub fn is_unscoped(&self) -> bool {
        self.projects.is_empty() && self.assets.is_empty()
    }

    /// Whether the work satisfies every non-empty axis of this scope.
    pub fn contains(&self, plan: &Plan, work: &WorkItem) -> bool {
        self.contains_project(plan, work.project) && self.fits_assets(work)
    }

    fn contains_project(&self, plan: &Plan, project: ProjectId) -> bool {
        if self.projects.is_empty() {
            return true;
        }
        let roots = self.projects.iter().map(|m| m.id).collect::<BTreeSet<_>>();
        let mut seen = BTreeSet::new();
        let mut next = Some(project);
        while let Some(id) = next {
            if roots.contains(&id) {
                return true;
            }
            if !seen.insert(id) {
                return false;
            }
            next = plan.projects.get(&id).and_then(|p| p.parent);
        }
        false
    }

    fn fits_assets(&self, work: &WorkItem) -> bool {
        if self.assets.is_empty() {
            return true;
        }
        let allowed = self.assets.iter().map(|m| m.id).collect::<BTreeSet<_>>();
        work.contract
            .assets
            .iter()
            .any(|need| allowed.contains(&need.asset))
            && work
                .contract
                .assets
                .iter()
                .filter(|need| need.access == AssetAccess::Write)
                .all(|need| allowed.contains(&need.asset))
    }
}

fn unique<'a>(keys: impl IntoIterator<Item = &'a str>) -> BTreeSet<&'a str> {
    keys.into_iter().collect()
}

/// A candidate within scope, carrying its position in the unfiltered ranking.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScopedCandidate {
    /// One-based rank among all eligible work in the workspace, before scope filtering.
    pub global_rank: usize,
    /// Ranking computed on the full graph.
    #[serde(flatten)]
    pub candidate: NextWorkCandidate,
}

/// Eligible work that the scope excluded, kept visible rather than hidden.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutsideScope {
    /// Number of eligible work items outside the scope.
    pub count: usize,
    /// Eligible outside work ranked above the best in-scope work, or all of it when none is in scope.
    pub higher_ranked_count: usize,
    /// Keys of eligible outside work in global rank order; the first `higher_ranked_count` rank higher.
    pub keys: Vec<Key>,
}

/// Versioned `next` result: global ranking, then scope, then limit.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NextWorkResult {
    /// Shape version; see [`NEXT_RESULT_VERSION`].
    pub result_version: u32,
    /// Resolved scope that selected `candidates`.
    pub scope: WorkScope,
    /// Capabilities used for eligibility; distinct from scope membership.
    pub capabilities: BTreeSet<String>,
    /// Maximum number of returned candidates, applied after scope filtering.
    pub limit: usize,
    /// Ready, capability-eligible work across the whole workspace.
    pub eligible_count: usize,
    /// Eligible work inside the scope before the limit.
    pub in_scope_count: usize,
    /// In-scope candidates in global rank order, truncated to `limit`.
    pub candidates: Vec<ScopedCandidate>,
    /// Eligible work excluded by scope.
    pub outside_scope: OutsideScope,
}

/// Rank eligible work on the full graph at an adapter-supplied time, then partition it by scope and
/// apply the limit.
pub fn next_in_scope(
    plan: &Plan,
    query: &NextWorkQuery,
    scope: &WorkScope,
    limit: usize,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<NextWorkResult, EngineError> {
    let ranked = next_work(plan, query, now)?;
    let eligible_count = ranked.len();
    let mut inside = Vec::new();
    let mut outside = OutsideScope::default();
    let mut best_inside_rank = None;
    for (index, candidate) in ranked.into_iter().enumerate() {
        let global_rank = index.wrapping_add(1);
        if scope.contains(plan, &candidate.work) {
            best_inside_rank.get_or_insert(global_rank);
            inside.push(ScopedCandidate {
                global_rank,
                candidate,
            });
        } else {
            if best_inside_rank.is_none() {
                outside.higher_ranked_count = outside.higher_ranked_count.wrapping_add(1);
            }
            outside.keys.push(candidate.work.key);
        }
    }
    outside.count = outside.keys.len();
    let in_scope_count = inside.len();
    inside.truncate(limit);
    Ok(NextWorkResult {
        result_version: NEXT_RESULT_VERSION,
        scope: scope.clone(),
        capabilities: query.capabilities.clone(),
        limit,
        eligible_count,
        in_scope_count,
        candidates: inside,
        outside_scope: outside,
    })
}

#[cfg(test)]
mod tests;
