// The client's local state for following the boundary, and the rules it obeys throughout.
//
// Nothing advances until it has been applied. A page already applied is discarded by feed identity
// (lineage or epoch) and sequence, so delivering it twice changes nothing, and a page served under
// another identity is an explicit reset that clears what the client held for that feed.
//
// Staleness is tracked per store. The client remembers the furthest position it has seen of each
// store, from its seed and every poll. A refreshed set of views makes a store current only if every
// view in the set that carries it was anchored at or beyond that position: a response delayed past
// a change it lacks leaves the client stale, however many empty polls follow. A client seeded from
// several views is anchored at the earliest of them, so a change any held view lacks is delivered
// by the first poll and never skipped; views taken under different identities cannot be composed.
// A failed refresh or a dropped connection leaves the staleness visible to the next poll, and a
// project-only answer can never mark run views current. No rule of the domain lives here.

import Foundation

public enum ConsumerError: Error {
    /// Pages kept coming: the feeds are moving faster than a bounded drain can follow.
    case didNotCatchUp
}

/// What a project position belongs to: the workspace and the history within it. Two read-only
/// previews both lack a lineage, so the workspace tells them apart. The run store's epoch is a
/// different thing, the identity of the sidecar's history, and is never mixed with this.
struct ProjectIdentity: Equatable {
    let workspace: String
    let lineage: String?

    init(_ basis: ProjectWatermark) {
        workspace = basis.workspaceId
        lineage = basis.lineageId
    }
}

/// What has been seen of the run store: its epoch and how far each position had got.
struct RunPositions: Equatable {
    var epoch: String?
    var lifecycle: UInt64
    var activity: UInt64
    var links: UInt64

    init(_ heads: RunHeads) {
        epoch = heads.epoch
        lifecycle = heads.lifecycleHead
        activity = heads.activityHead
        links = heads.linkCount
    }

    /// Whether `anchor`, in the same epoch, is at or beyond everything seen.
    func coveredBy(_ anchor: RunPositions) -> Bool {
        epoch == anchor.epoch && anchor.lifecycle >= lifecycle && anchor.activity >= activity && anchor.links >= links
    }
}

/// The earliest of several project bases; `.conflict` when they were taken under different
/// histories and cannot be composed.
enum Earliest<T> {
    case none
    case conflict
    case value(T)
}

func earliestProject(_ bases: [ProjectWatermark]) -> Earliest<ProjectWatermark> {
    guard let first = bases.first else { return .none }
    if bases.contains(where: { ProjectIdentity($0) != ProjectIdentity(first) }) { return .conflict }
    return .value(bases.min(by: { $0.historyHead < $1.historyHead }) ?? first)
}

func earliestRuns(_ bases: [RunHeads]) -> Earliest<RunPositions> {
    guard let first = bases.first else { return .none }
    if bases.contains(where: { $0.epoch != first.epoch }) { return .conflict }
    var earliest = RunPositions(first)
    for basis in bases {
        let positions = RunPositions(basis)
        earliest.lifecycle = min(earliest.lifecycle, positions.lifecycle)
        earliest.activity = min(earliest.activity, positions.activity)
        earliest.links = min(earliest.links, positions.links)
    }
    return .value(earliest)
}

public final class Consumer {
    /// The most entries each retained list below keeps; see `retaining`.
    public static let defaultRetained = 256

    public var cursors = Cursors()
    public private(set) var projectStale = false
    public private(set) var runsStale = false
    // What follows is diagnostics, bounded to the most recent `retaining` entries each. Nothing the
    // consumer decides depends on it: duplicates are discarded by the last sequence applied, kept
    // separately, and cursors move only with pages that were applied. The pages themselves are what
    // `poll` returns to the caller, so an entry dropped from a list here was already handed over.
    /// Sequences of the project operations applied, most recent last.
    public private(set) var project: [UInt64] = []
    public private(set) var lifecycle: [UInt64] = []
    public private(set) var activity: [UInt64] = []
    /// The link count each time the token said the displayed runs must be read again.
    public private(set) var linkChanges: [UInt64] = []
    /// Resets the boundary signalled for a cursor it could not follow.
    public private(set) var resets: [String] = []
    /// Resets applied on seeing a page served under another feed identity.
    public private(set) var identityResets: [String] = []
    /// How many entries were applied in all, per feed, however few are retained.
    public private(set) var applied = (project: 0, lifecycle: 0, activity: 0)
    /// Entries discarded because their feed identity and sequence were already applied.
    public private(set) var discarded = 0
    /// Entries that carried no readable sequence; each one made its store stale.
    public private(set) var unreadable = 0
    /// How many entries each retained list keeps.
    public let retaining: Int
    private var last = (project: UInt64?.none, lifecycle: UInt64?.none, activity: UInt64?.none)
    private var projectIdentity: ProjectIdentity?
    private var runIdentity: String??
    private var observedProject: (identity: ProjectIdentity, head: UInt64)?
    private var observedRuns: RunPositions?

