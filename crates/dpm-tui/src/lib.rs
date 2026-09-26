//! Read-only terminal projections of a validated plan snapshot.

mod console;
mod gantt;
mod text_panel;
mod view;
mod work_label;

pub use console::{TuiError, run_blocking};
