//! The client side of the worked sequences: a consumer that follows the rules the boundary
//! documents, and the fixtures the sequences share.

use super::*;
use dpm_model::LineageId;

pub(super) use super::fixtures::*;

/// A client's local state: three independent cursors, the link mark, what it has applied from each
/// feed, and which of the views it displays are known to be stale.
///
/// Nothing advances until it has been applied. A page already applied is discarded by feed
/// identity and sequence, so delivering it twice changes nothing; an identity change is an
/// explicit reset that clears what was held for that feed. Staleness is tracked per store and
/// cleared only by installing a refreshed set of views that carries that store's basis, so a
/// project-only answer can never mark run views current.
pub(super) struct Consumer {
    pub(super) cursors: Cursors,
    project_stale: bool,
    runs_stale: bool,
    pub(super) project: Vec<u64>,
    pub(super) lifecycle: Vec<u64>,
    pub(super) activity: Vec<u64>,
    /// The link count each time the token said the displayed runs must be read again.
    pub(super) links: Vec<u64>,
    /// Resets the boundary signalled for a cursor it could not follow.
    pub(super) resets: Vec<ResetReason>,
    /// Resets the client itself applied on seeing a page served under another feed identity.
    pub(super) identity_resets: Vec<ResetReason>,
    /// Entries discarded because their feed identity and sequence were already applied.
    pub(super) discarded: usize,
    /// The identity each feed's applied entries belong to.
    project_identity: Option<ProjectIdentity>,
    run_identity: Option<Option<LineageId>>,
    /// The furthest position of each store this client has seen, from its seed and every poll. A
    /// view makes the client current only if it was anchored at or beyond it.
    observed_project: Option<(ProjectIdentity, u64)>,
    observed_runs: Option<Observed>,
}

/// What a project position belongs to: the workspace and the history within it. Two read-only
/// previews both lack a lineage, so the workspace tells them apart. The run store's epoch is a
/// different thing, the identity of the sidecar's history, and is never mixed with this.
type ProjectIdentity = (dpm_model::WorkspaceId, Option<LineageId>);

fn identity_of(basis: &ProjectWatermark) -> ProjectIdentity {
    (basis.workspace_id, basis.lineage_id)
}

/// What has been seen of the run store: its epoch and how far each of its positions had got.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Observed {
    epoch: Option<LineageId>,
    lifecycle: u64,
    activity: u64,
    links: u64,
}

impl Observed {
    fn of(heads: &dpm_model::RunFeedHeads) -> Self {
        Self {
            epoch: heads.epoch,
            lifecycle: heads.lifecycle_head,
            activity: heads.activity_head,
            links: heads.link_count,
        }
    }

    /// Whether `anchor`, in the same epoch, is at or beyond everything seen.
    fn covered_by(&self, anchor: &Self) -> bool {
        self.epoch == anchor.epoch
            && anchor.lifecycle >= self.lifecycle
            && anchor.activity >= self.activity
            && anchor.links >= self.links
    }
}

impl Consumer {
    fn empty(cursors: Cursors) -> Self {
        Self {
            cursors,
            project_stale: false,
            runs_stale: false,
            project: Vec::new(),
            lifecycle: Vec::new(),
            activity: Vec::new(),
            links: Vec::new(),
            resets: Vec::new(),
            identity_resets: Vec::new(),
            discarded: 0,
            project_identity: None,
            run_identity: None,
            observed_project: None,
            observed_runs: None,
        }
    }

    /// Seed every feed from where an attach says it stands.
    pub(super) fn seeded(watermark: &Watermark) -> Self {
        let mut seeded = Self::empty(Cursors {
            project: Some(ProjectCursor {
                lineage_id: watermark.project.lineage_id,
                after_sequence: watermark.project.history_head,
            }),
            lifecycle: Some(FeedCursor {
                epoch: watermark.runs.epoch,
                after_sequence: watermark.runs.lifecycle_head,
            }),
            activity: Some(FeedCursor {
                epoch: watermark.runs.epoch,
                after_sequence: watermark.runs.activity_head,
            }),
            links: Some(LinkMark {
                epoch: watermark.runs.epoch,
                count: watermark.runs.link_count,
            }),
        });
        seeded.observed_project = Some((
            identity_of(&watermark.project),
            watermark.project.history_head,
        ));
        seeded.observed_runs = Some(Observed::of(&watermark.runs));
        seeded
    }

