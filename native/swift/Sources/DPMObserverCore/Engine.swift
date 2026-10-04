// The observer's engine: one actor that owns the connection, decides when to read again and
// publishes immutable snapshots. It is the only place a blocking step is started, and every one of
// them runs on the bridge's own queues, never on the main thread.
//
// Reads are driven by the boundary's feeds, not by a fixed re-query. Each pass polls `changes` from
// the consumer's cursors; run activity is appended to the selected run's bounded window without
// reading anything else; a project change reads the project views once; run changes read the runs
// at most once per interval; and only the application's own `refresh_at` instant, or its
// re-evaluation bound, reads the time-dependent views again at an unchanged revision.
//
// What a change obliges the engine to read again is kept in `owed` until the matching read has
// succeeded and been installed, so a refused or failed read is retried by the next pass even when
// the feed has nothing more to say. Staleness is published before any read is awaited, so a newer
// answer is never shown beside older ones marked current.
//
// Every connection has a generation, taken before the first suspension of the operation that opens
// or closes it. An answer that arrives after the connection it was asked of was closed or replaced
// is dropped before it can touch the state, so a delayed response can never be attached to a new
// source. Nothing is queued without bound: a selection is one value and a wake-up is one flag. The
// engine has no command call: it can only read.

import DPMNative
import Foundation