    public var dirty: Bool { projectStale || runsStale }

    /// Seed every feed from where an attach says it stands.
    public init(seed: Watermark, retaining: Int = Consumer.defaultRetained) {
        self.retaining = max(1, retaining)
        cursors = Cursors(
            project: ProjectCursor(lineageId: seed.project.lineageId, afterSequence: seed.project.historyHead),
            lifecycle: FeedCursor(epoch: seed.runs.epoch, afterSequence: seed.runs.lifecycleHead),
            activity: FeedCursor(epoch: seed.runs.epoch, afterSequence: seed.runs.activityHead),
            links: LinkMark(epoch: seed.runs.epoch, count: seed.runs.linkCount)
        )
        observedProject = (ProjectIdentity(seed.project), seed.project.historyHead)
        observedRuns = RunPositions(seed.runs)
    }

    /// Seed from the views the client holds, each store at the earliest basis among the views that
    /// carry it. A client with no run view follows no run feed.
    public init(views: [View], retaining: Int = Consumer.defaultRetained) {
        self.retaining = max(1, retaining)
        switch earliestProject(views.compactMap { $0.basis.project }) {
        case .value(let earliest):
            cursors.project = ProjectCursor(lineageId: earliest.lineageId, afterSequence: earliest.historyHead)
            observedProject = (ProjectIdentity(earliest), earliest.historyHead)
        case .conflict: projectStale = true
        case .none: break
        }
        switch earliestRuns(views.compactMap { $0.basis.runs }) {
        case .value(let earliest):
            cursors.lifecycle = FeedCursor(epoch: earliest.epoch, afterSequence: earliest.lifecycle)
            cursors.activity = FeedCursor(epoch: earliest.epoch, afterSequence: earliest.activity)
            cursors.links = LinkMark(epoch: earliest.epoch, count: earliest.links)
            observedRuns = earliest
        case .conflict: runsStale = true
        case .none: break
        }
    }

    /// Keep the most recent `retaining` entries of a list.
    private func keep<T>(_ list: inout [T], _ entry: T) {
        list.append(entry)
        if list.count > retaining { list.removeFirst(list.count - retaining) }
    }

    /// Apply the entries of a page whose sequence is beyond the last applied; returns how many were new.
    private func absorb(_ delta: Delta, into list: inout [UInt64], last: inout UInt64?, applied: inout Int) -> Int {
        var fresh = 0
        for entry in delta.entries {
            // An entry with no readable sequence cannot be placed or deduplicated. It is counted and
            // makes the store stale, so it is never silently skipped.
            guard let sequence = entry["sequence"].uint else {
                unreadable &+= 1
                fresh += 1
                continue
            }
            if let seen = last, sequence <= seen {
                discarded &+= 1
            } else {
                keep(&list, sequence)
                last = sequence
                applied &+= 1
                fresh += 1
            }
        }
        return fresh
    }

    /// A different identity than the one entries were applied under clears them, explicitly.
    private func checkIdentity(project identity: ProjectIdentity, epoch: String?) {
        if let known = projectIdentity, known != identity {
            project.removeAll()
            last.project = nil
            keep(&identityResets, "lineage_changed")
            projectStale = true
        }
        projectIdentity = identity
        if let known = runIdentity, known != epoch, known != nil {
            lifecycle.removeAll()
            activity.removeAll()
            last.lifecycle = nil
            last.activity = nil
            keep(&identityResets, "epoch_changed")
            runsStale = true
        }
        runIdentity = .some(epoch)
    }

    /// Remember how far each store has been seen to reach: the position at the end of a poll, which
    /// includes changes not yet delivered because a page was full.
    private func observe(_ watermark: Watermark) {
        let identity = ProjectIdentity(watermark.project)
        if let seen = observedProject, seen.identity == identity {
            observedProject = (identity, max(seen.head, watermark.project.historyHead))
        } else {
            observedProject = (identity, watermark.project.historyHead)
        }
        let now = RunPositions(watermark.runs)
        if var seen = observedRuns, seen.epoch == now.epoch {
            seen.lifecycle = max(seen.lifecycle, now.lifecycle)
            seen.activity = max(seen.activity, now.activity)
            seen.links = max(seen.links, now.links)
            observedRuns = seen
        } else {
            observedRuns = now
        }
    }

