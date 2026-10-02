//! From public events to run records: ordering, identity and the terminal outcome.
//!
//! The intake performs no I/O. It turns each provider frame into activity records with strictly
//! increasing source sequences and remembers which provider identities it has recorded, together
//! with what was recorded for them, so a repeated event adds nothing and a repeat with different
//! content is a reported conflict, never silently treated as a replay.
//!
//! The remembered identities are a bounded replay window, not a provider cursor. The provider
//! does not replay its stream, so recognition is only a defence against duplicate delivery within
//! one intake. When more identities pass than the window holds, the oldest are forgotten, the
//! window is reported incomplete, and a forgotten identity that came back would be recorded as
//! new: replaying a provider stream into a run beyond its window is not supported, and the run's
//! own record says so. A new intake, such as the one of a recovery run, starts with an empty
//! window and a new run identity; it never claims to continue another run's stream.

use crate::{
    event::{Dropped, Finish, Parsed, PublicEvent, Session, parse_line},
    frame::Frame,
    redact::public_text,
};
use dpm_model::{ActivityInput, ActivityKind, RunId};
use std::collections::{HashMap, VecDeque};

mod closing;
pub use closing::{Closing, End, Evidence, Exit, Stop, Terminal};

/// Provider identities remembered for deduplication: the replay window.
pub const SEEN_LIMIT: usize = 8192;

/// Malformed frames recorded one by one; further ones are counted and summarized once.
pub const MALFORMED_NOTES: u32 = 20;

/// What the session must do because of a frame, beyond recording it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// The provider answered one of this adapter's control requests.
    Answered {
        /// The request answered.
        request: String,
        /// Whether it reports success.
        ok: bool,
    },
    /// Refuse a permission request. Approving is never an action.
    Deny {
        /// The provider's request identity, which the reply must carry.
        request: String,
        /// What the refusal is about: the tool use, or else the request.
        subject: String,
    },
    /// Refuse a control request this adapter does not support.
    Refuse {
        /// The provider's request identity.
        request: String,
    },
}

/// What one frame produced.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Step {
    /// Records to write, in order, with their sequences already assigned.
    pub records: Vec<ActivityInput>,
    /// What the session must do.
    pub actions: Vec<Action>,
}

/// How an identity compares with what was recorded for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    /// Never seen.
    New,
    /// Seen with exactly this content.
    Same,
    /// Seen with different content.
    Conflict,
}

/// The identities recorded and their texts, bounded.
#[derive(Debug, Default)]
struct Seen {
    texts: HashMap<String, String>,
    order: VecDeque<String>,
    evicted: u64,
}

impl Seen {
    /// Compare `text` with what `key` recorded, and remember it when new.
    fn judge(&mut self, key: &str, text: &str) -> Verdict {
        match self.texts.get(key) {
            Some(known) if known == text => Verdict::Same,
            Some(_) => Verdict::Conflict,
            None => {
                self.texts.insert(key.to_string(), text.to_string());
                self.order.push_back(key.to_string());
                if self.order.len() > SEEN_LIMIT
                    && let Some(oldest) = self.order.pop_front()
                {
                    self.texts.remove(&oldest);
                    self.evicted = self.evicted.wrapping_add(1);
                }
                Verdict::New
            }
        }
    }
}

/// Counts of what was left out, refused or could not be read.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tally {
    /// Records written, including notes.
    pub recorded: u64,
    /// Events whose identity was already recorded with the same content.
    pub repeated: u64,
    /// Events whose identity was already recorded with different content.
    pub conflicts: u64,
    /// Frames that were not events.
    pub malformed: u32,
    /// Provider material left out by the allowlist.
    pub dropped: Dropped,
    /// Session-scoped events refused because they came before the root session initialized or from
    /// another session.
    pub foreign: u64,
}

/// Turns provider frames into ordered, deduplicated run records.
#[derive(Debug)]
pub struct Intake {
    run: RunId,
    next: u64,
    seen: Seen,
    /// The root provider session: the one the run recorded, or else the first that initializes.
    root: Option<String>,
    initialized: bool,
    finish: Option<Finish>,
    tally: Tally,
}

/// The identity-bearing text of a record: `kind id: rest`.
fn identified(kind: &str, id: &str, rest: &str) -> String {
    if rest.is_empty() {
        format!("{kind} {id}")
    } else {
        format!("{kind} {id}: {rest}")
    }
}

impl Intake {
    /// An intake that continues a run whose accepted source sequences reach `after_sequence`.
    /// `expected_session` is the provider session the run recorded, to notice a different one.
    #[must_use]
    pub fn new(run: RunId, after_sequence: u64, expected_session: Option<String>) -> Self {
        Self {
            run,
            next: after_sequence.wrapping_add(1),
            seen: Seen::default(),
            root: expected_session,
            initialized: false,
            finish: None,
            tally: Tally::default(),
        }
    }

    /// What has been counted so far.
    #[must_use]
    pub fn tally(&self) -> Tally {
        self.tally
    }