public actor ObserverEngine {
    public struct Settings: Sendable {
        public var helper: URL
        /// Pins the helper's clock for the whole session; nil follows the system clock.
        public var pinnedClock: Instant?
        /// How often the feeds are polled; the boundary is polled from cursors, never pushed.
        public var pollInterval: TimeInterval = 0.4
        public var pageLimit = 500
        /// Pages read in one pass before the rest is left for the next, so a burst cannot hold a pass.
        public var maxPagesPerPass = 8
        public var runsRefreshInterval: TimeInterval = 1
        /// Most activity entries kept for the selected run, and most lifecycle entries.
        public var windowLimit = 400
        public var lifecycleLimit = 200
        /// How a run's history is paged when it is selected: the size of a page and how many pages one
        /// reading takes at most. A run with more than they cover is said to be only partly read.
        public var lifecyclePageSize = 500
        public var lifecyclePages = 16
        public var activityPageSize = 1000
        public var activityPages = 20
        public var runLimit = 200
        public var operationLimit = 100
        public var reconnectDelays: [TimeInterval] = [0.5, 1, 2, 5]
        public var shutdownBound: TimeInterval = 5
        /// How long a replacement helper must have worked before a later failure counts as new.
        public var stableAfter: TimeInterval = 5
        public var onBlockingStep: (@Sendable (String) -> Void)?
        /// Test seams, nil in the app. `fault` may fail a query before it is sent, and `intercept`
        /// may deliver a feed answer more than once or annotate it; the engine reads and applies
        /// whatever they return exactly as it would an answer from the helper.
        public var fault: (@Sendable (JSON) -> Error?)?
        public var intercept: (@Sendable (Changes) -> [Changes])?

        public init(helper: URL) { self.helper = helper }
    }

    /// The reads a change obliges, until each has succeeded.
    struct Owed {
        enum Cause { case project, time }
        var status = false
        var inventory = false
        var runs = false
        var subject = false
        /// The selected task's own runs are read again.
        var workRuns = false
        /// The selected run's window is read again, not only its view.
        var window = false
        /// The schedule projection is read again; owed only while the Gantt is shown.
        var schedule = false
        var cause = Cause.time

        static var project: Owed { Owed(status: true, inventory: true, runs: true, subject: true, cause: .project) }
        static var runChange: Owed { Owed(runs: true) }
        static var clock: Owed { Owed(status: true, runs: true, subject: true, cause: .time) }
        static var everything: Owed { Owed(status: true, inventory: true, runs: true, subject: true, window: true, cause: .project) }

        var any: Bool { status || inventory || runs || subject || workRuns || schedule }

        mutating func merge(_ other: Owed) {
            status = status || other.status
            inventory = inventory || other.inventory
            runs = runs || other.runs
            subject = subject || other.subject
            workRuns = workRuns || other.workRuns
            window = window || other.window
            schedule = schedule || other.schedule
            if other.cause == .project && (other.status || other.inventory) { cause = .project }
        }
    }

    enum Outcome { case `continue`, stop }

    let settings: Settings
    let publisher: @Sendable (ObserverSnapshot) -> Void
    var snapshot = ObserverSnapshot()
    var generation = 0
    var connection: NativeConnection?
    var consumer: Consumer?
    var selection: WorkspaceSelection?
    var follower: Task<Void, Never>?
    /// The one open in progress, if any: ended at once when it is superseded or closed, so a helper
    /// that is still starting is not left to finish on its own.
    var opening: Task<NativeConnection, Error>?
    /// The ending of everything an earlier open, close or replacement gave up: each one waits for the
    /// one before it, so a new helper is started only after every earlier one has been ended.
    var cleanup: Task<Void, Never>?
    /// The views on display, by slot: the set the consumer judges freshness against.
    var displayed: [String: View] = [:]
    var subject: Subject?
    /// Whether the Gantt is the page shown: only then is the schedule projection read and displayed.
    var wantsSchedule = false
    var owed = Owed()
    /// Consecutive failed attempts to keep a working helper, across episodes: the delay between
    /// replacements grows with it, and only a pass that completes resets it.
    var failureStreak = 0
    var reconnectedAt = Date.distantPast
    var runsDeferredUntil: Date?
    var lastRunsRead = Date.distantPast
    var lastTimeRead = Date.distantPast
    var sleeper: CheckedContinuation<Void, Never>?
    var sleepToken = 0
    var wakePending = false

    public init(settings: Settings, publish: @escaping @Sendable (ObserverSnapshot) -> Void) {
        self.settings = settings
        publisher = publish
    }

    // MARK: Public operations

    public var current: ObserverSnapshot { snapshot }

    /// Returns once the follower has ended: after a source change, a fatal failure, a close or a
    /// replacement. An event to wait for instead of a time to wait out.
    public func awaitFollower() async { await follower?.value }

    /// The helper's process identifier, for diagnostics and fault injection.
    public var helperProcessIdentifier: Int32? { connection?.helperProcessIdentifier }

    /// Open a workspace the caller chose. Whatever was open is ended first; nothing is created.
    /// The generation is taken before the first suspension, so an older open that resumes later
    /// finds itself superseded and ends what it opened instead of publishing it.
    public func open(_ chosen: WorkspaceSelection) async {
        generation &+= 1
        let mine = generation
        let ending = detach()
        selection = chosen
        snapshot = ObserverSnapshot()
        snapshot.generation = mine
        snapshot.connection = .opening
        publish()
        // Not past this line until every earlier helper, a published one or one still in its
        // handshake, has been ended, so superseding opens cannot pile helpers up.
        await ending.value
        guard generation == mine else { return }
        var configuration = Configuration(helper: settings.helper, selection: chosen)
        configuration.pinnedClock = settings.pinnedClock
        configuration.shutdownBound = settings.shutdownBound
        configuration.onBlockingStep = settings.onBlockingStep
        let attempt = Task.detached { try await NativeConnection.open(configuration) }
        opening = attempt
        let opened: NativeConnection
        do {
            opened = try await attempt.value
        } catch {
            guard generation == mine else { return }
            opening = nil
            snapshot.connection = .failed(Self.describe(error))
            publish()
            return
        }
        guard generation == mine else {
            await Self.end(opened)
            return
        }
        opening = nil
        connection = opened
        do {
            try await load(opened, mine: mine)
        } catch {
            guard generation == mine else { return }
            snapshot.connection = .failed(Self.describe(error))
            publish()
            return
        }
        guard generation == mine else { return }
        snapshot.connection = .connected
        publish()
        follower = Task { await self.followLoop(mine) }
    }

    /// End the connection. Nothing is sent to the project and no ownership is released: closing a
    /// window is not a verdict on anything. The state is final before the helper is waited for, so
    /// a newer open can never be overwritten by this one finishing late.
    public func close() async {
        generation &+= 1
        let ending = detach()
        selection = nil
        snapshot.connection = .closed
        snapshot.generation = generation
        publish()
        await ending.value
    }

    /// Read everything again now; from a lost or changed source, open it again.
    public func reload() async {
        switch snapshot.connection {
        case .sourceChanged, .failed, .closed:
            if let chosen = selection { await open(chosen) }
        default:
            owed.merge(.everything)
            wake()
        }
    }

    /// Select by persistent identity. One value is kept, so repeated selections coalesce to the
    /// latest; the follower reads it at once. The previous selection's view leaves the displayed
    /// set immediately, so a view no longer shown can neither hold the client stale nor schedule
    /// reads of its own.
    public func select(_ chosen: Subject?) {
        guard chosen != subject else { return }
        subject = chosen
        // Everything the previous selection owned leaves the displayed set and the debts with it, so
        // a view that is no longer shown can neither hold the client stale nor schedule a read.
        displayed["subject"] = nil
        displayed["workRuns"] = nil
        snapshot.window = nil
        snapshot.detail = chosen.map { Self.placeholder($0, inventory: snapshot.inventory) }
        owed.subject = chosen != nil
        owed.window = chosen != nil
        owed.workRuns = false
        snapshot.workRuns = nil
        snapshot.runDetail = nil
        if let consumer = consumer {
            consumer.install(Array(displayed.values))
            syncFreshness()
        }
        publish()
        wake()
    }

    /// Say whether the Gantt is shown. Shown, its schedule projection is read and displayed like the
    /// other views, so it is judged for freshness and re-read with them; not shown, it leaves the
    /// displayed set at once, so a view nobody sees can neither hold the client stale nor schedule a read.
    public func showSchedule(_ shown: Bool) {
        guard shown != wantsSchedule else { return }
        wantsSchedule = shown
        if shown {
            owed.schedule = connection != nil
            wake()
        } else {
            displayed["schedule"] = nil
            snapshot.gantt = nil
            owed.schedule = false
            if let consumer = consumer {
                consumer.install(Array(displayed.values))
                syncFreshness()
            }
            publish()
        }
    }

    // MARK: Following

    func followLoop(_ mine: Int) async {
        while generation == mine, !Task.isCancelled {
            await idle(mine)
            guard generation == mine, !Task.isCancelled, let connection = connection, let consumer = consumer else { return }
            if case .stop = await pass(mine, connection: connection, consumer: consumer) { return }
        }
    }

    func pass(_ mine: Int, connection: NativeConnection, consumer: Consumer) async -> Outcome {
        do {
            var pages = 0
            var more = false
            repeat {
                let answer = try await connection.changes(consumer.cursors, limit: settings.pageLimit)
                guard generation == mine else { return .stop }
                for changes in settings.intercept?(answer) ?? [answer] {
                    consumer.apply(changes)
                    absorb(changes)
                }
                more = [answer.project?.more, answer.lifecycle?.more, answer.activity?.more].contains(true)
                pages += 1
            } while more && pages < settings.maxPagesPerPass
            snapshot.freshness.catchingUp = more
            owed.merge(due())
            // Whatever the feeds just said is shown as stale before any read is awaited.
            syncFreshness()
            if owed.any {
                publish()
                try await refresh(mine, connection: connection, consumer: consumer)
            }
            guard generation == mine else { return .stop }
            syncFreshness()
            if snapshot.connection != .connected { snapshot.connection = .connected }
            // A failure is forgotten only when nothing it left owing is still owed.
            if !owed.any { snapshot.freshness.lastError = nil }
            if Date().timeIntervalSince(reconnectedAt) >= settings.stableAfter { failureStreak = 0 }
            publish()
            return .continue
        } catch {
            return await fail(error, mine: mine, connection: connection)
        }
    }

    /// What the clock or a deferred read makes due, never a project or run change.
    func due() -> Owed {
        var needs = Owed()
        let now = Date()
        if let until = runsDeferredUntil, now >= until { needs.merge(.runChange) }
        if settings.pinnedClock == nil, now.timeIntervalSince(lastTimeRead) >= 0.05 {
            let bound = TimeInterval(connection?.capabilities?.limits.reevaluateWithinSeconds ?? 60)
            if let at = nextRefreshAt(), now >= at {
                needs.merge(.clock)
            } else if now.timeIntervalSince(lastTimeRead) >= bound {
                needs.merge(.clock)
            }
        }
        return needs
    }

    /// The earliest instant the application said a displayed answer changes with no new revision.
    func nextRefreshAt() -> Date? {
        displayed.values.compactMap { $0.refreshAt?.date }.min()
    }

    /// Where every displayed view was anchored and where each feed stands.
    func refreshInspection() {
        let names = ["status": "Status", "next": "Next work", "runs": "Runs, newest", "inventory": "Plan snapshot", "subject": "Selected detail", "workRuns": "Selected task's runs", "schedule": "Schedule (Gantt)"]
        var rows: [ViewBasisRow] = []
        for (slot, view) in displayed.sorted(by: { $0.key < $1.key }) {
            let project = view.basis.project.map { "revision \($0.revision), history head \($0.historyHead), lineage \($0.lineageId ?? "none")" }
            let runs = view.basis.runs.map { "epoch \($0.epoch ?? "none"), lifecycle \($0.lifecycleHead), activity \($0.activityHead) (pruned through \($0.activityPrunedThrough)), links \($0.linkCount)" }
            rows.append(ViewBasisRow(slot: names[slot] ?? slot, evaluatedAt: view.evaluatedAt, refreshAt: view.refreshAt, project: project, runs: runs))
        }
        var cursors: [String] = []
        if let held = consumer?.cursors {
            if let cursor = held.project { cursors.append("project feed: lineage \(cursor.lineageId ?? "none"), after \(cursor.afterSequence)") }
            if let cursor = held.lifecycle { cursors.append("lifecycle feed: epoch \(cursor.epoch ?? "none"), after \(cursor.afterSequence)") }
            if let cursor = held.activity { cursors.append("activity feed: epoch \(cursor.epoch ?? "none"), after \(cursor.afterSequence)") }
            if let mark = held.links { cursors.append("link count: \(mark.count)") }
        }
        if let window = snapshot.window { cursors.append("selected run's window: feed position \(window.last), \(window.entries.count) shown of \(window.recorded)") }
        snapshot.inspection = Inspection(views: rows, cursors: cursors)
    }

    func syncFreshness() {
        refreshInspection()
        guard let consumer = consumer else { return }
        snapshot.freshness.projectStale = consumer.projectStale
        snapshot.freshness.runsStale = consumer.runsStale
        let now = Date()
        snapshot.freshness.clockDue = settings.pinnedClock == nil && (nextRefreshAt().map { now >= $0 } ?? false)
    }

    func idle(_ mine: Int) async {
        if wakePending {
            wakePending = false
            return
        }
        var seconds = snapshot.freshness.catchingUp ? 0 : settings.pollInterval
        let now = Date()
        if settings.pinnedClock == nil, let at = nextRefreshAt() { seconds = min(seconds, max(at.timeIntervalSince(now), 0)) }
        if let until = runsDeferredUntil { seconds = min(seconds, max(until.timeIntervalSince(now), 0)) }
        guard seconds > 0 else { return }
        sleepToken &+= 1
        let token = sleepToken
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            sleeper = continuation
            Task {
                try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                self.timerFired(token)
            }
        }
    }

    func timerFired(_ token: Int) {
        guard token == sleepToken, let waiting = sleeper else { return }
        sleeper = nil
        waiting.resume()
    }

    /// Ask the follower to pass now rather than at its next interval. One flag: it cannot grow.
    func wake() {
        if let waiting = sleeper {
            sleeper = nil
            sleepToken &+= 1
            waiting.resume()
        } else {
            wakePending = true
        }
    }

    // MARK: Ending

    /// Take everything the engine holds for the current connection, at once and without suspending, and
    /// return the task that ends it: the published connection, and an open still starting its helper,
    /// which is cancelled and waited for. The task also waits for every earlier ending.
    func detach() -> Task<Void, Never> {
        follower?.cancel()
        follower = nil
        let pending = opening
        pending?.cancel()
        opening = nil
        let old = connection
        connection = nil
        consumer = nil
        displayed = [:]
        subject = nil
        owed = Owed()
        failureStreak = 0
        runsDeferredUntil = nil
        wakePending = false
        if let waiting = sleeper {
            sleeper = nil
            sleepToken &+= 1
            waiting.resume()
        }
        let previous = cleanup
        let ending = Task.detached {
            await previous?.value
            // A cancelled open that finished anyway returns its connection, which is ended too.
            if let pending = pending, let late = try? await pending.value { await Self.end(late) }
            await Self.end(old)
        }
        cleanup = ending
        return ending
    }

    /// One read, through the fault seam if there is one.
    func read(_ query: JSON, _ connection: NativeConnection) async throws -> View {
        if let error = settings.fault?(query) { throw error }
        return try await connection.query(query)
    }

    static func end(_ connection: NativeConnection?) async {
        guard let connection = connection else { return }
        _ = try? await connection.close()
    }

    func publish() {
        snapshot.sequence &+= 1
        snapshot.helperProcess = connection?.helperProcessIdentifier
        publisher(snapshot)
    }

    static func describe(_ error: Error) -> String {
        if let bridge = error as? BridgeError { return bridge.description }
        return "\(error)"
    }

    static func placeholder(_ subject: Subject, inventory: Inventory) -> SubjectDetail {
        var title = "Loading…"
        if case .work(let identity) = subject, let key = inventory.key(of: identity) { title = key }
        return SubjectDetail(subject: subject, title: title, subtitle: "", sections: [], revision: nil, loading: true, error: nil)
    }
}
