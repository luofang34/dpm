//! One provider turn, from handshake to the recorded terminal state.
//!
//! The session owns the loop between the provider, the intake and the sink, in two phases. First
//! it starts the provider and speaks the documented stream-json control protocol only as far as
//! `initialize`, then waits briefly for the provider to announce its session, so the run can be
//! recorded with whatever the provider announced as immutable provenance; frames read meanwhile are
//! kept and replayed in order. A provider that does not acknowledge `initialize` with a matching
//! success response is never recorded and never sent anything: its output ending, a bound being
//! reached or a session announced out of turn all fail the session closed. Only once DPM has
//! recorded the run does it send the one user message, so nothing the provider can do happens
//! before the run exists. Then it records: activity
//! in bounded batches in the order it was accepted, an answer to every permission request that is
//! always a refusal, and a terminal transition written last, once, under an identity derived from
//! the run. A provider that announces itself only after the prompt is recorded with its observed
//! identity absent from the run's provenance, and the announcement is kept as ordinary activity.
//!
//! Every wait has a bound. Each pass over the provider's output handles a bounded number of frames
//! before the deadline and the cancel flag are looked at again, so a stream of frames that record
//! nothing cannot starve them, and a batch that cannot be written is retried only within a time
//! budget, after which the provider is stopped rather than left to run unobserved.

use crate::{
    event::{PublicEvent, Session, parse_line},
    frame::Frame,
    intake::{Action, End, Intake, Stop},
    process::{Message, Provider, Received},
    sink::{RunSink, SinkError, terminal_event},
};
use dpm_model::{ActivityInput, RunId};
use serde_json::{Value, json};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};

mod shapes;
pub use shapes::{BeginError, Begun, DriveFailure, HandshakeError, Report, Retry, Turn};

/// Identity of this adapter's one `initialize` request.
const INITIALIZE: &str = "dpm-init-1";

/// What the provider is told when it asks for something this adapter will not grant.
const DENIAL: &str = "refused by DPM: this run has no approvals and no answers to give";

/// Most frames handled before the deadline and cancel flag are looked at again.
const FRAMES_PER_PASS: usize = 256;

/// The longest one write to the provider may wait for the pipe to drain.
const WRITE_BOUND: Duration = Duration::from_secs(5);

/// Most frames kept while waiting for the provider to announce its session.
const BEFORE_ANNOUNCE: usize = 64;

fn retrying<T>(
    retry: Retry,
    mut write: impl FnMut() -> Result<T, SinkError>,
) -> Result<T, SinkError> {
    let started = Instant::now();
    let mut attempt = 0_u32;
    loop {
        match write() {
            Err(SinkError::Busy { .. })
                if attempt.wrapping_add(1) < retry.attempts && started.elapsed() < retry.budget =>
            {
                attempt = attempt.wrapping_add(1);
                let pause = retry
                    .backoff
                    .saturating_mul(attempt)
                    .min(retry.budget.saturating_sub(started.elapsed()));
                thread::sleep(pause);
            }
            other => return other,
        }
    }
}

/// Write `pending` in order and in bounded chunks; on failure what was not written stays in `pending`.
fn flush<S: RunSink>(
    sink: &mut S,
    pending: &mut Vec<ActivityInput>,
    plan: &Turn,
) -> Result<(), SinkError> {
    let size = plan.batch.clamp(1, dpm_model::MAX_ACTIVITY_BATCH);
    while !pending.is_empty() {
        let chunk: Vec<ActivityInput> = pending.iter().take(size).cloned().collect();
        let count = chunk.len();
        retrying(plan.retry, || sink.record_blocking(chunk.clone()))?;
        pending.drain(..count);
    }
    Ok(())
}

/// The provider and what has been said to it.
struct Wire<'a> {
    provider: Provider,
    plan: &'a Turn,
    cancel: &'a AtomicBool,
    started: Instant,
    prompted: bool,
    stop: Option<Stop>,
}

