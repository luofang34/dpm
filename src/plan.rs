//! The `plan` subcommands: schema and template, reviewed proposals and their apply, reverts of
//! recorded changes, and Microsoft Project interchange.

use crate::app::{actor, query_blocking};
use crate::args::{PlanCommand, Preconditions};
use crate::bootstrap::read_candidate_blocking;
use crate::{error::CliError, output};
use dpm_app::{Application, CommandRequest, PlanChangeRequest, Query};

pub(crate) fn plan_command_blocking(
    app: &mut Application,
    command: PlanCommand,
    json: bool,
    preconditions: Preconditions,
) -> Result<(), CliError> {
    match command {
        PlanCommand::Template => query_blocking(app, Query::PlanTemplate, json),
        PlanCommand::Schema => query_blocking(app, Query::PlanSchema, json),
        PlanCommand::Diff { file } => query_blocking(
            app,
            Query::ProposeChange {
                plan: Box::new(read_candidate_blocking(&file)?),
            },
            json,
        ),
        PlanCommand::Apply {
            file,
            reason,
            actor: who,
        } => {
            app.ensure_writable()?;
            let plan = read_candidate_blocking(&file)?;
            let operation = app.apply_plan_change_blocking(PlanChangeRequest {
                actor: actor(&who)?,
                base_revision: preconditions.base_revision.unwrap_or(plan.revision),
                base_lineage: preconditions.base_lineage,
                operation_id: preconditions.operation_id,
                plan: Box::new(plan),
                reason,
            })?;
            output::operation_blocking(operation, json)
        }
        PlanCommand::RevertPreview { operation } => {
            query_blocking(app, Query::ProposeRevert { operation }, json)
        }
        PlanCommand::Revert {
            operation,
            reason,
            actor: who,
        } => {
            // A revert applies only at the revision whose review the caller saw.
            let base_revision = preconditions.base_revision.ok_or_else(|| {
                CliError::Input(
                    "plan revert applies only at the revision its preview showed: pass --base-revision"
                        .into(),
                )
            })?;
            let command = app.build_command_blocking(preconditions.operation_id, |app| {
                app.revert_command_blocking(operation, reason)
            })?;
            let recorded = app.execute_blocking(CommandRequest {
                actor: actor(&who)?,
                base_revision,
                base_lineage: preconditions.base_lineage,
                operation_id: preconditions.operation_id,
                command,
            })?;
            output::operation_blocking(recorded, json)
        }
        interchange @ (PlanCommand::ImportMspdi { .. } | PlanCommand::ExportMspdi { .. }) => {
            crate::interchange::plan_command_blocking(app, interchange, json)
        }
    }
}
