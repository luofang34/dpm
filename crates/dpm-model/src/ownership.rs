//! Who held a task, and when: the append-only records independent review is judged against.

use crate::{ActorId, WorkItem};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;

/// Authorized transfer of claimed or started work between owners.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Handoff {
    /// Owner the work was taken from.
    pub from: ActorId,
    /// Owner the work was given to.
    pub to: ActorId,
    /// Human or service that authorized the transfer.
    pub actor: ActorId,
    /// Caller-supplied UTC operation time.
    pub at: DateTime<Utc>,
    /// Why the executor changed.
    pub reason: String,
}

/// An owner giving back an unstarted claim.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimRelease {
    /// Owner that gave the claim back.
    pub actor: ActorId,
    /// Caller-supplied UTC operation time.
    pub at: DateTime<Utc>,
    /// Why the claim was given back.
    pub reason: String,
}

/// The record through which an actor held a task at or before some time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holding {
    /// Owned it then, and owns it now or until no later handoff.
    Owner,
    /// Owned it then and handed it on at this later time.
    OwnerUntil(DateTime<Utc>),
    /// Received it by the handoff at this time.
    HandedTo(DateTime<Utc>),
    /// Gave it away by the handoff at this time.
    HandedFrom(DateTime<Utc>),
    /// Gave back a claim on it at this time.
    Released(DateTime<Utc>),
}

impl fmt::Display for Holding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Owner => write!(f, "owned the task at that time"),
            Self::OwnerUntil(at) => write!(f, "owned the task then, until the handoff at {at}"),
            Self::HandedTo(at) => write!(f, "received the task by the handoff at {at}"),
            Self::HandedFrom(at) => write!(f, "held the task until the handoff at {at}"),
            Self::Released(at) => write!(f, "released its claim on the task at {at}"),
        }
    }
}

impl WorkItem {
    /// Whether the actor owns this work now or held it before a handoff or release.
    ///
    /// Independent review reads this rather than the current owner alone, so handing work away
    /// or releasing a claim never turns an actor that held it into its reviewer.
    #[must_use]
    pub fn held_by(&self, actor: &ActorId) -> bool {
        self.owner.as_ref() == Some(actor)
            || self.handoffs.iter().any(|h| h.from == *actor)
            || self.releases.iter().any(|r| r.actor == *actor)
    }

    /// How the actor held this work at or before `at`, if it did.
    ///
    /// A recorded review stays independent after its reviewer later takes the work over, so the
    /// records are judged at their own time. Only started work is reviewed and a release only
    /// ends an unstarted claim, so the current owner with no later handoff has held the work
    /// since before any review. A handoff at the review instant counts as preceding it, because
    /// equal times cannot be ordered and independence must not be assumed.
    #[must_use]
    pub fn holding_at(&self, actor: &ActorId, at: DateTime<Utc>) -> Option<Holding> {
        let earlier = self.handoffs.iter().filter(|h| h.at <= at);
        if let Some(h) = earlier.clone().find(|h| h.to == *actor) {
            return Some(Holding::HandedTo(h.at));
        }
        if let Some(h) = earlier.clone().find(|h| h.from == *actor) {
            return Some(Holding::HandedFrom(h.at));
        }
        if let Some(r) = self
            .releases
            .iter()
            .find(|r| r.at <= at && r.actor == *actor)
        {
            return Some(Holding::Released(r.at));
        }
        match self.handoffs.iter().find(|h| h.at > at) {
            Some(later) if later.from == *actor => Some(Holding::OwnerUntil(later.at)),
            None if self.owner.as_ref() == Some(actor) => Some(Holding::Owner),
            _ => None,
        }
    }
}