impl Wire<'_> {
    fn stop_with(&mut self, why: Stop) {
        self.stop.get_or_insert(why);
    }

    /// Write one line to the provider within the time the session has left, abandoning it if the
    /// run is cancelled.
    fn send(&mut self, value: &Value) -> Result<(), String> {
        let left = self.plan.deadline.saturating_sub(self.started.elapsed());
        let cancel = self.cancel;
        self.provider
            .send_line_blocking(&value.to_string(), left.min(WRITE_BOUND), &|| {
                cancel.load(Ordering::SeqCst)
            })
            .map_err(|error| error.to_string())
    }

    fn initialize(&mut self) {
        let request = json!({"type": "control_request", "request_id": INITIALIZE, "request": {"subtype": "initialize", "hooks": null}});
        if let Err(error) = self.send(&request) {
            self.stop_with(Stop::Protocol(format!(
                "the handshake could not be written: {error}"
            )));
        }
    }

    /// Send the one prompt. Called only after the run has been recorded.
    fn prompt(&mut self) {
        if self.prompted || self.stop.is_some() {
            return;
        }
        self.prompted = true;
        let message = json!({"type": "user", "session_id": "", "message": {"role": "user", "content": self.plan.prompt}, "parent_tool_use_id": null});
        if let Err(error) = self.send(&message) {
            self.stop_with(Stop::Protocol(format!(
                "the prompt could not be written: {error}"
            )));
        }
    }

    /// Whether to keep going: false once a stop is set, the cancel flag is raised or time is up.
    fn within_bounds(&mut self, cancel: &AtomicBool, started: Instant) -> Option<Duration> {
        if cancel.load(Ordering::SeqCst) {
            self.stop_with(Stop::Cancelled);
        }
        match self
            .plan
            .deadline
            .checked_sub(started.elapsed())
            .filter(|left| !left.is_zero())
        {
            Some(left) if self.stop.is_none() => Some(left),
            Some(_) => None,
            None => {
                self.stop_with(Stop::Deadline);
                None
            }
        }
    }
}

/// What the provider said before the run was recorded.
struct Announcement {
    /// Every frame read, to be replayed in order once the run exists.
    kept: Vec<Frame>,
    /// The session it announced, accepted only after the handshake was acknowledged.
    announced: Option<Session>,
    /// When it acknowledged `initialize`: the one fact that lets the run be recorded and the
    /// prompt be sent.
    acknowledged: Option<Instant>,
    /// Whether its output ended.
    ended: bool,
}

/// Take what one line says about the handshake. Only a successful `control_response` to this
/// adapter's own `initialize` request acknowledges it, and a session announced before that proves
/// nothing about the handshake, so it fails the session closed.
fn inspect(wire: &mut Wire<'_>, line: &str, seen: &mut Announcement) {
    for event in parse_line(line)
        .map(|parsed| parsed.events)
        .unwrap_or_default()
    {
        match event {
            PublicEvent::Answered { request, ok: true } if request == INITIALIZE => {
                seen.acknowledged.get_or_insert_with(Instant::now);
            }
            PublicEvent::Answered { request, ok: false } if request == INITIALIZE => {
                wire.stop_with(Stop::Protocol("the provider refused to initialize".into()));
            }
            PublicEvent::Initialized(session) if seen.acknowledged.is_some() => {
                seen.announced = Some(session);
            }
            PublicEvent::Initialized(_) => wire.stop_with(Stop::Protocol(
                "the provider announced a session before it acknowledged the handshake".into(),
            )),
            _ => {}
        }
    }
}

