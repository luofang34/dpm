//! Bounded interchange with external project-scheduling files.
//!
//! The supported format is the documented Microsoft Project XML (MSPDI) subset. Imports never write
//! state: they build a candidate plan for the reviewed plan-change path together with a per-item
//! report of preserved values, approximations and rejected source data. Exports write the same
//! subset deterministically.

mod error;
mod mspdi;

pub use error::InterchangeError;
pub use mspdi::{
    DependencyExportReport, ExportReport, ExportResult, Finding, ImportOptions, ImportReport,
    ImportResult, ItemExportReport, ItemOutcome, ItemReport, LinkOutcome, LinkReport,
    SourceSummary, WorkReference, export_mspdi, import_mspdi,
};
