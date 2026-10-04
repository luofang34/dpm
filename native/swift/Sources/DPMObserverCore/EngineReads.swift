// What the engine reads, how it applies the feeds, and what it does when the bridge fails.

import DPMNative
import Foundation

extension ObserverEngine {
    // MARK: Opening

    /// The first read of a workspace: every view the interface shows, then a consumer seeded from
    /// all of them, so a write that lands during startup is delivered by the first poll.
    func load(_ connection: NativeConnection, mine: Int) async throws {
        snapshot.identity = connection.identity
        if let capabilities = connection.capabilities {
            snapshot.unsupported = capabilities.unsupported.map { Unsupported(control: $0.control, reason: $0.reason) }
            snapshot.reportedOnlyNote = capabilities.runs.reportedOnly
        }
        let status = try await read(Queries.status, connection)
        guard generation == mine else { return }
        let next = try await read(Queries.next, connection)
        guard generation == mine else { return }
        let runs = try await read(Queries.runs(limit: settings.runLimit), connection)
        guard generation == mine else { return }
        let inventory = try await read(Queries.inventory, connection)
        guard generation == mine else { return }
        var schedule: View?
        if wantsSchedule {
            schedule = try await read(Queries.schedule, connection)
            guard generation == mine else { return }
        }
        let head = status.basis.project?.historyHead ?? 0
        let from = head > UInt64(settings.operationLimit) ? head - UInt64(settings.operationLimit) : 0
        let history = try await read(Queries.history(after: from, limit: settings.operationLimit), connection)
        guard generation == mine else { return }
        install(status: status)
        install(next: next)
        install(runs: runs)
        install(inventory: inventory)
        if let schedule = schedule { install(schedule: schedule) }
        snapshot.operations = history.envelope.data["entries"].items.compactMap(OperationEntry.init)
        lastRunsRead = Date()
        lastTimeRead = Date()
        let seeded = Consumer(views: Array(displayed.values))
        seeded.install(Array(displayed.values))
        consumer = seeded
        syncFreshness()
    }

    // MARK: Installing answers

    func remember(_ slot: String, _ view: View) {
        displayed[slot] = view
        snapshot.freshness.receivedAt = Date()
    }

    func install(status view: View) {
        remember("status", view)
        snapshot.status = StatusSummary(view)
        snapshot.revision = view.envelope.revision
        snapshot.freshness.evaluatedAt = view.evaluatedAt
    }

    func install(next view: View) {
        remember("next", view)
        snapshot.candidates = view.envelope.data["candidates"].items.compactMap(Candidate.init)
    }

    func install(runs view: View) {
        remember("runs", view)
        snapshot.runs = view.envelope.data["runs"].items.compactMap(RunSummary.init)
        snapshot.runsCoverage = Coverage(read: snapshot.runs.count, limit: settings.runLimit)
        reconcileWindowTotal()
    }

    /// The selected run's total never falls behind what the application currently says it received:
    /// the larger of the window's own count and the latest run summary, never their sum, so a record
    /// that arrived through both the page and the summary is counted once.
    func reconcileWindowTotal() {
        guard var window = snapshot.window else { return }
        let summary = snapshot.runDetail?.identity == window.run ? snapshot.runDetail : snapshot.runs.first { $0.identity == window.run }
        if let summary = summary, summary.recorded > window.recorded {
            window.recorded = summary.recorded
            snapshot.window = window
        }
    }

    func install(inventory view: View) {
        remember("inventory", view)
        snapshot.inventory = Inventory(view)
    }

    /// The Gantt's projection, kept only while the Gantt is shown.
    func install(schedule view: View) {
        guard wantsSchedule else { return }
        remember("schedule", view)
        snapshot.gantt = GanttSchedule(view)
        owed.schedule = false
    }

    // MARK: Refreshing