/// Wait for the provider to acknowledge `initialize` and announce its session, keeping the frames
/// it sends meanwhile. No prompt has been sent: the provider has been given nothing to act on.
fn announce(wire: &mut Wire<'_>, cancel: &AtomicBool, started: Instant) -> Announcement {
    let mut seen = Announcement {
        kept: Vec::new(),
        announced: None,
        acknowledged: None,
        ended: false,
    };
    wire.initialize();
    while seen.announced.is_none() && seen.kept.len() < BEFORE_ANNOUNCE {
        if seen
            .acknowledged
            .is_some_and(|at| at.elapsed() >= wire.plan.announce_wait)
        {
            break;
        }
        let Some(left) = wire.within_bounds(cancel, started) else {
            break;
        };
        let message = match wire
            .provider
            .next_blocking(left.min(Duration::from_millis(100)))
        {
            Received::Timeout if wire.provider.has_exited() => {
                seen.ended = true;
                break;
            }
            Received::Timeout => continue,
            Received::Message(message) => message,
        };
        match message {
            Message::Frame(frame) => {
                if let Frame::Line(line) = &frame {
                    inspect(wire, line, &mut seen);
                }
                seen.kept.push(frame);
            }
            Message::End => {
                seen.ended = true;
                break;
            }
            Message::Failed(error) => {
                wire.stop_with(Stop::Protocol(format!(
                    "reading the provider's output failed: {error}"
                )));
                seen.ended = true;
                break;
            }
        }
    }
    seen
}

/// Why no run was recorded when the handshake was never acknowledged.
fn unacknowledged(wire: &Wire<'_>, ended: bool) -> HandshakeError {
    match &wire.stop {
        Some(Stop::Protocol(reason)) => HandshakeError::Protocol(reason.clone()),
        Some(Stop::Deadline) => HandshakeError::Deadline,
        Some(Stop::Cancelled) => HandshakeError::Cancelled,
        _ if ended => HandshakeError::Ended,
        _ => HandshakeError::Silent {
            frames: BEFORE_ANNOUNCE,
        },
    }
}

/// The recording loop of one run.
struct Recording<'a, S> {
    wire: Wire<'a>,
    sink: &'a mut S,
    intake: Intake,
    pending: Vec<ActivityInput>,
    sink_failure: Option<SinkError>,
}

impl<S: RunSink> Recording<'_, S> {
    fn act(&mut self, action: Action) {
        match action {
            Action::Answered { .. } => {}
            Action::Deny { request, subject } => {
                let reply = json!({"type": "control_response", "response": {
                    "subtype": "success", "request_id": request, "response": {"behavior": "deny", "message": DENIAL}}});
                let delivered = self.wire.send(&reply);
                let failed = delivered.is_err();
                let step = self.intake.replied(&subject, delivered);
                self.pending.extend(step.records);
                if failed {
                    self.wire.stop_with(Stop::Protocol(
                        "a refusal could not be written, so the provider would wait for it".into(),
                    ));
                }
            }
            Action::Refuse { request } => {
                let reply = json!({"type": "control_response", "response": {
                    "subtype": "error", "request_id": request, "error": "this adapter does not support that control request"}});
                if self.wire.send(&reply).is_err() {
                    self.wire.stop_with(Stop::Protocol(
                        "a refusal could not be written, so the provider would wait for it".into(),
                    ));
                }
            }
        }
    }

    /// Take one message; true when the provider's output has ended.
    fn take(&mut self, message: Message) -> bool {
        match message {
            Message::Frame(frame) => {
                let step = self.intake.accept(frame);
                self.pending.extend(step.records);
                for action in step.actions {
                    self.act(action);
                }
                if self.intake.finished().is_some() {
                    // The turn is over: closing the input is how the provider is told to end.
                    self.wire.provider.close_input();
                }
                false
            }
            Message::End => true,
            Message::Failed(error) => {
                self.wire.stop_with(Stop::Protocol(format!(
                    "reading the provider's output failed: {error}"
                )));
                true
            }
        }
    }

    fn write_pending(&mut self, plan: &Turn) {
        if self.sink_failure.is_some() {
            return;
        }
        if let Err(error) = flush(self.sink, &mut self.pending, plan) {
            self.sink_failure = Some(error);
            self.wire.stop_with(Stop::TelemetryLoss);
        }
    }

    /// Run until the output ends, a bound is reached or recording fails.
    fn run(&mut self, plan: &Turn, cancel: &AtomicBool, started: Instant) {
        while let Some(left) = self.wire.within_bounds(cancel, started) {
            let mut ended = match self
                .wire
                .provider
                .next_blocking(left.min(Duration::from_millis(100)))
            {
                // A silent interval after the process exited means nothing more is coming, even if
                // something it started still holds its pipes and no end of input arrives.
                Received::Timeout => self.wire.provider.has_exited(),
                Received::Message(message) => self.take(message),
            };
            // A bounded pass: frames that record nothing still count, so the bounds are re-read.
            let mut handled = 1;
            while !ended && handled < FRAMES_PER_PASS && self.pending.len() < plan.batch {
                let Some(message) = self.wire.provider.try_next() else {
                    break;
                };
                ended = self.take(message);
                handled += 1;
            }
            self.write_pending(plan);
            if ended {
                break;
            }
        }
    }
}