    /// Whether the replay window still covers every record the run has had.
    #[must_use]
    pub fn window_complete(&self) -> bool {
        self.seen.evicted == 0
    }

    /// The provider's explicit terminal result, once seen.
    #[must_use]
    pub fn finished(&self) -> Option<&Finish> {
        self.finish.as_ref()
    }

    fn record(&mut self, kind: ActivityKind, text: String) -> ActivityInput {
        let entry = ActivityInput {
            run: self.run,
            source_sequence: self.next,
            kind,
            text: Some(text),
            observed_at: None,
        };
        self.next = self.next.wrapping_add(1);
        self.tally.recorded = self.tally.recorded.wrapping_add(1);
        entry
    }

    /// An adapter note, identified by its own sequence so a restart never collides with it.
    fn note(&mut self, what: &str, detail: &str) -> ActivityInput {
        let id = format!("{what}-{}", self.next);
        self.record(ActivityKind::Progress, identified("adapter", &id, detail))
    }

    /// Record `text` once per provider identity; a repeat with other content is a reported conflict.
    fn once(&mut self, step: &mut Step, key: String, kind: ActivityKind, text: String) {
        match self.seen.judge(&key, &text) {
            Verdict::New => {
                let entry = self.record(kind, text);
                step.records.push(entry);
            }
            Verdict::Same => self.tally.repeated = self.tally.repeated.wrapping_add(1),
            Verdict::Conflict => {
                self.tally.conflicts = self.tally.conflicts.wrapping_add(1);
                let detail = format!(
                    "provider identity {key} repeated with different content; the first record stands and this one was not kept"
                );
                let entry = self.note("conflict", &public_text(&detail, 220));
                step.records.push(entry);
            }
        }
    }

    /// Take one provider frame.
    pub fn accept(&mut self, frame: Frame) -> Step {
        let mut step = Step::default();
        let problem = match frame {
            Frame::Line(line) if line.trim().is_empty() => return step,
            Frame::Line(line) => match parse_line(&line) {
                Ok(parsed) => {
                    self.accept_parsed(parsed, &mut step);
                    return step;
                }
                Err(error) => format!(
                    "a line that is not a provider event ({error}, {} bytes)",
                    line.len()
                ),
            },
            Frame::Oversized { bytes } => format!("an oversized line ({bytes} bytes)"),
            Frame::NotUtf8 { bytes } => format!("a line that is not UTF-8 ({bytes} bytes)"),
            Frame::Truncated { bytes } => format!("a truncated final line ({bytes} bytes)"),
        };
        self.tally.malformed = self.tally.malformed.wrapping_add(1);
        if self.tally.malformed <= MALFORMED_NOTES {
            let entry = self.note("malformed", &format!("dropped {problem}"));
            step.records.push(entry);
        }
        step
    }

    fn accept_parsed(&mut self, parsed: Parsed, step: &mut Step) {
        let Dropped {
            hidden_reasoning,
            unlisted_blocks,
            unlisted_events,
            unidentified,
        } = parsed.dropped;
        let total = &mut self.tally.dropped;
        total.hidden_reasoning = total.hidden_reasoning.wrapping_add(hidden_reasoning);
        total.unlisted_blocks = total.unlisted_blocks.wrapping_add(unlisted_blocks);
        total.unlisted_events = total.unlisted_events.wrapping_add(unlisted_events);
        total.unidentified = total.unidentified.wrapping_add(unidentified);
        let scope = Scope {
            session: parsed.session,
            subagent: parsed.subagent,
        };
        for event in parsed.events {
            self.accept_event(event, &scope, step);
        }
    }

    /// Whether an event belongs to the root conversation of an initialized session. Anything else,
    /// from before the session started, from another session or from a subagent's turn, is counted
    /// and goes no further: a subagent's text, tool uses, results and own result are not this run's
    /// activity and never end it. Only its permission requests are still answered, since the
    /// provider waits on them.
    fn in_root(&mut self, scope: &Scope) -> bool {
        let belongs = self.initialized
            && !scope.subagent
            && self
                .root
                .as_deref()
                .is_some_and(|root| scope.session.as_deref() == Some(root));
        if !belongs {
            self.tally.foreign = self.tally.foreign.wrapping_add(1);
        }
        belongs
    }

