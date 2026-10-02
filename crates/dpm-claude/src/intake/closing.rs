//! How a run ends: which evidence decides its terminal state.
//!
//! Evidence is kept apart on purpose. Only the provider's own explicit, well-formed terminal result
//! can complete a run. The provider's output ending, a clean exit and a nonzero exit are not that
//! result: each is a distinct failure of the turn, never a success, and a stop requested by this
//! adapter or a signal is an interruption. Whether the task itself was submitted or verified is not
//! decided here and never follows from any of them.

use super::{Intake, MALFORMED_NOTES, identified};
use crate::event::{Finish, Verdict};
use crate::redact::public_text;
use dpm_model::{ActivityInput, ActivityKind, RunState};

/// How the provider process ended.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Exit {
    /// Its exit status, when it exited by itself.
    pub code: Option<i32>,
    /// The signal that ended it, when it did not.
    pub signal: Option<i32>,
}

/// Why this adapter stopped the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    /// The adapter's time bound ended the run.
    Deadline,
    /// The operator asked the adapter to stop.
    Cancelled,
    /// The adapter could not record the provider's activity and stopped rather than continue blind.
    TelemetryLoss,
    /// The provider broke the protocol: it refused to initialize, or the like.
    Protocol(String),
}

/// What is known when the provider's output has ended.
#[derive(Debug, Clone, Default)]
pub struct End {
    /// How the process ended, if it was reaped.
    pub exit: Option<Exit>,
    /// Why the adapter stopped it, if it did.
    pub stopped: Option<Stop>,
    /// Whether the provider could not be reaped even after it was killed: an unresolved process,
    /// stated in the terminal detail and never hidden.
    pub unresolved: bool,
}

/// The kind of evidence that decided the terminal state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The provider's explicit terminal result reported success.
    ResultSuccess,
    /// The provider's explicit terminal result reported an error.
    ResultError,
    /// The provider's explicit terminal result reported that the turn was interrupted.
    ResultInterrupted,
    /// The provider's output ended and it exited without any terminal result.
    EofWithoutResult,
    /// The provider exited with a nonzero status without a terminal result.
    NonzeroExit,
    /// A signal ended the provider before a terminal result.
    Signal,
    /// This adapter stopped the provider before a terminal result.
    AdapterStopped,
    /// The provider broke the protocol.
    ProtocolFailure,
}

impl Evidence {
    /// The lowercase word reports spell the evidence with.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::ResultSuccess => "result_success",
            Self::ResultError => "result_error",
            Self::ResultInterrupted => "result_interrupted",
            Self::EofWithoutResult => "eof_without_result",
            Self::NonzeroExit => "nonzero_exit",
            Self::Signal => "signal",
            Self::AdapterStopped => "adapter_stopped",
            Self::ProtocolFailure => "protocol_failure",
        }
    }
}

/// The lifecycle transition that ends the run, with what justified it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Terminal {
    /// The state to record.
    pub state: RunState,
    /// What decided it.
    pub evidence: Evidence,
    /// Why, in words, bounded.
    pub detail: String,
}

/// Records to write before the terminal transition, and the transition.
#[derive(Debug)]
pub struct Closing {
    /// Final notes about what was left out; empty when nothing was.
    pub records: Vec<ActivityInput>,
    /// The terminal transition.
    pub terminal: Terminal,
}

fn describe(exit: Exit) -> String {
    match (exit.code, exit.signal) {
        (_, Some(signal)) => format!("terminated by signal {signal}"),
        (Some(code), None) => format!("exited with status {code}"),
        (None, None) => "ended with an unknown status".to_string(),
    }
}

type Ending = (RunState, Evidence, String);

/// The ending the provider's own terminal result describes, with how the process then went.
fn from_result(finish: &Finish, end: &End) -> Ending {
    let after = match (end.exit, end.stopped.is_some()) {
        (Some(exit), false) if exit.code.is_some_and(|code| code != 0) || exit.signal.is_some() => {
            format!("; the process then {}", describe(exit))
        }
        _ => String::new(),
    };
    let reason = finish
        .terminal_reason
        .as_deref()
        .unwrap_or("no reason given");
    let turns = finish
        .turns
        .map_or_else(String::new, |turns| format!(", {turns} turns"));
    match finish.verdict {
        Verdict::Completed => (
            RunState::Completed,
            Evidence::ResultSuccess,
            format!(
                "the provider reported a finished turn ({reason}{turns}); the task is neither submitted nor verified{after}"
            ),
        ),
        Verdict::Interrupted => (
            RunState::Interrupted,
            Evidence::ResultInterrupted,
            format!("the provider reported the turn was interrupted ({reason}{turns}){after}"),
        ),
        Verdict::Failed => {
            let error = finish.error.as_deref().unwrap_or("no detail");
            (
                RunState::Failed,
                Evidence::ResultError,
                format!("the provider reported {}: {error}{after}", finish.subtype),
            )
        }
    }
}

