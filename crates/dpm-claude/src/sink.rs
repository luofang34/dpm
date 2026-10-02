//! Where run records go: the durable side of the intake, through the shared application.
//!
//! Records are written through `dpm-app` run commands only, as a service actor, to the run sidecar
//! store: a run write never takes the project writer lock, never changes the plan revision and
//! never touches task state. Activity is written with the intake's source sequences, so a retry of
//! a batch after a failure is idempotent; the lifecycle's terminal transition has an identity
//! derived from the run, so repeating it after a restart is a replay and never a second fact.
//! A run's provider session, turn and provenance are recorded once, when it starts, and are
//! immutable from then on.

use dpm_app::{AppError, Application, RunActivityRequest, RunReportRequest, RunStartRequest};
use dpm_model::{
    ActivityInput, ActivityKind, ActorId, LineageId, Observation, RunEventId, RunId, RunProvenance,
    RunSession, RunSource, RunState, RunView,
};
use std::error::Error;
use thiserror::Error;
use uuid::Uuid;

type Source = Box<dyn Error + Send + Sync>;

/// Why a write did not happen.
#[derive(Debug, Error)]
pub enum SinkError {
    /// The store was busy past its timeout; nothing was written and the same write may be retried.
    #[error("{context}: the run store was busy")]
    Busy {
        /// What was being written.
        context: &'static str,
        /// The underlying failure.
        #[source]
        source: Source,
    },
    /// The write was refused or failed; retrying will not help.
    #[error("{context}: not recorded")]
    Refused {
        /// What was being written.
        context: &'static str,
        /// The underlying failure.
        #[source]
        source: Source,
    },
}

impl SinkError {
    /// Classify an application failure of the write described by `context`.
    #[must_use]
    pub fn of(context: &'static str, error: AppError) -> Self {
        if error.code() == "store_busy" {
            Self::Busy {
                context,
                source: Box::new(error),
            }
        } else {
            Self::Refused {
                context,
                source: Box::new(error),
            }
        }
    }

    /// The failure and every cause under it, as one line.
    #[must_use]
    pub fn chain(&self) -> String {
        let mut text = self.to_string();
        let mut next = self.source();
        while let Some(cause) = next {
            text.push_str(": ");
            text.push_str(&cause.to_string());
            next = cause.source();
        }
        text
    }
}

/// What a batch write did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Receipt {
    /// Records newly stored.
    pub recorded: usize,
    /// Records the store already held with the same content.
    pub duplicates: usize,
}

/// The durable destination of one run's records.
pub trait RunSink {
    /// Store a batch of activity, all or none.
    fn record_blocking(&mut self, entries: Vec<ActivityInput>) -> Result<Receipt, SinkError>;

    /// Record the run's terminal transition under `event`, which makes a repeat a replay.
    fn transition_blocking(
        &mut self,
        state: RunState,
        event: RunEventId,
        detail: String,
    ) -> Result<(), SinkError>;
}

/// The identity of a run's terminal transition: the run's own, with bits in its random part
/// changed, so repeating the transition after a restart is the same fact and the version and
/// variant of the identity stay valid.
#[must_use]
pub fn terminal_event(run: RunId) -> RunEventId {
    const MASK: u128 = 0x2d1e_4b7a_91c3_5f08;
    RunEventId(Uuid::from_u128(run.0.as_u128() ^ MASK))
}

/// A sink backed by an opened workspace.
pub struct AppSink {
    app: Application,
    recorder: ActorId,
    run: RunId,
    lineage: Option<LineageId>,
}

impl AppSink {
    /// A sink for `run`, written as `recorder` under the lineage the run was started in.
    #[must_use]
    pub fn new(
        app: Application,
        recorder: ActorId,
        run: RunId,
        lineage: Option<LineageId>,
    ) -> Self {
        Self {
            app,
            recorder,
            run,
            lineage,
        }
    }

    /// The application, for reads.
    #[must_use]
    pub fn application(&self) -> &Application {
        &self.app
    }
}

impl RunSink for AppSink {
    fn record_blocking(&mut self, entries: Vec<ActivityInput>) -> Result<Receipt, SinkError> {
        let written = self
            .app
            .record_run_activity_blocking(RunActivityRequest {
                actor: self.recorder.clone(),
                entries,
                base_lineage: self.lineage,
            })
            .map_err(|error| SinkError::of("recording run activity", error))?;
        let duplicates = written
            .data
            .entries
            .iter()
            .filter(|entry| entry.duplicate)
            .count();
        Ok(Receipt {
            recorded: written.data.entries.len().saturating_sub(duplicates),
            duplicates,
        })
    }

