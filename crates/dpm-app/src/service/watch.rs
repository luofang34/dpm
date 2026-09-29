//! In-process notification of committed operations, for native clients that redraw on change.
//!
//! Only commits made through this [`Application`] are reported. Another process, or another
//! `Application` on the same store, is observed by polling [`Application::revision_blocking`].

use super::{Application, WorkspaceRevision};
use std::sync::mpsc::{Receiver, Sender, channel};

#[derive(Default)]
pub(super) struct Watchers(Vec<Sender<WorkspaceRevision>>);

impl Watchers {
    /// Report one commit to every receiver still listening.
    #[cfg(feature = "sqlite")]
    pub(super) fn notify(&mut self, revision: WorkspaceRevision) {
        // A dropped receiver has stopped watching; its sender is discarded rather than kept.
        self.0.retain(|sender| sender.send(revision).is_ok());
    }
}

impl Application {
    /// Receive the revision and lineage of every operation this application commits from now on.
    ///
    /// Each committed command or plan change sends exactly one message after its transaction
    /// commits; refused mutations and resends answered with an already recorded operation send
    /// none. The receiver may be moved to another thread; dropping it stops the notifications.
    /// Messages queue until received, so a subscriber drains the channel (a view needs only the
    /// last revision it holds) or drops it.
    pub fn watch_commits(&mut self) -> Receiver<WorkspaceRevision> {
        let (sender, receiver) = channel();
        self.watchers.0.push(sender);
        receiver
    }
}

#[cfg(test)]
#[cfg(feature = "sqlite")]
mod tests;