    /// Read what is owed. Each read clears its own debt only after it was installed, so a failure
    /// leaves the rest owed for the next pass.
    func refresh(_ mine: Int, connection: NativeConnection, consumer: Consumer) async throws {
        // The schedule follows the status read: both depend on the revision and on the clock. Its debt is
        // registered here, before any read is awaited, and `install(schedule:)` or hiding the Gantt is
        // what clears it: a status read that succeeds and a schedule read that then fails leaves it owed.
        if wantsSchedule, owed.status { owed.schedule = true }
        if owed.status {
            let status = try await read(Queries.status, connection)
            guard generation == mine else { return }
            install(status: status)
            publish()
            let next = try await read(Queries.next, connection)
            guard generation == mine else { return }
            install(next: next)
            if owed.cause == .project { snapshot.counters.projectReads &+= 1 } else { snapshot.counters.timeReads &+= 1 }
            lastTimeRead = Date()
            owed.status = false
            publish()
        }
        if wantsSchedule, owed.schedule {
            let schedule = try await read(Queries.schedule, connection)
            guard generation == mine else { return }
            install(schedule: schedule)
            publish()
        }
        if owed.inventory {
            let inventory = try await read(Queries.inventory, connection)
            guard generation == mine else { return }
            install(inventory: inventory)
            owed.inventory = false
            publish()
        }
        if owed.runs {
            let runs = try await read(Queries.runs(limit: settings.runLimit), connection)
            guard generation == mine else { return }
            install(runs: runs)
            runsDeferredUntil = nil
            lastRunsRead = Date()
            snapshot.counters.runReads &+= 1
            owed.runs = false
            switch subject {
            case .run?: owed.subject = true
            case .work?: owed.workRuns = true
            default: break
            }
            publish()
        }
        if owed.workRuns, case .work(let identity)? = subject {
            try await loadWorkRuns(identity, mine: mine, connection: connection)
            guard generation == mine else { return }
            owed.workRuns = false
            publish()
        }
        if owed.subject, let chosen = subject {
            try await loadSubject(chosen, mine: mine, connection: connection, keepWindow: !owed.window)
            guard generation == mine else { return }
        }
        if subject == nil {
            owed.subject = false
            owed.workRuns = false
        }
        consumer.install(Array(displayed.values))
    }

    /// The selected task's own runs, read by the per-task query, with how much of them was read.
    func loadWorkRuns(_ identity: String, mine: Int, connection: NativeConnection) async throws {
        guard let key = snapshot.inventory.key(of: identity) else { return }
        let view = try await read(Queries.runs(limit: settings.runLimit, key: key), connection)
        guard generation == mine, subject == .work(identity) else { return }
        remember("workRuns", view)
        let runs = view.envelope.data["runs"].items.compactMap(RunSummary.init)
        snapshot.workRuns = WorkRuns(work: identity, runs: runs, coverage: Coverage(read: runs.count, limit: settings.runLimit))
    }

