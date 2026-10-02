//! Whether the source a locator now selects is still the one this connection opened.
//!
//! A project locator may be repointed while a long-lived connection stays on what it opened, so
//! every read and every write is checked against the locator as it stands. The comparison is by
//! identity, never by revision: the file the locator names, the kind of source, the workspace a
//! preview file holds, and the lineage a store continues. Two previews both have no lineage, so
//! lineage alone cannot tell them apart.
//!
//! The check runs before a mutation, not inside its transaction: the connection only ever writes
//! the store it opened, and the commit's own revision and lineage preconditions still apply to
//! that store.

use super::{
    error::NativeError,
    protocol::{SourceChange, SourceIdentity, SourceKind},
};
use crate::{AppError, Application, ProjectError, ProjectLocation, ProjectSource};
use dpm_model::WorkspaceId;
use serde::Deserialize;
use std::{fs, path::Path};

impl Application {
    /// Refuse the call if the locator no longer selects the source this connection opened.
    ///
    /// A connection opened without a locator, such as on an explicit database path, is its own
    /// source and always passes.
    pub(super) fn check_source_blocking(&self) -> Result<(), NativeError> {
        let (Some(root), Some(opened)) = (&self.project_root, &self.opened_location) else {
            return Ok(());
        };
        let location = ProjectLocation::at_blocking(root).map_err(AppError::from)?;
        let attached = self.attached_source_blocking()?;
        let current = selected_source_blocking(&location)?;
        if let Some(reason) = difference(location.source == opened.source, &attached, &current) {
            return Err(NativeError::SourceChanged {
                reason,
                attached,
                current,
            });
        }
        // The locator's own declarations are validated as a fresh open validates them, with the
        // same errors, so an edit that a new open would refuse cannot be written through an open
        // connection. The workspace is the connection's own, so this reads no plan.
        location
            .expect_workspace(self.workspace_identity_blocking()?)
            .map_err(AppError::from)?;
        if location.asset != opened.asset {
            // An unknown asset is refused as a fresh open refuses it; a known but different one is
            // another context than this connection was opened with.
            location
                .resolve_asset(&self.plan_blocking()?)
                .map_err(AppError::from)?;
            return Err(NativeError::SourceChanged {
                reason: SourceChange::Asset,
                attached,
                current,
            });
        }
        Ok(())
    }

    /// The identity of the source this connection reads.
    fn attached_source_blocking(&self) -> Result<SourceIdentity, AppError> {
        let state = self.project_state_blocking()?;
        Ok(SourceIdentity {
            kind: state.source,
            workspace_id: Some(state.watermark.workspace_id),
            lineage_id: state.watermark.lineage_id,
        })
    }
}

/// How what the locator selects differs from what the connection holds, if it does.
///
/// The file named is compared first, because two previews both lack a lineage and so are told
/// apart only by where they are and what workspace they hold. A store is identified by its
/// lineage, so it leaves `workspace_id` absent and is not compared on it.
pub(super) fn difference(
    same_locator: bool,
    attached: &SourceIdentity,
    current: &SourceIdentity,
) -> Option<SourceChange> {
    if !same_locator {
        Some(SourceChange::Locator)
    } else if attached.kind != current.kind {
        Some(SourceChange::Kind)
    } else if current
        .workspace_id
        .is_some_and(|found| Some(found) != attached.workspace_id)
    {
        Some(SourceChange::Workspace)
    } else if attached.lineage_id != current.lineage_id {
        Some(SourceChange::Lineage)
    } else {
        None
    }
}

/// The identity of what the locator selects now, reading as little of it as identity needs.
fn selected_source_blocking(location: &ProjectLocation) -> Result<SourceIdentity, AppError> {
    if let ProjectSource::Preview(path) = &location.source {
        return Ok(SourceIdentity {
            kind: SourceKind::Preview,
            workspace_id: Some(preview_workspace_blocking(path)?),
            lineage_id: None,
        });
    }
    #[cfg(feature = "sqlite")]
    {
        let path = location
            .store_path_blocking()?
            .ok_or(AppError::NotInitialized)?;
        let found = dpm_store::store_revision_blocking(&path)?.ok_or(AppError::NotInitialized)?;
        Ok(SourceIdentity {
            kind: if found.lineage.archived {
                SourceKind::Archive
            } else {
                SourceKind::Live
            },
            workspace_id: None,
            lineage_id: Some(found.lineage.lineage_id),
        })
    }
    #[cfg(not(feature = "sqlite"))]
    Err(AppError::Unsupported { feature: "sqlite" })
}

/// The workspace a preview plan file holds, read without validating the rest of the plan.
fn preview_workspace_blocking(path: &Path) -> Result<WorkspaceId, AppError> {
    #[derive(Deserialize)]
    struct Head {
        workspace: Workspace,
    }
    #[derive(Deserialize)]
    struct Workspace {
        id: WorkspaceId,
    }
    let text = fs::read_to_string(path).map_err(|source| ProjectError::Io {
        action: "read preview plan",
        path: path.to_path_buf(),
        source,
    })?;
    Ok(serde_json::from_str::<Head>(&text)?.workspace.id)
}