    /// Apply one answer. Applying the same answer again changes nothing.
    public func apply(_ changes: Changes) {
        let lineage = changes.watermark.project.lineageId
        let epoch = changes.watermark.runs.epoch
        checkIdentity(project: ProjectIdentity(changes.watermark.project), epoch: epoch)
        observe(changes.watermark)
        if let delta = changes.project {
            if delta.isReset {
                keep(&resets, delta.reason ?? "unknown")
                projectStale = true
            } else {
                projectStale = absorb(delta, into: &project, last: &last.project, applied: &applied.project) > 0 || projectStale
                let held = cursors.project.flatMap { $0.lineageId == lineage ? $0.afterSequence : nil } ?? 0
                cursors.project = ProjectCursor(lineageId: lineage, afterSequence: max(held, delta.nextAfterSequence))
            }
        }
        if let delta = changes.lifecycle {
            if delta.isReset {
                keep(&resets, delta.reason ?? "unknown")
                runsStale = true
            } else {
                runsStale = absorb(delta, into: &lifecycle, last: &last.lifecycle, applied: &applied.lifecycle) > 0 || runsStale
                let held = cursors.lifecycle.flatMap { $0.epoch == epoch ? $0.afterSequence : nil } ?? 0
                cursors.lifecycle = FeedCursor(epoch: epoch, afterSequence: max(held, delta.nextAfterSequence))
            }
        }
        if let delta = changes.activity {
            if delta.isReset {
                keep(&resets, delta.reason ?? "unknown")
                runsStale = true
            } else {
                let fresh = absorb(delta, into: &activity, last: &last.activity, applied: &applied.activity)
                runsStale = fresh > 0 || delta.gap != nil || runsStale
                let held = cursors.activity.flatMap { $0.epoch == epoch ? $0.afterSequence : nil } ?? 0
                cursors.activity = FeedCursor(epoch: epoch, afterSequence: max(held, delta.nextAfterSequence))
            }
        }
        if let signal = changes.links {
            if signal.isReset {
                keep(&resets, signal.reason ?? "unknown")
                runsStale = true
            } else if signal.changed {
                // Stale until views are installed; the mark is deliberately not adopted here.
                if linkChanges.last != signal.count { keep(&linkChanges, signal.count) }
                runsStale = true
            }
        }
    }

    /// One poll, applied the way a client must.
    @discardableResult
    public func poll(_ connection: NativeConnection, limit: Int = 100) async throws -> Changes {
        let changes = try await connection.changes(cursors, limit: limit)
        apply(changes)
        return changes
    }

    /// Poll until no feed reports more; returns the pages it took.
    @discardableResult
    public func drain(_ connection: NativeConnection, limit: Int = 100) async throws -> Int {
        for pages in 1...64 {
            let changes = try await poll(connection, limit: limit)
            let more = [changes.project?.more, changes.lifecycle?.more, changes.activity?.more].compactMap { $0 }
            if !more.contains(true) { return pages }
        }
        throw ConsumerError.didNotCatchUp
    }

    /// Install a refreshed set of displayed views. A store is current only if every view in the set
    /// that carries it was anchored at or beyond everything this client has seen of it. The installed
    /// set is what is displayed now, so an old response that arrives after a fresh one makes the
    /// display stale again rather than leaving it clean. The link mark is adopted only when the run
    /// views are current, from the earliest run basis installed.
    public func install(_ views: [View]) {
        switch earliestProject(views.compactMap({ $0.basis.project })) {
        case .value(let earliest):
            let covered = observedProject.map { $0.identity == ProjectIdentity(earliest) && earliest.historyHead >= $0.head } ?? true
            projectStale = !covered
        case .conflict: projectStale = true
        case .none: break
        }
        switch earliestRuns(views.compactMap({ $0.basis.runs })) {
        case .value(let earliest):
            let covered = observedRuns.map { $0.coveredBy(earliest) } ?? true
            runsStale = !covered
            if covered { cursors.links = LinkMark(epoch: earliest.epoch, count: earliest.links) }
        case .conflict: runsStale = true
        case .none: break
        }
    }

    /// Read the displayed views again, installing the set only if every call succeeded; a refusal
    /// is thrown and leaves the client exactly as stale as it was.
    @discardableResult
    public func refresh(_ connection: NativeConnection, queries: [JSON], attached: Attachment? = nil) async throws -> [View] {
        var views: [View] = []
        for query in queries { views.append(try await connection.query(query, attached: attached)) }
        install(views)
        return views
    }
}