    /// Read the selected task or run. A task is read by the key it goes by now, found from its
    /// persistent identity, so a rename is followed rather than lost. `keepWindow` keeps the run's
    /// window, which the feed maintains, instead of reading it again. The debt is cleared only when
    /// this selection's answer was installed; a selection that changed meanwhile stays owed.
    func loadSubject(_ chosen: Subject, mine: Int, connection: NativeConnection, keepWindow: Bool) async throws {
        switch chosen {
        case .work(let identity):
            guard let key = snapshot.inventory.key(of: identity) else {
                guard generation == mine, subject == chosen else { return }
                snapshot.detail = SubjectDetail(subject: chosen, title: "Task not found", subtitle: "", sections: [], revision: snapshot.revision, loading: false,
                                                error: "This task is no longer in the plan the application serves.")
                displayed["subject"] = nil
                owed.subject = false
                owed.window = false
                return
            }
            do {
                let view = try await read(Queries.explain(key), connection)
                guard generation == mine, subject == chosen else { return }
                let inventory = snapshot.inventory
                guard view.envelope.data["work"]["id"].string == identity else {
                    throw BridgeError.invalidFrame("the key \(key) now names another task", fate: .notSent)
                }
                remember("subject", view)
                snapshot.detail = DetailBuilder.work(view, name: { inventory.key(of: $0) ?? String($0.prefix(8)) })
                try await loadWorkRuns(identity, mine: mine, connection: connection)
                guard generation == mine, subject == chosen else { return }
                owed.subject = false
                owed.workRuns = false
                owed.window = false
            } catch BridgeError.refused(let native) where native.code == .notFound {
                guard generation == mine, subject == chosen else { return }
                snapshot.detail = SubjectDetail(subject: chosen, title: key, subtitle: "", sections: [], revision: snapshot.revision, loading: false, error: native.message)
                displayed["subject"] = nil
                owed.subject = false
                owed.window = false
            }
        case .decision(let id):
            guard generation == mine, subject == chosen else { return }
            if let info = snapshot.inventory.decisionByIdentity[id] {
                snapshot.detail = DetailBuilder.decision(info, inventory: snapshot.inventory)
            } else {
                snapshot.detail = SubjectDetail(subject: chosen, title: "Decision not found", subtitle: "", sections: [], revision: snapshot.revision, loading: false,
                                                error: "This decision is no longer in the plan the application serves.")
            }
            displayed["subject"] = nil
            owed.subject = false
            owed.window = false
        case .run(let id):
            do {
                let view = try await read(Queries.run(id), connection)
                guard generation == mine, subject == chosen else { return }
                var window = snapshot.window
                if !keepWindow || window?.run != id {
                    window = try await readWindow(id, recorded: view.envelope.data["activity"]["recorded"].int ?? 0, connection: connection)
                    guard generation == mine, subject == chosen else { return }
                }
                snapshot.window = window
                snapshot.runDetail = RunSummary(view.envelope.data)
                reconcileWindowTotal()
                remember("subject", view)
                let inventory = snapshot.inventory
                snapshot.detail = DetailBuilder.run(view, lifecycle: window?.lifecycle ?? [], lifecycleNote: window?.lifecycleIncomplete, operations: snapshot.operations, inventory: inventory)
                owed.subject = false
                owed.window = false
            } catch BridgeError.refused(let native) where native.code == .notFound {
                guard generation == mine, subject == chosen else { return }
                snapshot.detail = SubjectDetail(subject: chosen, title: "Run not found", subtitle: "", sections: [], revision: snapshot.revision, loading: false, error: native.message)
                snapshot.window = nil
                snapshot.runDetail = nil
                displayed["subject"] = nil
                owed.subject = false
                owed.window = false
            }
        }
    }

    /// The words for a retention gap the activity feed reported: what was asked, where it resumes,
    /// and how many records, across every run, are gone.
    static func describe(gap: JSON) -> String {
        let lost = gap["lost"].uint.map { "\($0) records (across all runs)" } ?? "records"
        let from = gap["requested_after"].uint.map { " after feed position \($0)" } ?? ""
        let to = gap["resumes_at"].uint.map { ", resuming at \($0)" } ?? ""
        return "Retention dropped \(lost)\(from)\(to). Activity before that is not held, so what is shown is not complete."
    }

