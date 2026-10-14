//! Read-only terminal projections of a validated plan snapshot.

mod console;
mod detail;
mod gantt;
mod notice;
mod now;
mod open_choices;
mod shortcuts;
mod source;
mod text_panel;
mod theme;
mod view;
mod work_label;
mod wrap;

pub use console::{
    TuiError, run_as_of_blocking, run_blocking, run_following_blocking, run_preview_blocking,
};
pub use source::{Snapshot, SnapshotSource, SourceRevision};