    /// Seed from the views the client holds. Each store is anchored at the earliest basis among the
    /// views that carry it, so a change that any held view lacks is delivered by the first poll
    /// and never skipped; views anchored under different identities cannot be composed, so that
    /// store is seeded from nothing and marked stale. A client with no run view follows no run
    /// feed.
    pub(super) fn from_views(held: &[&View]) -> Self {
        let mut client = Self::empty(Cursors::default());
        let projects: Vec<ProjectWatermark> =
            held.iter().filter_map(|view| view.basis.project).collect();
        match earliest_project(&projects) {
            Some(Some(earliest)) => {
                client.cursors.project = Some(ProjectCursor {
                    lineage_id: earliest.lineage_id,
                    after_sequence: earliest.history_head,
                });
                client.observed_project = Some((identity_of(&earliest), earliest.history_head));
            }
            Some(None) => client.project_stale = true,
            None => {}
        }
        let runs: Vec<dpm_model::RunFeedHeads> =
            held.iter().filter_map(|view| view.basis.runs).collect();
        match earliest_runs(&runs) {
            Some(Some(earliest)) => {
                client.cursors.lifecycle = Some(FeedCursor {
                    epoch: earliest.epoch,
                    after_sequence: earliest.lifecycle,
                });
                client.cursors.activity = Some(FeedCursor {
                    epoch: earliest.epoch,
                    after_sequence: earliest.activity,
                });
                client.cursors.links = Some(LinkMark {
                    epoch: earliest.epoch,
                    count: earliest.links,
                });
                client.observed_runs = Some(earliest);
            }
            Some(None) => client.runs_stale = true,
            None => {}
        }
        client
    }

    /// Whether any displayed view is known to be stale.
    pub(super) fn dirty(&self) -> bool {
        self.project_stale || self.runs_stale
    }

    pub(super) fn project_stale(&self) -> bool {
        self.project_stale
    }

    pub(super) fn runs_stale(&self) -> bool {
        self.runs_stale
    }

    /// Append the sequences not yet applied, counting repeats; returns how many were new.
    fn absorb(&mut self, feed: Feed, sequences: impl Iterator<Item = u64>) -> usize {
        let seen = match feed {
            Feed::Project => &mut self.project,
            Feed::Lifecycle => &mut self.lifecycle,
            Feed::Activity => &mut self.activity,
        };
        let mut fresh = 0;
        for sequence in sequences {
            if seen.last().is_some_and(|last| sequence <= *last) {
                self.discarded += 1;
            } else {
                seen.push(sequence);
                fresh += 1;
            }
        }
        fresh
    }

    /// The identity a page was served under; a different one than was applied before is an
    /// explicit reset that clears what the client held for the feeds under it.
    fn check_identity(&mut self, project: ProjectIdentity, epoch: Option<LineageId>) {
        if self.project_identity.is_some_and(|known| known != project) {
            self.project.clear();
            self.identity_resets.push(ResetReason::LineageChanged);
            self.project_stale = true;
        }
        self.project_identity = Some(project);
        let run_changed = self
            .run_identity
            .is_some_and(|known| known != epoch && known.is_some());
        if run_changed {
            self.lifecycle.clear();
            self.activity.clear();
            self.identity_resets.push(ResetReason::EpochChanged);
            self.runs_stale = true;
        }
        self.run_identity = Some(epoch);
    }