/// Drive one provider turn to the end and record how it ended. `begin` records the run once the
/// provider has announced its session, or has failed to, and receives that announcement.
pub fn drive_blocking<S: RunSink>(
    provider: Provider,
    plan: &Turn,
    begin: impl FnOnce(Option<&Session>) -> Result<Begun<S>, BeginError>,
    cancel: &AtomicBool,
) -> Result<Report, DriveFailure> {
    let started = Instant::now();
    let mut wire = Wire {
        provider,
        plan,
        cancel,
        started,
        prompted: false,
        stop: None,
    };
    let Announcement {
        kept,
        announced,
        acknowledged,
        ended,
    } = announce(&mut wire, cancel, started);
    if acknowledged.is_none() {
        // Nothing was acknowledged, so nothing is recorded and nothing is asked of the provider.
        let error = BeginError::new("the handshake", unacknowledged(&wire, ended));
        let stopped = wire.provider.stop_blocking(plan.grace);
        return Err(DriveFailure { error, stopped });
    }
    let Begun { mut sink, run } = match begin(announced.as_ref()) {
        Ok(begun) => begun,
        Err(error) => {
            let stopped = wire.provider.stop_blocking(plan.grace);
            return Err(DriveFailure { error, stopped });
        }
    };
    // The run is recorded: only now may the provider be given something to do.
    wire.prompt();
    let intake = Intake::new(run, 0, Some(plan.session.clone()));
    let mut recording = Recording {
        wire,
        sink: &mut sink,
        intake,
        pending: Vec::new(),
        sink_failure: None,
    };
    let mut finished = ended;
    for frame in kept {
        finished = recording.take(Message::Frame(frame)) || finished;
    }
    recording.write_pending(plan);
    if !finished && recording.wire.stop.is_none() {
        recording.run(plan, cancel, started);
    }
    Ok(close(recording, plan, run))
}

/// Stop the provider and write what is left, then the one terminal transition.
fn close<S: RunSink>(mut recording: Recording<'_, S>, plan: &Turn, run: RunId) -> Report {
    let stopped = recording.wire.provider.stop_blocking(plan.grace);
    let end = End {
        exit: stopped.reaped.then_some(stopped.exit),
        stopped: recording.wire.stop.clone(),
        unresolved: !stopped.reaped,
    };
    let closing = recording.intake.finish(&end);
    recording.pending.extend(closing.records);
    recording.write_pending(plan);
    let terminal = closing.terminal;
    let recorded = recording.sink_failure.is_none()
        && match retrying(plan.retry, || {
            recording.sink.transition_blocking(
                terminal.state,
                terminal_event(run),
                terminal.detail.clone(),
            )
        }) {
            Ok(()) => true,
            Err(error) => {
                recording.sink_failure = Some(error);
                false
            }
        };
    Report {
        run,
        terminal,
        terminal_recorded: recorded,
        sink_failure: recording.sink_failure,
        tally: recording.intake.tally(),
        stopped,
        stderr_bytes: recording.wire.provider.stderr_bytes(),
        pending_high_water: recording.wire.provider.pending_high_water(),
        window_complete: recording.intake.window_complete(),
    }
}