    /// A run's lifecycle and the newest of its activity, paged within bounds. What does not fit the
    /// window is counted, a retention gap the feed reports is kept, and an unfinished read says so.
    func readWindow(_ id: String, recorded: Int, connection: NativeConnection) async throws -> RunWindow {
        var lifecycle: [LifecycleEntry] = []
        var lifecycleDropped = 0
        var after: UInt64 = 0
        var lifecycleIncomplete: String?
        let size = settings.lifecyclePageSize, lifecyclePages = settings.lifecyclePages
        for number in 0..<lifecyclePages {
            let page = try await read(Queries.lifecycle(run: id, after: after, limit: size), connection)
            let entries = page.envelope.data["entries"].items.compactMap(LifecycleEntry.init)
            lifecycle += entries
            if lifecycle.count > settings.lifecycleLimit {
                lifecycleDropped += lifecycle.count - settings.lifecycleLimit
                lifecycle.removeFirst(lifecycle.count - settings.lifecycleLimit)
            }
            after = page.envelope.data["next_after_sequence"].uint ?? after
            if entries.count < size { break }
            if number == lifecyclePages - 1 {
                lifecycleIncomplete = "Reading stopped after \(size * lifecyclePages) lifecycle entries; later transitions of this run are not shown here, so what is listed is not its newest. The executor's latest report is in the state above."
            }
        }
        var entries: [ActivityEntry] = []
        var dropped = 0
        var last: UInt64 = 0
        var cursor: UInt64 = 0
        var gap: String?
        var incomplete: String?
        let pageSize = settings.activityPageSize, pages = settings.activityPages
        for number in 0..<pages {
            let page = try await read(Queries.activity(run: id, after: cursor, limit: pageSize), connection)
            if gap == nil, case let found = page.envelope.data["gap"], found != .null { gap = Self.describe(gap: found) }
            let found = page.envelope.data["entries"].items.compactMap(ActivityEntry.init)
            entries += found
            if entries.count > settings.windowLimit {
                dropped += entries.count - settings.windowLimit
                entries.removeFirst(entries.count - settings.windowLimit)
            }
            last = max(last, found.last?.sequence ?? last)
            cursor = page.envelope.data["next_after_sequence"].uint ?? cursor
            if found.count < pageSize { break }
            if number == pages - 1 {
                incomplete = "Reading stopped after \(pages * pageSize) records; this run has more activity than one reading takes in, so the newest records may be missing until the next refresh."
            }
        }
        // The run view was read before the pages, so a record written between them is in the pages
        // and not in `recorded`. Every record held is a record received, so the total is never less.
        return RunWindow(run: id, entries: entries, lifecycle: lifecycle, recorded: max(recorded, dropped + entries.count), droppedFromView: dropped, lifecycleDropped: lifecycleDropped, gap: gap, incomplete: incomplete, lifecycleIncomplete: lifecycleIncomplete, last: last)
    }

    // MARK: Applying the feeds

    /// Apply one answer of `changes` to what is shown, and record what must be read again.
    func absorb(_ changes: Changes) {
        snapshot.counters.polls &+= 1
        if let project = changes.project {
            if project.isReset {
                snapshot.counters.resets &+= 1
                snapshot.operations.removeAll()
                owed.merge(.everything)
            } else if !project.entries.isEmpty {
                for entry in project.entries.compactMap(OperationEntry.init) where !snapshot.operations.contains(where: { $0.sequence >= entry.sequence }) {
                    snapshot.operations.append(entry)
                    snapshot.counters.projectOperations &+= 1
                }
                if snapshot.operations.count > settings.operationLimit { snapshot.operations.removeFirst(snapshot.operations.count - settings.operationLimit) }
                owed.merge(.project)
            }
        }
        if let revision = snapshot.revision, changes.watermark.project.revision != revision { owed.merge(.project) }
        if let lifecycle = changes.lifecycle {
            if lifecycle.isReset {
                snapshot.counters.resets &+= 1
                owed.merge(.runChange)
                if case .run? = subject { owed.subject = true; owed.window = true }
                snapshot.window = nil
            } else if !lifecycle.entries.isEmpty {
                owed.merge(.runChange)
                applyLifecycle(lifecycle)
            }
        }
        if let activity = changes.activity {
            if activity.isReset {
                snapshot.counters.resets &+= 1
                owed.merge(.runChange)
                if case .run? = subject { owed.subject = true; owed.window = true }
                snapshot.window = nil
            } else {
                applyActivity(activity)
            }
        }
        if let links = changes.links, links.isReset || links.changed { owed.merge(.runChange) }
    }

    func applyLifecycle(_ page: Delta) {
        guard var window = snapshot.window else { return }
        for entry in page.entries.compactMap(LifecycleEntry.init) where entry.run == window.run && !window.lifecycle.contains(where: { $0.sequence >= entry.sequence }) {
            window.lifecycle.append(entry)
        }
        if window.lifecycle.count > settings.lifecycleLimit {
            window.lifecycleDropped += window.lifecycle.count - settings.lifecycleLimit
            window.lifecycle.removeFirst(window.lifecycle.count - settings.lifecycleLimit)
        }
        snapshot.window = window
    }

