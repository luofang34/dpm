//! What run commands return.

use dpm_model::{ActivityEntry, LifecycleEntry, RunLink, RunView};
use serde::{Deserialize, Serialize};

/// The result of starting a run, reporting a transition or linking an operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunWrite {
    /// `true` when the identity was already recorded with this content: nothing was written and the
    /// recorded run is returned, so a retry is told from a first attempt.
    pub replayed: bool,
    /// The run after the write, with its derived observation.
    pub run: RunView,
    /// The lifecycle fact the command recorded or found already recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event: Option<LifecycleEntry>,
    /// The link the command recorded or found already recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<RunLink>,
}

/// One activity record the store holds for a request, and whether it was already there.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReceivedActivity {
    /// The record as stored, with its feed sequence.
    #[serde(flatten)]
    pub entry: ActivityEntry,
    /// `true` when this was a duplicate delivery and nothing was written for it.
    pub duplicate: bool,
}

/// The result of recording activity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityReceipt {
    /// One entry per request record, in request order.
    pub entries: Vec<ReceivedActivity>,
}