    fn transition_blocking(
        &mut self,
        state: RunState,
        event: RunEventId,
        detail: String,
    ) -> Result<(), SinkError> {
        self.app
            .report_run_blocking(RunReportRequest {
                actor: self.recorder.clone(),
                run: self.run,
                state,
                event_id: Some(event),
                detail: Some(detail),
                observed_at: None,
                base_lineage: self.lineage,
            })
            .map(|_| ())
            .map_err(|error| SinkError::of("recording the run's terminal state", error))
    }
}

/// What starting a run produced.
#[derive(Debug, Clone, Copy)]
pub struct Started {
    /// The run's identity.
    pub run: RunId,
    /// The lineage the run was recorded under.
    pub lineage: Option<LineageId>,
}

/// Who and what a new run is for.
#[derive(Debug, Clone)]
pub struct NewRun {
    /// The service recording the run; only a service may record a managed run.
    pub recorder: ActorId,
    /// The principal doing the work, which must own the task.
    pub executor: ActorId,
    /// The task's key.
    pub work_key: String,
    /// The run this one recovers, if it does.
    pub parent: Option<RunId>,
    /// The provider session this run belongs to.
    pub session: String,
    /// The adapter-assigned ordinal of this turn within the session: the provider has no turn
    /// identity of its own in this mode.
    pub turn: String,
    /// Immutable facts about the runtime, fixed for the life of the run.
    pub provenance: Option<Box<RunProvenance>>,
    /// The exact source the task's own evidence names, when it names one.
    pub sources: Vec<RunSource>,
}

/// Record a new managed run on a task its executor owns. The run's identity is minted here and
/// is its idempotency key. The application refuses a task that is not claimed or started, or that
/// its executor does not own, so a run is only ever recorded against the task's current state.
pub fn start_blocking(app: &mut Application, new: &NewRun) -> Result<Started, AppError> {
    let run = RunId::new();
    let written = app.start_run_blocking(RunStartRequest {
        actor: new.recorder.clone(),
        work_key: new.work_key.clone(),
        executor: Some(new.executor.clone()),
        run_id: Some(run),
        parent: new.parent,
        session: Some(RunSession {
            provider: "claude".into(),
            session: new.session.clone(),
            turn: Some(new.turn.clone()),
            provenance: new.provenance.clone(),
        }),
        observation: Observation::Managed,
        sources: new.sources.clone(),
        observed_at: None,
        base_lineage: None,
    })?;
    Ok(Started {
        run,
        lineage: written.lineage_id,
    })
}

/// A run as recorded, read for recovery.
#[derive(Debug)]
pub struct Prior {
    /// The run with its derived observation.
    pub view: RunView,
    /// The lineage the store continues.
    pub lineage: Option<LineageId>,
}

/// Read a run.
pub fn prior_blocking(app: &Application, run: RunId) -> Result<Prior, AppError> {
    let observed = app.run_blocking(run)?;
    Ok(Prior {
        view: observed.data,
        lineage: observed.lineage_id,
    })
}

/// The note that records the gap a recovery leaves in the run it closes.
fn gap_note(run: RunId, high_water: u64) -> ActivityInput {
    ActivityInput {
        run,
        source_sequence: high_water.wrapping_add(1),
        kind: ActivityKind::Progress,
        text: Some(format!(
            "adapter gap-{}: the adapter that recorded this run ended; provider events after source sequence {high_water} were not observed and cannot be replayed, so what became of its turn is unknown",
            high_water.wrapping_add(1)
        )),
        observed_at: None,
    }
}

/// Close a run whose adapter ended, without claiming anything about its provider turn: record the
/// gap in its activity, then an interruption whose detail says the fate is unknown. Both are
/// idempotent, so repeating the recovery after a failure is safe.
pub fn close_prior_blocking(
    app: &mut Application,
    recorder: &ActorId,
    prior: &Prior,
) -> Result<(), SinkError> {
    let run = prior.view.run.id;
    let high_water = prior.view.activity.source_high_water;
    app.record_run_activity_blocking(RunActivityRequest {
        actor: recorder.clone(),
        entries: vec![gap_note(run, high_water)],
        base_lineage: prior.lineage,
    })
    .map_err(|error| SinkError::of("recording the recovery gap", error))?;
    app.report_run_blocking(RunReportRequest {
        actor: recorder.clone(),
        run,
        state: RunState::Interrupted,
        event_id: Some(terminal_event(run)),
        detail: Some(format!(
            "the adapter that recorded this run ended; its provider turn's fate is unknown after source sequence {high_water}"
        )),
        observed_at: None,
        base_lineage: prior.lineage,
    })
    .map(|_| ())
    .map_err(|error| SinkError::of("closing the recovered run", error))
}
