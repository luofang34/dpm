//! Keeping the console current with changes committed by other processes.
//!
//! The loop probes the source's revision and lineage at most once per [`INTERVAL`] and reloads
//! only when the source moved forward on the displayed lineage. An older revision or another
//! lineage means the source no longer continues what is on screen: the snapshot stays, a notice
//! says so in text, and only the operator's explicit reload displays the other history.

use crate::{
    notice::ReloadNotice,
    source::{SnapshotSource, SourceRevision},
    view::View,
};
use std::time::{Duration, Instant};

/// Time between probes; the event loop wakes at least this often, so a change appears within
/// about one interval plus the reload.
pub(crate) const INTERVAL: Duration = Duration::from_secs(1);

/// How the probed source relates to the displayed snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    /// Nothing to reload.
    Current,
    /// Later revision of the displayed lineage; safe to display.
    Advanced,
    /// Older revision or another lineage; the operator decides.
    Diverged,
}

/// Compare a probed source revision with the displayed one.
pub(crate) fn classify(shown: SourceRevision, found: SourceRevision) -> Change {
    if found.lineage_id != shown.lineage_id || found.revision < shown.revision {
        Change::Diverged
    } else if found.revision > shown.revision {
        Change::Advanced
    } else {
        Change::Current
    }
}

/// What the last probe reported that is still on screen, so it is neither retried nor redrawn
/// every interval while the source stays the same.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Reported {
    /// The source at this revision diverged, or failed to reload.
    Source(SourceRevision),
    /// The probe itself failed with this text.
    Probe(String),
}

pub(crate) struct Watch {
    shown: SourceRevision,
    next_probe: Instant,
    reported: Option<Reported>,
}

impl Watch {
    pub(crate) fn new(shown: SourceRevision, now: Instant) -> Self {
        Self {
            shown,
            next_probe: now + INTERVAL,
            reported: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn shown(&self) -> SourceRevision {
        self.shown
    }

    /// Probe the source when due and follow a forward change; report anything else.
    pub(crate) fn poll_blocking<S: SnapshotSource>(
        &mut self,
        view: &mut View,
        source: &mut S,
        now: Instant,
    ) {
        if now < self.next_probe {
            return;
        }
        self.next_probe = now + INTERVAL;
        let found = match source.revision_blocking() {
            Ok(found) => found,
            Err(error) => {
                let text = error.to_string();
                let reported = Some(Reported::Probe(text.clone()));
                if self.reported != reported {
                    view.set_notice(Some(ReloadNotice::unchecked(&text, self.shown.revision)));
                    self.reported = reported;
                }
                return;
            }
        };
        match classify(self.shown, found) {
            Change::Current => {
                // The source is back where the display is, so an earlier report no longer holds.
                if self.reported.take().is_some() {
                    view.set_notice(None);
                }
            }
            _ if self.reported == Some(Reported::Source(found)) => {}
            Change::Advanced => self.reload_blocking(view, source, Some(found)),
            Change::Diverged => self.report(view, found),
        }
    }

    /// The operator's explicit reload: display whatever the source holds now, whichever history.
    pub(crate) fn accept_blocking<S: SnapshotSource>(&mut self, view: &mut View, source: &mut S) {
        self.reload_blocking(view, source, None);
    }

    /// Load and display a snapshot. A reload the loop started (`probed`) displays only a forward
    /// change: the source can diverge between the probe and the load.
    fn reload_blocking<S: SnapshotSource>(
        &mut self,
        view: &mut View,
        source: &mut S,
        probed: Option<SourceRevision>,
    ) {
        let snapshot = match source.snapshot_blocking() {
            Ok(snapshot) => snapshot,
            Err(error) => return self.failed(view, &error, probed),
        };
        let loaded = snapshot.revision();
        if probed.is_some() && classify(self.shown, loaded) == Change::Diverged {
            return self.report(view, loaded);
        }
        match view.refresh(&snapshot.plan, chrono::Utc::now()) {
            Ok(()) => {
                self.shown = loaded;
                self.reported = None;
            }
            Err(error) => self.failed(view, &error, probed.map(|_| loaded)),
        }
    }

    fn failed(
        &mut self,
        view: &mut View,
        error: &impl std::fmt::Display,
        probed: Option<SourceRevision>,
    ) {
        view.reload_failed(error);
        if let Some(found) = probed {
            self.reported = Some(Reported::Source(found));
        }
    }

    fn report(&mut self, view: &mut View, found: SourceRevision) {
        view.set_notice(Some(ReloadNotice::diverged(self.shown, found)));
        self.reported = Some(Reported::Source(found));
    }
}

#[cfg(test)]
mod tests;
