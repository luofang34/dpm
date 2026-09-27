//! Work identity of a source task, and how the report states it.
//!
//! A task GUID is the identity. Without one, the identity is derived from the project GUID and the
//! UID, or, when the document has no project GUID either, from the target project, the explicit
//! key prefix that names the source, and the UID.

use crate::mspdi::encoding::{derived_work_id, scoped_work_id};
use crate::mspdi::report::{Finding, ItemReport};
use crate::mspdi::source::{SourceProject, SourceTask};
use dpm_model::{ProjectId, WorkItemId};
use uuid::Uuid;

/// Where a task's work identity comes from.
pub(super) enum Basis {
    TaskGuid,
    ProjectGuid,
    SourceScope,
}

pub(crate) struct Resolver<'a> {
    project: ProjectId,
    project_key: &'a str,
    prefix: &'a str,
    source_guid: Option<Uuid>,
}

impl<'a> Resolver<'a> {
    pub(crate) fn new(
        source: &SourceProject,
        project: ProjectId,
        project_key: &'a str,
        prefix: &'a str,
    ) -> Self {
        Self {
            project,
            project_key,
            prefix,
            source_guid: source.guid,
        }
    }

    fn derived(&self, task: &SourceTask) -> WorkItemId {
        match self.source_guid {
            Some(project) => derived_work_id(project, task.uid),
            None => scoped_work_id(self.project, self.prefix, task.uid),
        }
    }

    pub(super) fn resolve(&self, task: &SourceTask) -> (WorkItemId, Basis) {
        if let Some(guid) = task.guid {
            return (WorkItemId(guid), Basis::TaskGuid);
        }
        let basis = if self.source_guid.is_some() {
            Basis::ProjectGuid
        } else {
            Basis::SourceScope
        };
        (self.derived(task), basis)
    }

    /// A task GUID is preserved source data; every derived identity is an approximation.
    pub(super) fn record(&self, task: &SourceTask, basis: &Basis, report: &mut ItemReport) {
        let uid = task.uid;
        let detail = match basis {
            Basis::TaskGuid => {
                report.preserved.insert(0, "identity".into());
                return;
            }
            Basis::ProjectGuid => format!(
                "no task GUID; identity derived from the project GUID and UID {uid}, so a renumbered UID imports as new work"
            ),
            Basis::SourceScope => format!(
                "no task or project GUID; identity derived from target project {}, key prefix {} and UID {uid}: re-importing with the same prefix updates this work, a renumbered UID imports as new work, and another GUID-less source imported under the same prefix is treated as this source",
                self.project_key, self.prefix
            ),
        };
        report
            .approximated
            .insert(0, Finding::new("identity", detail));
    }
}

#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests;
