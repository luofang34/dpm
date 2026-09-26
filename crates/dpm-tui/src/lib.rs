//! Read-only terminal projections of a validated plan snapshot.

mod console;
mod detail;
mod gantt;
mod text_panel;
mod view;
mod work_label;

pub use console::{TuiError, run_blocking, run_preview_blocking, run_reloading_blocking};
