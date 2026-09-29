//! Which commands a pinned `--clock` applies to.

use crate::{
    args::{Commands, PlanCommand, StoreCommand},
    error::CliError,
};
use chrono::{DateTime, Utc};
use dpm_app::QueryClock;

/// The query clock a command runs with; a pinned reading is refused by anything but a workspace
/// query, because a mutation records the time it commits, and plan validation and the schema read
/// neither a workspace nor a clock.
pub(crate) fn for_command(
    command: &Commands,
    clock: Option<DateTime<Utc>>,
) -> Result<QueryClock, CliError> {
    let Some(at) = clock else {
        return Ok(QueryClock::System);
    };
    if is_query(command) {
        Ok(QueryClock::Fixed(at))
    } else {
        Err(CliError::Input(
            "--clock applies only to queries; a mutation records the time it commits".into(),
        ))
    }
}

fn is_query(command: &Commands) -> bool {
    matches!(
        command,
        Commands::Status { .. }
            | Commands::Next { .. }
            | Commands::Show { .. }
            | Commands::Explain { .. }
            | Commands::Export
            | Commands::Store(StoreCommand::History { .. } | StoreCommand::Revision)
            | Commands::Plan {
                command: PlanCommand::Template
                    | PlanCommand::Diff { .. }
                    | PlanCommand::ImportMspdi { .. }
                    | PlanCommand::ExportMspdi { .. },
            }
    )
}

#[cfg(test)]
mod tests;
