//! Read-only terminal projections of a validated plan snapshot.

mod console;
mod detail;
mod gantt;
mod notice;
mod now;
mod open_choices;
mod source;
mod text_panel;
mod view;
mod work_label;
mod wrap;

pub use console::{TuiError, run_blocking, run_following_blocking, run_preview_blocking};
pub use source::{Snapshot, SnapshotSource, SourceRevision};