    func applyActivity(_ page: Delta) {
        if let gap = page.gap {
            snapshot.counters.gaps &+= 1
            snapshot.window?.gap = Self.describe(gap: gap)
            owed.merge(.runChange)
        }
        guard !page.entries.isEmpty else { return }
        // Run telemetry never reads the project and is read back at most once per interval.
        if Date().timeIntervalSince(lastRunsRead) >= settings.runsRefreshInterval {
            owed.merge(.runChange)
        } else if runsDeferredUntil == nil {
            runsDeferredUntil = lastRunsRead.addingTimeInterval(settings.runsRefreshInterval)
        }
        guard var window = snapshot.window else {
            snapshot.counters.activityApplied &+= page.entries.count
            return
        }
        for entry in page.entries.compactMap(ActivityEntry.init) where entry.run == window.run {
            if entry.sequence <= window.last {
                snapshot.counters.activityDiscarded &+= 1
                continue
            }
            window.entries.append(entry)
            window.last = entry.sequence
            window.recorded &+= 1
            snapshot.counters.activityApplied &+= 1
        }
        if window.entries.count > settings.windowLimit {
            window.droppedFromView += window.entries.count - settings.windowLimit
            window.entries.removeFirst(window.entries.count - settings.windowLimit)
        }
        snapshot.window = window
    }

    // MARK: Failure

    enum Failure {
        case transient(String)
        case lost(String)
        case sourceChanged(String)
        case fatal(String)
    }

    static func classify(_ error: Error) -> Failure {
        guard let bridge = error as? BridgeError else { return .transient("\(error)") }
        switch bridge {
        case .refused(let native):
            switch native.code {
            case .sourceChanged, .workspaceMismatch, .lineageMismatch: return .sourceChanged(native.message)
            default: return .transient("\(native)")
            }
        case .sourceIdentityChanged(let expected, let found):
            return .sourceChanged("The project now selects another source: was \(expected), now \(found).")
        case .busy, .cancelled:
            return .transient(bridge.description)
        case .startupRefused, .incompatible, .invalidConfiguration, .requestTooLarge:
            return .fatal(bridge.description)
        default:
            return .lost(bridge.description)
        }
    }

    func fail(_ error: Error, mine: Int, connection: NativeConnection) async -> Outcome {
        guard generation == mine else { return .stop }
        switch Self.classify(error) {
        case .transient(let message):
            snapshot.freshness.lastError = message
            syncFreshness()
            publish()
            return .continue
        case .sourceChanged(let message):
            snapshot.connection = .sourceChanged(message)
            snapshot.freshness.projectStale = true
            snapshot.freshness.runsStale = true
            publish()
            return .stop
        case .fatal(let message):
            snapshot.connection = .failed(message)
            publish()
            return .stop
        case .lost(let cause):
            return await reconnect(mine: mine, connection: connection, cause: cause)
        }
    }

    /// Replace the helper until it works, keeping everything shown and saying how long it has been
    /// lost. The same source is required; a different one is reported, never adopted. Everything is
    /// owed again afterwards: nothing was observed while the helper was gone.
    func reconnect(mine: Int, connection: NativeConnection, cause first: String) async -> Outcome {
        var cause = first
        var attempt = failureStreak
        while generation == mine, !Task.isCancelled {
            attempt += 1
            failureStreak = attempt
            snapshot.connection = .reconnecting(attempt: attempt, cause: cause)
            snapshot.freshness.runsStale = true
            snapshot.freshness.projectStale = true
            publish()
            let delays = settings.reconnectDelays
            let delay = delays.isEmpty ? 1 : delays[min(attempt - 1, delays.count - 1)]
            try? await Task.sleep(nanoseconds: UInt64(delay * 1_000_000_000))
            guard generation == mine, !Task.isCancelled else { return .stop }
            do {
                _ = try await connection.reconnect()
                guard generation == mine else { return .stop }
                snapshot.counters.reconnects &+= 1
                reconnectedAt = Date()
                snapshot.connection = .connected
                owed.merge(.everything)
                publish()
                return .continue
            } catch {
                guard generation == mine else { return .stop }
                switch Self.classify(error) {
                case .sourceChanged(let message):
                    snapshot.connection = .sourceChanged(message)
                    publish()
                    return .stop
                case .fatal(let message):
                    snapshot.connection = .failed(message)
                    publish()
                    return .stop
                case .lost(let text), .transient(let text):
                    cause = text
                }
            }
        }
        return .stop
    }
}
