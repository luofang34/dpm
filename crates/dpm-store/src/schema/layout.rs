//! The exact `sqlite_master` contents each accepted layout consists of.
//!
//! Every schema object is compared, not only tables: a trigger or view planted in a copied store
//! would otherwise run inside DPM's own write transactions.

use super::Layout;
use crate::{StoreError, error::database_error};
use rusqlite::Connection;
use std::path::Path;

pub(crate) const PLAN_STATE: &str = "CREATE TABLE plan_state (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 revision INTEGER NOT NULL,
                 plan_json TEXT NOT NULL
             )";
pub(crate) const OPERATIONS: &str = "CREATE TABLE operations (
                 sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                 operation_id TEXT NOT NULL UNIQUE,
                 base_revision INTEGER NOT NULL,
                 resulting_revision INTEGER NOT NULL,
                 actor_json TEXT NOT NULL,
                 timestamp TEXT NOT NULL,
                 command_json TEXT NOT NULL
             )";
pub(crate) const HISTORY_ORIGIN: &str = "CREATE TABLE history_origin (
                 singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                 revision INTEGER NOT NULL
             )";
/// SQLite creates this table itself for the `AUTOINCREMENT` column of `operations`.
const SEQUENCE: &str = "CREATE TABLE sqlite_sequence(name,seq)";

/// One `sqlite_master` row with whitespace-normalized SQL.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct SchemaObject {
    kind: String,
    name: String,
    table: String,
    sql: Option<String>,
}

impl SchemaObject {
    fn new(kind: &str, name: &str, table: &str, sql: Option<&str>) -> Self {
        Self {
            kind: kind.into(),
            name: name.into(),
            table: table.into(),
            sql: sql.map(normalize),
        }
    }

    fn describe(&self) -> String {
        format!("{} {} on {}", self.kind, self.name, self.table)
    }
}

/// Whitespace is the only difference SQLite introduces between DPM's DDL and the stored text.
fn normalize(sql: &str) -> String {
    sql.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The schema objects a layout consists of, sorted like [`objects_blocking`] returns them.
pub(crate) fn expected(layout: Layout) -> Vec<SchemaObject> {
    let table = |name: &str, sql| SchemaObject::new("table", name, name, Some(sql));
    let mut objects = vec![
        SchemaObject::new("index", "sqlite_autoindex_operations_1", "operations", None),
        table("operations", OPERATIONS),
        table("plan_state", PLAN_STATE),
        table("sqlite_sequence", SEQUENCE),
    ];
    match layout {
        Layout::Empty => objects.clear(),
        Layout::Originless => {}
        Layout::Current => objects.push(table("history_origin", HISTORY_ORIGIN)),
    }
    objects.sort();
    objects
}

pub(crate) fn objects_blocking(
    connection: &Connection,
    path: &Path,
) -> Result<Vec<SchemaObject>, StoreError> {
    let mut statement = connection
        .prepare("SELECT type, name, tbl_name, sql FROM sqlite_master")
        .map_err(database_error(path, "prepare schema query"))?;
    let mut objects = statement
        .query_map([], |row| {
            Ok(SchemaObject {
                kind: row.get(0)?,
                name: row.get(1)?,
                table: row.get(2)?,
                sql: row.get::<_, Option<String>>(3)?.as_deref().map(normalize),
            })
        })
        .map_err(database_error(path, "inspect schema"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error(path, "read schema"))?;
    objects.sort();
    Ok(objects)
}

/// Report every missing, unexpected or altered object, so an operator sees the whole difference.
pub(crate) fn compare(
    path: &Path,
    version: i64,
    found: &[SchemaObject],
    expected: Vec<SchemaObject>,
) -> Result<(), StoreError> {
    if found == expected {
        return Ok(());
    }
    let same_name = |a: &SchemaObject, b: &SchemaObject| a.kind == b.kind && a.name == b.name;
    let mut differences = Vec::new();
    for object in found {
        match expected.iter().find(|wanted| same_name(wanted, object)) {
            None => differences.push(format!("unexpected {}", object.describe())),
            Some(wanted) if wanted != object => {
                differences.push(format!("altered {}", object.describe()));
            }
            Some(_) => {}
        }
    }
    for wanted in &expected {
        if !found.iter().any(|object| same_name(wanted, object)) {
            differences.push(format!("missing {}", wanted.describe()));
        }
    }
    Err(StoreError::UnrecognizedSchema {
        path: path.to_path_buf(),
        detail: format!(
            "schema version {version} layout differs: {}",
            differences.join(", ")
        ),
    })
}
