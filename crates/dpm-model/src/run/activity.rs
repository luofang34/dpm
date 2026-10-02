//! High-volume run activity: bounded telemetry kept apart from lifecycle facts.
//!
//! Every record carries the reporter's own per-run `source_sequence`, strictly increasing within a
//! run. That sequence is the idempotency identity and what keeps retries safe under bounded
//! retention: the store remembers only one high-water number per run, so a retry of a record that
//! retention already removed is recognised as expired instead of being recorded again as new.

use crate::{ActorId, RunId};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Longest activity text kept; longer text is truncated and flagged, and a digest of the whole
/// text stays with the record so a retry is still compared against everything that was sent.
pub const MAX_ACTIVITY_TEXT_BYTES: usize = 4096;

/// Most activity records one request may carry.
pub const MAX_ACTIVITY_BATCH: usize = 100;

/// What a reported activity record describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    /// A tool call began.
    ToolStarted,
    /// A tool call produced a result or failed.
    ToolResult,
    /// The executor reported public progress.
    Progress,
    /// The executor asked for input or approval.
    InputRequested,
    /// The executor is alive. A heartbeat is a receipt, nothing more.
    Heartbeat,
}

/// An activity kind word no record has.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "unknown activity kind {0:?}; expected tool_started, tool_result, progress, input_requested or heartbeat"
)]
pub struct ActivityKindParseError(String);

impl ActivityKind {
    /// The lowercase word every adapter spells the kind with.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::ToolStarted => "tool_started",
            Self::ToolResult => "tool_result",
            Self::Progress => "progress",
            Self::InputRequested => "input_requested",
            Self::Heartbeat => "heartbeat",
        }
    }
}

impl std::str::FromStr for ActivityKind {
    type Err = ActivityKindParseError;

    fn from_str(word: &str) -> Result<Self, Self::Err> {
        [
            Self::ToolStarted,
            Self::ToolResult,
            Self::Progress,
            Self::InputRequested,
            Self::Heartbeat,
        ]
        .into_iter()
        .find(|kind| kind.word() == word)
        .ok_or_else(|| ActivityKindParseError(word.into()))
    }
}

/// The SHA-256 of `text` as 64 lowercase hexadecimal digits.
fn sha256_hex(text: &str) -> String {
    Sha256::digest(text.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// What a caller asks to record as one activity record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityInput {
    /// The run the activity belongs to.
    pub run: RunId,
    /// The reporter's own sequence number for this record within the run, at least 1 and strictly
    /// increasing; gaps are allowed. It is the idempotency identity: resending a record with the
    /// same sequence and content is harmless while the record is retained, and a sequence at or
    /// below the run's high-water mark that is no longer retained is refused as expired.
    pub source_sequence: u64,
    /// What the record describes.
    pub kind: ActivityKind,
    /// Public text such as a tool name or progress note; never hidden reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// When the source says it happened. It is a claim, never used to judge freshness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
}

/// An activity record's text after the size bound, with whether it was cut and what it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedActivity {
    /// Text within [`MAX_ACTIVITY_TEXT_BYTES`].
    pub text: Option<String>,
    /// Whether the supplied text was longer and was cut at a character boundary.
    pub truncated: bool,
    /// SHA-256, in lowercase hex, of the whole supplied text; the identity of the text even when
    /// only a prefix is kept.
    pub text_digest: Option<String>,
}

impl ActivityInput {
    /// Apply the size bound and fingerprint the whole text, so a retry is compared against
    /// everything that was sent and not only the part that was kept.
    #[must_use]
    pub fn normalized(&self) -> NormalizedActivity {
        let Some(text) = &self.text else {
            return NormalizedActivity {
                text: None,
                truncated: false,
                text_digest: None,
            };
        };
        let text_digest = Some(sha256_hex(text));
        if text.len() <= MAX_ACTIVITY_TEXT_BYTES {
            return NormalizedActivity {
                text: Some(text.clone()),
                truncated: false,
                text_digest,
            };
        }
        let mut kept = String::new();
        for character in text.chars() {
            if kept.len().saturating_add(character.len_utf8()) > MAX_ACTIVITY_TEXT_BYTES {
                break;
            }
            kept.push(character);
        }
        NormalizedActivity {
            text: Some(kept),
            truncated: true,
            text_digest,
        }
    }
}

/// One recorded activity record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivityRecord {
    /// The run the activity belongs to.
    pub run: RunId,
    /// The reporter's sequence number within the run.
    pub source_sequence: u64,
    /// What the record describes.
    pub kind: ActivityKind,
    /// Public text, within the size bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Whether the reported text was cut to the size bound.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
    /// SHA-256 of the whole reported text, in lowercase hex.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text_digest: Option<String>,
    /// When the source says it happened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub observed_at: Option<DateTime<Utc>>,
    /// The principal that reported it.
    pub recorded_by: ActorId,
    /// When DPM received it, from DPM's own clock; freshness is judged from this alone.
    pub recorded_at: DateTime<Utc>,
}

/// An activity record addressed by its position in the activity feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityEntry {
    /// Feed cursor value, never reused.
    pub sequence: u64,
    /// The record.
    #[serde(flatten)]
    pub record: ActivityRecord,
}

/// Records the feed no longer holds because retention removed them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityGap {
    /// The cursor the client asked to continue from.
    pub requested_after: u64,
    /// The first sequence the feed can still serve.
    pub resumes_at: u64,
    /// Sequences between them, across every run in the feed, that are gone.
    pub lost: u64,
}

/// A bounded page of the activity feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityPage {
    /// Records in ascending feed order.
    pub entries: Vec<ActivityEntry>,
    /// Cursor for the next request; unchanged for an empty page without a gap.
    pub next_after_sequence: u64,
    /// Newest sequence ever assigned, so a client can tell it is caught up or was reset.
    pub head_sequence: u64,
    /// Present when the requested cursor precedes what retention kept; the client must treat the
    /// feed as discontinuous rather than as complete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<ActivityGap>,
}

/// What one run's activity amounts to, kept so retention cannot erase the fact of its silence or
/// the boundary that makes retries safe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActivityTally {
    /// Records ever received for the run. The count wraps at `u64::MAX`.
    pub recorded: u64,
    /// Records the feed still holds for it.
    pub retained: u64,
    /// The highest `source_sequence` accepted; zero before any activity. A record at or below it
    /// that is no longer retained is expired, never recorded again.
    pub source_high_water: u64,
    /// The latest record received, even when retention has since removed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest: Option<LatestActivity>,
}

/// The most recent activity record a run received.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LatestActivity {
    /// What it described.
    pub kind: ActivityKind,
    /// The reporter's sequence number for it.
    pub source_sequence: u64,
    /// When DPM received it.
    pub recorded_at: DateTime<Utc>,
}