    /// Apply one answer. Applying the same answer again changes nothing.
    pub(super) fn apply(&mut self, result: &ChangesResult) {
        let lineage = result.watermark.project.lineage_id;
        let epoch = result.watermark.runs.epoch;
        self.check_identity(identity_of(&result.watermark.project), epoch);
        self.observe(&result.watermark);
        if let Some(delta) = &result.project {
            match delta.status {
                FeedStatus::Continue => {
                    let fresh =
                        self.absorb(Feed::Project, delta.entries.iter().map(|e| e.sequence));
                    self.project_stale |= fresh > 0;
                    advance_project(&mut self.cursors, lineage, delta.next_after_sequence);
                }
                FeedStatus::Reset { reason } => self.reset(reason, true),
            }
        }
        if let Some(delta) = &result.lifecycle {
            match delta.status {
                FeedStatus::Continue => {
                    let fresh =
                        self.absorb(Feed::Lifecycle, delta.entries.iter().map(|e| e.sequence));
                    self.runs_stale |= fresh > 0;
                    advance_feed(
                        &mut self.cursors.lifecycle,
                        epoch,
                        delta.next_after_sequence,
                    );
                }
                FeedStatus::Reset { reason } => self.reset(reason, false),
            }
        }
        if let Some(delta) = &result.activity {
            match delta.status {
                FeedStatus::Continue => {
                    let fresh =
                        self.absorb(Feed::Activity, delta.entries.iter().map(|e| e.sequence));
                    self.runs_stale |= fresh > 0 || delta.gap.is_some();
                    advance_feed(&mut self.cursors.activity, epoch, delta.next_after_sequence);
                }
                FeedStatus::Reset { reason } => self.reset(reason, false),
            }
        }
        if let Some(signal) = &result.links {
            match signal.status {
                FeedStatus::Continue if signal.changed => {
                    // Stale until views are installed; the mark is deliberately not adopted here.
                    if self.links.last() != Some(&signal.count) {
                        self.links.push(signal.count);
                    }
                    self.runs_stale = true;
                }
                FeedStatus::Continue => {}
                FeedStatus::Reset { reason } => self.reset(reason, false),
            }
        }
    }

    /// A cursor the boundary could not follow: what was held for it is stale. `project` says
    /// whether it was the project feed or one of the run feeds.
    /// Remember how far each store has been seen to reach: the position at the end of a poll,
    /// which includes changes not yet delivered because a page was full.
    fn observe(&mut self, watermark: &Watermark) {
        let identity = identity_of(&watermark.project);
        let head = watermark.project.history_head;
        self.observed_project = Some(match self.observed_project {
            Some((known, seen)) if known == identity => (identity, seen.max(head)),
            _ => (identity, head),
        });
        let now = Observed::of(&watermark.runs);
        self.observed_runs = Some(match self.observed_runs {
            Some(seen) if seen.epoch == now.epoch => Observed {
                epoch: now.epoch,
                lifecycle: seen.lifecycle.max(now.lifecycle),
                activity: seen.activity.max(now.activity),
                links: seen.links.max(now.links),
            },
            _ => now,
        });
    }

    fn reset(&mut self, reason: ResetReason, project: bool) {
        self.resets.push(reason);
        if project {
            self.project_stale = true;
        } else {
            self.runs_stale = true;
        }
    }

    /// One poll, applied the way a client must.
    pub(super) fn poll_blocking(&mut self, app: &mut Application, limit: u16) -> ChangesResult {
        let result = changes_blocking(app, self.cursors, Some(limit));
        self.apply(&result);
        result
    }

    /// Poll until no feed reports more, returning how many pages that took.
    pub(super) fn drain_blocking(&mut self, app: &mut Application, limit: u16) -> usize {
        for pages in 1..=64 {
            let result = self.poll_blocking(app, limit);
            let more = [
                result.project.as_ref().map(|d| d.more),
                result.lifecycle.as_ref().map(|d| d.more),
                result.activity.as_ref().map(|d| d.more),
            ];
            if !more.into_iter().flatten().any(|more| more) {
                return pages;
            }
        }
        panic!("a consumer did not catch up");
    }

