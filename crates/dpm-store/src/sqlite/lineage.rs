//! Which writable history a store file continues.
//!
//! A lineage is minted whenever a file is created for writing: `init` and `import` start one, and
//! `restore` starts a new one in its copy while the source keeps its own. A backup keeps its
//! source's lineage but is archived, so it is read and verified, never written; only a restore
//! makes a writable store from it. Two writable files therefore never share a lineage, and a client
//! that remembers the lineage next to a revision notices when a path now names another history.

use super::snapshot::{column, decode};
use crate::{LineageError, StoreError, StoreLineage, StoredRecord, error::database_error};
use dpm_model::LineageId;
use rusqlite::{Connection, params};
use std::path::Path;

/// What a copied file becomes before it is closed.
#[derive(Debug, Clone, Copy)]
pub(super) enum Seal {
    /// A backup: the source's lineage, archived.
    Archive,
    /// A restored store: a new lineage, writable.
    Fork(LineageId),
}

pub(super) fn read_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<StoreLineage, StoreError> {
    let mut statement = connection
        .prepare("SELECT lineage_id, archived FROM store_lineage WHERE singleton = 1")
        .map_err(database_error(path, "prepare lineage query"))?;
    let mut rows = statement
        .query([])
        .map_err(database_error(path, "load lineage"))?;
    let Some(row) = rows.next().map_err(database_error(path, "load lineage"))? else {
        return Err(StoreError::Lineage(LineageError::Missing {
            path: path.to_path_buf(),
        }));
    };
    let text: String = column(row, 0, path, StoredRecord::Lineage, "lineage_id")?;
    let archived: bool = column(row, 1, path, StoredRecord::Lineage, "archived")?;
    Ok(StoreLineage {
        lineage_id: decode(
            &serde_json::Value::String(text).to_string(),
            path,
            StoredRecord::Lineage,
            "lineage_id",
        )?,
        archived,
    })
}

/// Start a new writable lineage in a store being initialized.
pub(super) fn write_initial_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<(), StoreError> {
    connection
        .execute(
            "INSERT INTO store_lineage(singleton, lineage_id, archived) VALUES(1, ?1, 0)",
            params![LineageId::new().to_string()],
        )
        .map_err(database_error(path, "record lineage"))?;
    Ok(())
}

/// Mark a freshly copied file as an archive or as the start of a new lineage.
pub(super) fn seal_blocking(
    connection: &Connection,
    path: &Path,
    seal: Seal,
) -> Result<(), StoreError> {
    let changed = match seal {
        Seal::Archive => connection.execute("UPDATE store_lineage SET archived = 1", []),
        Seal::Fork(lineage) => connection.execute(
            "UPDATE store_lineage SET lineage_id = ?1, archived = 0",
            params![lineage.to_string()],
        ),
    }
    .map_err(database_error(path, "seal copied lineage"))?;
    if changed == 1 {
        Ok(())
    } else {
        Err(StoreError::Lineage(LineageError::Missing {
            path: path.to_path_buf(),
        }))
    }
}