    fn accept_event(&mut self, event: PublicEvent, scope: &Scope, step: &mut Step) {
        match event {
            PublicEvent::Initialized(session) => self.session(&session, step),
            PublicEvent::Answered { request, ok } => {
                step.actions.push(Action::Answered { request, ok })
            }
            PublicEvent::Text { .. }
            | PublicEvent::ToolUse { .. }
            | PublicEvent::ToolResult { .. }
            | PublicEvent::Retry { .. }
            | PublicEvent::Denied { .. }
            | PublicEvent::Finished(_)
                if !self.in_root(scope) => {}
            event @ (PublicEvent::Text { .. }
            | PublicEvent::ToolUse { .. }
            | PublicEvent::ToolResult { .. }
            | PublicEvent::Retry { .. }) => self.plain_event(event, step),
            PublicEvent::InputRequest {
                request,
                tool_use,
                tool,
                detail,
            } => {
                let subject = tool_use.unwrap_or_else(|| request.clone());
                self.input_request(step, &subject, &tool, &detail, false);
                step.actions.push(Action::Deny { request, subject });
            }
            PublicEvent::Denied {
                tool_use,
                tool,
                detail,
            } => {
                let subject = tool_use.unwrap_or_else(|| format!("denied-{tool}"));
                self.input_request(step, &subject, &tool, &detail, true);
            }
            PublicEvent::UnsupportedControl { request, subtype } => {
                let entry = self.note(
                    "unsupported",
                    &format!("control request {subtype} ({request}) is not supported; this adapter answers only permission requests"),
                );
                step.records.push(entry);
                step.actions.push(Action::Refuse { request });
            }
            PublicEvent::Finished(finish) => {
                self.finish.get_or_insert(finish);
            }
            PublicEvent::MalformedResult { reason } => {
                let entry = self.note(
                    "malformed-result",
                    &format!("ignored a result event that ends nothing: {reason}"),
                );
                step.records.push(entry);
            }
        }
    }

    /// The events that record once per provider identity and need nothing else done: text, tool
    /// starts and results, and retry notices.
    fn plain_event(&mut self, event: PublicEvent, step: &mut Step) {
        match event {
            PublicEvent::Text { id, text } => {
                self.once(
                    step,
                    format!("message:{id}"),
                    ActivityKind::Progress,
                    identified("message", &id, &text),
                );
            }
            PublicEvent::ToolUse { id, tool, detail } => {
                let rest = if detail.is_empty() {
                    tool
                } else {
                    format!("{tool}: {detail}")
                };
                self.once(
                    step,
                    format!("tool_use:{id}"),
                    ActivityKind::ToolStarted,
                    identified("tool_use", &id, &rest),
                );
            }
            PublicEvent::ToolResult { id, error, detail } => {
                let rest = if error {
                    format!("error: {detail}")
                } else {
                    detail
                };
                self.once(
                    step,
                    format!("tool_result:{id}"),
                    ActivityKind::ToolResult,
                    identified("tool_result", &id, &rest),
                );
            }
            PublicEvent::Retry { key, detail } => {
                self.once(
                    step,
                    format!("retry:{key}"),
                    ActivityKind::Progress,
                    identified("retry", &key, &detail),
                );
            }
            _ => {}
        }
    }

    /// One input request per provider tool use, however many ways the provider reports it: the
    /// control request, a recap in the terminal result, or a repeat. A recap is the same request
    /// described again, not new content for an identity already recorded, so it is a repeat and
    /// never a conflict; it records only when no control request ever did.
    fn input_request(
        &mut self,
        step: &mut Step,
        subject: &str,
        tool: &str,
        detail: &str,
        recap: bool,
    ) {
        let what = if detail.is_empty() {
            tool.to_string()
        } else {
            format!("{tool}: {detail}")
        };
        let key = format!("input_request:{subject}");
        if recap && self.seen.texts.contains_key(&key) {
            self.tally.repeated = self.tally.repeated.wrapping_add(1);
            return;
        }
        let rest = if recap {
            format!("{what} (reported denied by the terminal result)")
        } else {
            what
        };
        self.once(
            step,
            key,
            ActivityKind::InputRequested,
            identified("input_request", subject, &rest),
        );
    }

    /// Record what became of a refusal, after the reply was actually written or failed to be.
    /// A fate is never recorded before it is known.
    pub fn replied(&mut self, subject: &str, delivered: Result<(), String>) -> Step {
        let mut step = Step::default();
        let detail = match delivered {
            Ok(()) => format!(
                "refused permission request {subject}; the refusal was written to the provider"
            ),
            Err(error) => format!(
                "could not write the refusal of permission request {subject} to the provider ({})",
                public_text(&error, 80)
            ),
        };
        let entry = self.note("refusal", &detail);
        step.records.push(entry);
        step
    }

    fn session(&mut self, session: &Session, step: &mut Step) {
        let root = self.root.get_or_insert_with(|| session.id.clone()).clone();
        if root != session.id {
            let detail = format!(
                "a session {} initialized but this run is session {root}; its events are ignored",
                session.id
            );
            let entry = self.note("session-mismatch", &public_text(&detail, 200));
            step.records.push(entry);
            return;
        }
        self.initialized = true;
        let mut facts = Vec::new();
        for (name, value) in [
            ("model", &session.model),
            ("permission mode", &session.permission_mode),
            ("runtime", &session.version),
            ("credential source", &session.key_source),
        ] {
            if let Some(value) = value {
                facts.push(format!("{name} {value}"));
            }
        }
        facts.push(format!("{} tools", session.tool_count));
        self.once(
            step,
            format!("session:{}", session.id),
            ActivityKind::Progress,
            identified("session", &session.id, &facts.join(", ")),
        );
    }
}

/// Which conversation an event's line belongs to.
#[derive(Debug)]
struct Scope {
    session: Option<String>,
    subagent: bool,
}

#[cfg(test)]
mod tests;