/// The ending when this adapter stopped the provider before any terminal result.
fn from_stop(stop: &Stop) -> Ending {
    match stop {
        Stop::Deadline => (
            RunState::Interrupted,
            Evidence::AdapterStopped,
            "stopped by the adapter at its time bound before any terminal result".into(),
        ),
        Stop::Cancelled => (
            RunState::Interrupted,
            Evidence::AdapterStopped,
            "stopped on request before any terminal result".into(),
        ),
        Stop::TelemetryLoss => (
            RunState::Interrupted,
            Evidence::AdapterStopped,
            "stopped by the adapter because it could not record the provider's activity".into(),
        ),
        Stop::Protocol(why) => (
            RunState::Failed,
            Evidence::ProtocolFailure,
            format!("the provider broke the protocol: {why}"),
        ),
    }
}

/// The ending when the provider's output simply ended: how the process went is all there is.
fn from_exit(exit: Exit) -> Ending {
    if exit.signal.is_some() {
        (
            RunState::Interrupted,
            Evidence::Signal,
            format!("the provider {} before any terminal result", describe(exit)),
        )
    } else if exit.code.is_some_and(|code| code != 0) {
        (
            RunState::Failed,
            Evidence::NonzeroExit,
            format!(
                "the provider {} without any terminal result",
                describe(exit)
            ),
        )
    } else {
        (
            RunState::Failed,
            Evidence::EofWithoutResult,
            format!(
                "the provider's output ended and it {} without any terminal result: the turn is unfinished",
                describe(exit)
            ),
        )
    }
}

impl Intake {
    /// The terminal state the evidence supports, and the final notes.
    pub fn finish(&mut self, end: &End) -> Closing {
        let terminal = self.decide(end);
        let records = self.closing_note().into_iter().collect();
        Closing { records, terminal }
    }

    fn decide(&self, end: &End) -> Terminal {
        let (state, evidence, detail) = if let Some(finish) = self.finish.as_ref() {
            from_result(finish, end)
        } else if let Some(stop) = end.stopped.as_ref() {
            from_stop(stop)
        } else {
            from_exit(end.exit.unwrap_or_default())
        };
        let unreaped = if end.unresolved {
            "; the provider process could not be reaped and may still be running"
        } else {
            ""
        };
        Terminal {
            state,
            evidence,
            detail: public_text(&format!("{detail}{unreaped}"), 400),
        }
    }

    /// One note saying what the allowlist and the identity rules left out, if anything.
    fn closing_note(&mut self) -> Option<ActivityInput> {
        let tally = self.tally;
        let dropped = tally.dropped;
        let further = tally.malformed.saturating_sub(MALFORMED_NOTES);
        let counts = [
            (u64::from(dropped.hidden_reasoning), "reasoning blocks"),
            (
                u64::from(dropped.unlisted_blocks),
                "unlisted content blocks",
            ),
            (u64::from(dropped.unlisted_events), "unlisted events"),
            (
                u64::from(dropped.unidentified),
                "events without a provider identity",
            ),
            (
                tally.foreign,
                "events from before initialization or another session",
            ),
            (u64::from(further), "further malformed lines"),
        ];
        let mut parts: Vec<String> = counts
            .iter()
            .filter(|(count, _)| *count > 0)
            .map(|(count, what)| format!("{count} {what}"))
            .collect();
        if tally.conflicts > 0 {
            parts.push(format!(
                "{} repeated identities with conflicting content",
                tally.conflicts
            ));
        }
        if !self.window_complete() {
            parts.push("a replay window that no longer covers every earlier record".to_string());
        }
        if parts.is_empty() {
            return None;
        }
        let id = format!("closing-{}", self.next);
        let text = identified(
            "adapter",
            &id,
            &format!("left out and not retained: {}", parts.join(", ")),
        );
        Some(self.record(ActivityKind::Progress, text))
    }
}
