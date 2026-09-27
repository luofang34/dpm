//! Work identity of a source task, and how the report states it.
//!
//! DPM metadata supplies identity when present; otherwise a task GUID is the identity. Without one, the identity is derived from the project GUID and the
//! UID, or, when the document has no project GUID either, from the target project, the explicit
//! key prefix that names the source, and the UID. Only when the import opts in does a task without
//! a GUID match existing work by title path instead.

use super::matching::Matcher;
use crate::mspdi::encoding::{derived_work_id, scoped_work_id};
use crate::mspdi::report::{Finding, ItemReport};
use crate::mspdi::source::{SourceProject, SourceTask};
use crate::{ExistingMatch, InterchangeError};
use dpm_model::{Plan, ProjectId, WorkItemId};
use uuid::Uuid;

/// Where a task's work identity comes from.
pub(super) enum Basis {
    TaskGuid,
    Metadata,
    ProjectGuid,
    SourceScope,
    /// Existing work whose title path equals the task's outline path.
    Matched,
}

pub(crate) struct Resolver<'a> {
    current: &'a Plan,
    project: ProjectId,
    project_key: &'a str,
    prefix: &'a str,
    source_guid: Option<Uuid>,
    matcher: Option<Matcher>,
}

impl<'a> Resolver<'a> {
    /// Refuses the import when an opted-in match is ambiguous anywhere in the document, so no task
    /// silently merges into the wrong work.
    pub(crate) fn new(
        current: &'a Plan,
        source: &SourceProject,
        (project, project_key): (ProjectId, &'a str),
        prefix: &'a str,
        mode: Option<ExistingMatch>,
    ) -> Result<Self, InterchangeError> {
        let mut resolver = Self {
            current,
            project,
            project_key,
            prefix,
            source_guid: source.guid,
            matcher: None,
        };
        let Some(ExistingMatch::TitlePath) = mode else {
            return Ok(resolver);
        };
        let (matcher, mut ambiguities) = Matcher::new(current, source, project);
        for task in source
            .tasks
            .iter()
            .filter(|t| t.guid.is_none() && t.metadata.is_none())
        {
            let derived = resolver.derived(task);
            if let Some(id) = matcher.find(task.uid)
                && id != derived
                && let Some(work) = current.work_items.get(&derived)
            {
                let other = current
                    .work_items
                    .get(&id)
                    .map_or("?", |w| w.key.0.as_str());
                ambiguities.push(format!(
                    "task UID {} already maps to {} through its derived identity, but its title path {:?} matches {other}",
                    task.uid,
                    work.key,
                    matcher.path(task.uid).unwrap_or_default()
                ));
            }
        }
        if !ambiguities.is_empty() {
            return Err(InterchangeError::AmbiguousMatch { ambiguities });
        }
        resolver.matcher = Some(matcher);
        Ok(resolver)
    }

    fn derived(&self, task: &SourceTask) -> WorkItemId {
        match self.source_guid {
            Some(project) => derived_work_id(project, task.uid),
            None => scoped_work_id(self.project, self.prefix, task.uid),
        }
    }

    /// A derived identity that already names local work wins over a match: that work came from
    /// this very source, and the constructor refused any case where the two disagree.
    pub(super) fn resolve(&self, task: &SourceTask) -> (WorkItemId, Basis) {
        if let Some(metadata) = &task.metadata {
            return (metadata.id, Basis::Metadata);
        }
        if let Some(guid) = task.guid {
            return (WorkItemId(guid), Basis::TaskGuid);
        }
        let derived = self.derived(task);
        if let Some(id) = self.matcher.as_ref().and_then(|m| m.find(task.uid))
            && !self.current.work_items.contains_key(&derived)
        {
            return (id, Basis::Matched);
        }
        let basis = if self.source_guid.is_some() {
            Basis::ProjectGuid
        } else {
            Basis::SourceScope
        };
        (derived, basis)
    }

    /// A task GUID is preserved source data; every derived or matched identity is an
    /// approximation.
    pub(super) fn record(
        &self,
        task: &SourceTask,
        (id, basis): (WorkItemId, &Basis),
        report: &mut ItemReport,
    ) {
        let uid = task.uid;
        let path = self.matcher.as_ref().and_then(|m| m.path(uid));
        let mut detail = match basis {
            Basis::Metadata => {
                report.preserved.insert(0, "dpm_metadata".into());
                report.preserved.insert(0, "identity".into());
                return;
            }
            Basis::TaskGuid => {
                report.preserved.insert(0, "identity".into());
                return;
            }
            Basis::Matched => format!(
                "no task GUID; matched existing work {} by title path {:?}, as the import requested",
                self.current
                    .work_items
                    .get(&id)
                    .map_or("?", |w| w.key.0.as_str()),
                path.clone().unwrap_or_default()
            ),
            Basis::ProjectGuid => format!(
                "no task GUID; identity derived from the project GUID and UID {uid}, so a renumbered UID imports as new work"
            ),
            Basis::SourceScope => format!(
                "no task or project GUID; identity derived from target project {}, key prefix {} and UID {uid}: re-importing with the same prefix updates this work, a renumbered UID imports as new work, and another GUID-less source imported under the same prefix is treated as this source",
                self.project_key, self.prefix
            ),
        };
        let matched = self.matcher.as_ref().and_then(|m| m.find(uid)).is_some();
        if let (Some(path), false) = (path, matched) {
            detail.push_str(&format!("; no existing work has the title path {path:?}"));
        }
        report
            .approximated
            .insert(0, Finding::new("identity", detail));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