    /// Install a refreshed set of displayed views. A store becomes current only if every view in the
    /// set that carries it was anchored at or beyond everything this client has seen of it: a
    /// response that was delayed past a change it lacks leaves the client stale, however many empty
    /// polls follow. The link mark is adopted only then, from the earliest run basis installed.
    pub(super) fn install(&mut self, views: &[View]) {
        let projects: Vec<ProjectWatermark> =
            views.iter().filter_map(|view| view.basis.project).collect();
        match earliest_project(&projects) {
            Some(Some(earliest)) => {
                let covered = self.observed_project.is_none_or(|(identity, seen)| {
                    identity == identity_of(&earliest) && earliest.history_head >= seen
                });
                // The installed set is what is displayed now, so an old response that arrives
                // after a fresh one makes the display stale again rather than leaving it clean.
                self.project_stale = !covered;
            }
            Some(None) => self.project_stale = true,
            None => {}
        }
        let runs: Vec<dpm_model::RunFeedHeads> =
            views.iter().filter_map(|view| view.basis.runs).collect();
        match earliest_runs(&runs) {
            Some(Some(earliest)) => {
                let covered = self
                    .observed_runs
                    .is_none_or(|seen| seen.covered_by(&earliest));
                self.runs_stale = !covered;
                if covered {
                    self.cursors.links = Some(LinkMark {
                        epoch: earliest.epoch,
                        count: earliest.links,
                    });
                }
            }
            Some(None) => self.runs_stale = true,
            None => {}
        }
    }

    /// Read the displayed views again, installing the set only if every call succeeded; a refusal
    /// is returned as its code and leaves the client exactly as stale as it was.
    pub(super) fn refresh_all_blocking(
        &mut self,
        app: &mut Application,
        queries: &[Value],
        attached: Option<Attachment>,
    ) -> Result<Vec<View>, String> {
        let mut views = Vec::new();
        for query in queries {
            let response = exchange_blocking(
                app,
                json!({"type": "query", "query": query, "attached": attached}),
            );
            match serde_json::from_value::<NativeResponse>(response).expect("typed response") {
                NativeResponse {
                    result: Some(NativeResult::View(view)),
                    ..
                } => views.push(view),
                NativeResponse {
                    error: Some(error), ..
                } => return Err(error.code),
                other => panic!("neither a view nor a refusal: {other:?}"),
            }
        }
        self.install(&views);
        Ok(views)
    }

    /// Refresh one displayed view.
    pub(super) fn refresh_blocking(
        &mut self,
        app: &mut Application,
        query: Value,
        attached: Option<Attachment>,
    ) -> Result<View, String> {
        self.refresh_all_blocking(app, &[query], attached)
            .map(|mut views| views.remove(0))
    }
}

/// The earliest project basis among those given, or `Some(None)` when they were taken under
/// different histories and cannot be composed; `None` when there are none.
fn earliest_project(bases: &[ProjectWatermark]) -> Option<Option<ProjectWatermark>> {
    let first = bases.first()?;
    if bases
        .iter()
        .any(|basis| identity_of(basis) != identity_of(first))
    {
        return Some(None);
    }
    bases
        .iter()
        .min_by_key(|basis| basis.history_head)
        .copied()
        .map(Some)
}

/// The earliest run basis among those given, position by position, under one epoch.
fn earliest_runs(bases: &[dpm_model::RunFeedHeads]) -> Option<Option<Observed>> {
    let first = bases.first()?;
    if bases.iter().any(|basis| basis.epoch != first.epoch) {
        return Some(None);
    }
    let all: Vec<Observed> = bases.iter().map(Observed::of).collect();
    Some(Some(Observed {
        epoch: first.epoch,
        lifecycle: all.iter().map(|o| o.lifecycle).min().unwrap_or(0),
        activity: all.iter().map(|o| o.activity).min().unwrap_or(0),
        links: all.iter().map(|o| o.links).min().unwrap_or(0),
    }))
}

#[derive(Clone, Copy)]
enum Feed {
    Project,
    Lifecycle,
    Activity,
}

/// Move a project cursor forward under its identity, never back: a stale page cannot rewind it.
fn advance_project(cursors: &mut Cursors, lineage: Option<LineageId>, next: u64) {
    let current = cursors
        .project
        .filter(|cursor| cursor.lineage_id == lineage)
        .map_or(0, |cursor| cursor.after_sequence);
    cursors.project = Some(ProjectCursor {
        lineage_id: lineage,
        after_sequence: current.max(next),
    });
}

fn advance_feed(cursor: &mut Option<FeedCursor>, epoch: Option<LineageId>, next: u64) {
    let current = cursor
        .filter(|held| held.epoch == epoch)
        .map_or(0, |held| held.after_sequence);
    *cursor = Some(FeedCursor {
        epoch,
        after_sequence: current.max(next),
    });
}
