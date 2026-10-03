// UI-40: the observer's engine against the real helper and the real CLI. Every scenario reads a
// disposable synthetic store that another process writes, and asserts what an operator would see in
// the snapshot the interface renders. Nothing here sleeps and retries: each wait ends on the
// published snapshot that satisfies it, and a wait that never ends fails the scenario with what the
// observer showed instead. Every engine a scenario opens is closed whether it passed or failed.

import DPMNative
import DPMObserverCore
import Foundation

extension JSON {
    /// The values of an object, or none.
    var objectValues: [JSON] {
        if case .object(let fields) = self { return Array(fields.values) }
        return []
    }
}

final class Flag: @unchecked Sendable {
    private let lock = NSLock()
    private var armed = false
    func arm() { lock.lock(); armed = true; lock.unlock() }
    func take() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        let was = armed
        armed = false
        return was
    }
}

extension Context {
    /// The fixture's gated plan with a start-to-start lag of a few seconds, so readiness depends on
    /// time alone once TEST-A has started.
    func shortLagPlan(seconds: Double) throws -> String {
        var plan = try JSONDecoder().decode(JSON.self, from: Data(contentsOf: URL(fileURLWithPath: tools.laggedPlan)))
        guard case .object(var fields) = plan, case .array(var edges) = fields["dependencies"] ?? .null else { throw SuiteError(description: "the lagged plan has no dependencies") }
        for index in edges.indices {
            guard case .object(var edge) = edges[index], edge["kind"]?.string == "StartStart" else { continue }
            edge["lag_hours"] = .number(seconds / 3600)
            edges[index] = .object(edge)
        }
        fields["dependencies"] = .array(edges)
        plan = .object(fields)
        let path = scratch("short-lag-plan.json")
        try JSONEncoder().encode(plan).write(to: path)
        return path.path
    }

    func require(_ condition: Bool, _ message: @autoclosure () -> String) throws {
        check(condition, message())
        if !condition { throw SuiteError(description: "stopped because: \(message())") }
    }

    func observerStartup() async throws {
        print("observer: startup shows the work, the runs, the review queue and what holds work back")
        let database = try await newStore("observer-startup")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        for (index, entry) in [("progress", "reading"), ("tool_started", "Read: a.txt"), ("tool_result", "Read ok"), ("input_requested", "AskUserQuestion: ok?")].enumerated() {
            try await record(database, run, entry.0, index + 1, entry.1)
        }
        try await worker(database, ["submit", "TEST-A", "--note", "ready for review"])
        let operations = try await historyCount(database)
        try await withObserver(.database(database)) { engine, recorder, snapshot in
            check(snapshot.identity?.source == "live" && !snapshot.isPreview, "a live store is labelled live")
            check(snapshot.status?.awaitingVerification == 1 && snapshot.inventory.submitted.map(\.key) == ["TEST-A"], "the review queue names the one submitted task, which the status can only count")
            check(snapshot.candidates.isEmpty && snapshot.status?.blockers.contains(where: { $0.kind == .dependency && $0.key == "TEST-A" }) == true, "with nothing ready, the blockers say what holds the work back: a dependency on TEST-A")
            check(snapshot.runs.count == 1 && snapshot.runs[0].observation == .known(.managed) && !snapshot.runs[0].finished, "the managed run is listed as unfinished")
            check(snapshot.runs.first?.workIdentity == identity(of: "TEST-A", in: snapshot), "a run follows its task by persistent identity")
            check(snapshot.unsupported.map(\.control).contains("steer_run") && snapshot.unsupported.map(\.control).contains("answer_input_request"), "unsupported controls are carried from the application's own capabilities")
            check(snapshot.unsupported.allSatisfy { !$0.plain.contains("RUN-") && !$0.plain.contains("belongs to") }, "and are told in plain words, with no internal phase names")
            check(snapshot.freshness.current && snapshot.freshness.evaluatedAt != nil, "the first snapshot is current and names its evaluation time")
            check(!snapshot.operations.isEmpty, "recent operations are seeded from history")
            check(!snapshot.inspection.views.isEmpty && snapshot.inspection.cursors.contains { $0.hasPrefix("project feed") }, "the basis of every displayed view and the feed cursors can be inspected")
            let selected = try await selectAndRead(engine, recorder, .run(run), "the managed run")
            try require(selected.window?.run == run, "the run's window was read")
            check(selected.window?.entries.map(\.sourceSequence) == [1, 2, 3, 4], "the run's public activity is shown in the order it was recorded")
            check(selected.window?.entries.last?.kind == .known(.inputRequested), "an input request is shown as one")
            let sections = selected.detail?.sections.map(\.title) ?? []
            check(["Run", "Contract observed when the run started (fixed)", "The task now (it may have moved on)", "Lifecycle"].allSatisfy { sections.contains($0) }, "a run's detail carries the contract it observed, apart from the task as it is now: \(sections)")
            check(selected.detail?.sections.first { $0.title.hasPrefix("Contract observed") }?.rows.contains { $0.text.contains("Produce observable result A") } == true, "including the objective it saw")
            guard let identity = identity(of: "TEST-A", in: selected) else { throw SuiteError(description: "TEST-A has no identity in the inventory") }
            let work = try await selectAndRead(engine, recorder, .work(identity), "TEST-A")
            let titles = work.detail?.sections.map(\.title) ?? []
            check(["Summary", "Objective", "Acceptance criteria", "Readiness"].allSatisfy(titles.contains), "a task's detail carries its objective, acceptance criteria and readiness: \(titles)")
            let tasked = try await recorder.expect("TEST-A's own runs") { $0.workRuns?.work == identity }
            check(tasked.workRuns?.runs.count == 1 && tasked.workRuns?.coverage.truncated == false, "the task's own runs were read for it, completely")
            check(work.window == nil, "leaving a run releases its window")
            let after = try await historyCount(database)
            check(after == operations, "observing wrote nothing: the operation count is unchanged at \(after)")
        }
    }

    /// A run's completion is the executor's report: the task is exactly as it was.
    func observerCompletionIsNotVerification() async throws {
        print("observer: a completed run, a full progress report and a closed window verify nothing")
        let database = try await newStore("observer-completion")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await record(database, run, "progress", 1, "all done")
        try await worker(database, ["progress", "TEST-A", "100"])
        _ = try await cliJSON(database, ["run", "report", run, "completed", "--actor", "service:dpm-claude"])
        var history = 0
        try await withObserver(.database(database)) { _, _, snapshot in
            check(snapshot.runs.first?.state == "completed" && snapshot.runs.first?.finished == true, "the run is shown as ended by the executor's report")
            let task = snapshot.inventory.items.first { $0.key == "TEST-A" }
            check(task?.status == "InProgress" && task?.reportedProgress == 100, "the task is still in progress at 100 percent reported: not submitted, not accepted")
            check(snapshot.inventory.submitted.isEmpty && snapshot.status?.awaitingVerification == 0 && snapshot.status?.verified == false, "nothing awaits review and nothing is verified")
            check(task?.owner == "agent:worker", "the owner still holds the task")
            history = try await historyCount(database)
        }
        let remaining = try await historyCount(database)
        var stillOwner = true
        do { try await worker(database, ["progress", "TEST-A", "100"]) } catch { stillOwner = false }
        check(history == remaining && stillOwner, "closing released nothing and wrote nothing: the same owner can still report")
    }

    func observerExternalCommitLatency() async throws {
        print("observer: an external commit appears within two seconds and the selection stays")
        let database = try await newStore("observer-latency")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        monitor.reset()
        try await withObserver(.database(database), production: true) { engine, recorder, ready in
            guard let identity = identity(of: "TEST-A", in: ready) else { throw SuiteError(description: "TEST-A has no identity in the inventory") }
            try await selectAndRead(engine, recorder, .work(identity), "TEST-A")
            var worst = 0.0
            var worstAfterCommit = 0.0
            var kept = true
            var samples: [String] = []
            for percent in [20, 40, 60, 80] {
                let revision = recorder.latest.revision ?? 0
                let started = DispatchTime.now()
                try await worker(database, ["progress", "TEST-A", "\(percent)"])
                let committed = DispatchTime.now()
                let shown = try await recorder.expect("\(percent) percent to appear in the selected task", 10) { snapshot in
                    snapshot.revision == revision + 1
                        && snapshot.detail?.sections.first?.rows.contains(where: { $0.label == "Reported progress" && $0.text.hasPrefix("\(percent)%") }) == true
                }
                let total = milliseconds(since: started)
                worst = max(worst, total)
                worstAfterCommit = max(worstAfterCommit, milliseconds(since: committed))
                samples.append("\(Int(total)) ms")
                kept = kept && shown.detail?.subject == .work(identity)
            }
            check(worst < 2000, "each external commit was on screen within two seconds of the command starting: \(samples.joined(separator: ", ")) (worst after the commit returned: \(Int(worstAfterCommit)) ms; workload: the 7-task fixture, production poll interval, release helper)")
            check(kept, "the selection stayed on the same task by identity through every refresh")
            let gap = self.monitor.snapshot.maxGapMilliseconds
            check(gap < 500, "the main queue was never held for long while this ran (longest gap \(Int(gap)) ms)")
            print("  measured: worst update latency \(Int(worst)) ms from the CLI starting, \(Int(worstAfterCommit)) ms from the commit returning")
        }
    }

    func observerStartupRace() async throws {
        print("observer: a write that lands during startup is not lost")
        let database = try await newStore("observer-race")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let exchanges = Counter()
        let dpm = tools.dpm
        let (engine, recorder) = observer { settings in
            let inner = settings.onBlockingStep
            settings.onBlockingStep = { name in
                inner?(name)
                // The fifth exchange is the runs read: after status and next were read, before the rest.
                if name == "exchange", exchanges.next() == 5 {
                    runSynchronously(dpm, ["--database", database.path, "--json", "progress", "TEST-A", "30", "--actor", "agent:worker"])
                }
            }
        }
        await engine.open(.database(database))
        do {
            let revision = try await currentRevision(database)
            let current = try await recorder.expect("the observer to catch up with the write made during its startup (store revision \(revision))") { $0.connection == .connected && $0.revision == revision && $0.freshness.current }
            check(current.operations.contains(where: { $0.verb == "ReportProgress" }), "the operation made during startup is in the recent operations: \(current.operations.map(\.verb))")
            check(current.operations.filter { $0.verb == "ReportProgress" }.count == 1, "and it is not doubled")
        } catch {
            await engine.close()
            throw error
        }
        await engine.close()
    }

    func observerClockOnlyGate() async throws {
        print("observer: a gate that opens by the clock alone appears at an unchanged revision")
        let lag = 3.0
        let database = try await newStore("observer-clock", plan: try shortLagPlan(seconds: lag))
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        try await withObserver(.database(database)) { _, recorder, first in
            guard let started = first.inventory.items.first(where: { $0.key == "TEST-A" })?.startedAt else { throw SuiteError(description: "TEST-A has no start time") }
            check(!first.candidates.contains(where: { $0.key == "TEST-B" }), "TEST-B is not offered while its start-to-start lag has not elapsed")
            let boundary = started.adding(seconds: Int64(lag)).date
            let opened = try await recorder.expect("TEST-B to become ready when the lag elapsed", 15) { $0.candidates.contains(where: { $0.key == "TEST-B" }) }
            let late = Date().timeIntervalSince(boundary)
            check(opened.revision == first.revision && opened.counters.projectOperations == first.counters.projectOperations, "with no project operation and no new revision")
            check(opened.counters.timeReads >= 1, "the engine read again because the application's refresh instant passed")
            check(late >= 0 && late < 1.5, "and it appeared \(Int(late * 1000)) ms after the instant the application named")
        }
    }

    func observerReportedOnlyAndStale() async throws {
        print("observer: a reported-only run that goes quiet is stale, never idle or finished")
        let database = try await newStore("observer-stale")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await reportedRun(database)
        try await record(database, run, "heartbeat", 1, actor: "agent:worker")
        try await withObserver(.database(database)) { _, _, current in
            check(current.runs.first?.observation == .known(.reportedOnly) && current.runs.first?.status == "working", "a reported-only run heard from just now is working")
        }
        let later = Instant(seconds: Int64(Date().timeIntervalSince1970) + 600)
        try await withObserver(.database(database), clock: later) { _, _, snapshot in
            let seen = snapshot.runs.first
            check(seen?.status == "stale" && seen?.state == "working" && seen?.finished == false, "ten minutes of silence makes it stale while its last report stays working: not idle, not finished")
            check(snapshot.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "InProgress", "and the task is untouched")
        }
    }

    /// The application refuses to rename a task's key in a reviewed change, so a key can differ for
    /// one identity only between sources. A selection is kept by identity and read by whatever key
    /// the source now gives that identity. This is a cross-source identity check, not an in-place
    /// rename: no such rename exists to observe.
    func observerFollowsRenamedKeys() async throws {
        print("observer: a selection follows a task's persistent identity across sources, not its key")
        var plan = try JSONDecoder().decode(JSON.self, from: Data(contentsOf: URL(fileURLWithPath: tools.plan)))
        let identity = plan["work_items"].objectValues.first { $0["key"].string == "TEST-C" }?["id"].string ?? ""
        guard case .object(var fields) = plan, case .object(var work) = fields["work_items"] ?? .null, case .object(var item) = work[identity] ?? .null else {
            throw SuiteError(description: "the fixture has no TEST-C")
        }
        item["key"] = .string("TEST-C-RENAMED")
        work[identity] = .object(item)
        fields["work_items"] = .object(work)
        plan = .object(fields)
        let renamed = scratch("renamed-plan.json")
        try JSONEncoder().encode(plan).write(to: renamed)
        let workspace = plan["workspace"]["id"].string ?? ""
        let root = try project("observer-rename", workspace: workspace, selects: "database = 'a.sqlite'")
        let first = root.appendingPathComponent(".dpm/a.sqlite"), second = root.appendingPathComponent(".dpm/b.sqlite")
        _ = try await cliJSON(first, ["import", tools.plan])
        _ = try await cliJSON(second, ["import", renamed.path])
        let refusal = try await runProcess(tools.dpm, ["--database", first.path, "--json", "plan", "apply", renamed.path, "--reason", "rename", "--actor", "human:reviewer"])
        check(refusal.status != 0 && refusal.text.contains("preserve keys"), "a reviewed change cannot rename a key: the application refuses it, so only another source can differ")
        let model = await MainActor.run { () -> ObserverModel in
            let model = ObserverModel()
            model.minimumInterval = 0
            var settings = ObserverEngine.Settings(helper: helperURL)
            settings.pollInterval = 0.05
            model.configure(settings: settings, problem: nil)
            model.open(.project(root))
            return model
        }
        do {
            _ = try await modelExpect(model, "the first source to connect") { $0.connection == .connected }
            await MainActor.run { model.select(.work(identity)) }
            let shown = try await modelExpect(model, "TEST-C to be selected") { $0.detail?.subject == .work(identity) && $0.detail?.subtitle.hasPrefix("TEST-C ") == true }
            try "version = 3\nworkspace = '\(workspace)'\ndatabase = 'b.sqlite'\n".write(to: root.appendingPathComponent(".dpm/project.toml"), atomically: true, encoding: .utf8)
            _ = try await modelExpect(model, "the repointed locator to be reported") { if case .sourceChanged = $0.connection { return true } else { return false } }
            await MainActor.run { model.reload() }
            let followed = try await modelExpect(model, "the selection to be read again from the new source") { $0.generation > shown.generation && $0.connection == .connected && $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
            check(followed.detail?.subtitle.hasPrefix("TEST-C-RENAMED") == true && followed.detail?.error == nil, "the same task is shown under the key the new source gives it: \(followed.detail?.subtitle ?? "")")
        } catch {
            await MainActor.run { model.close() }
            throw error
        }
        await MainActor.run { model.close() }
    }

    func observerRefusedRefreshIsRetried() async throws {
        print("observer: a refused refresh stays owed and is retried even when the feed is empty")
        let database = try await newStore("observer-refused")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let armed = Flag()
        try await withObserver(.database(database), adjust: { settings in
            settings.fault = { query in
                guard query["query"].string == "status", armed.take() else { return nil }
                return BridgeError.refused(NativeError(code: .workspaceChanging, message: "the project changed under a read", details: nil))
            }
        }) { _, recorder, first in
            guard let before = first.revision else { throw SuiteError(description: "the first snapshot has no revision") }
            armed.arm()
            try await worker(database, ["progress", "TEST-A", "50"])
            let after = try await currentRevision(database)
            let failed = try await recorder.expect("the refused read to be visible as a stale view with an error") { $0.freshness.lastError != nil && $0.freshness.projectStale }
            check(failed.revision == before, "while the read failed the old revision stayed on screen, marked stale")
            let healed = try await recorder.expect("the failed refresh to be retried and installed") { $0.revision == after && !$0.freshness.projectStale && $0.freshness.lastError == nil }
            check(healed.counters.projectReads >= 1, "the retry read the project views again (\(healed.counters.projectReads) project reads)")
            let coherent = recorder.snapshots.allSatisfy { snapshot in !(snapshot.revision == after && !snapshot.freshness.projectStale && snapshot.freshness.lastError != nil) }
            check(coherent, "no snapshot showed the new revision as current while its refresh was still owed")
        }
    }

    /// A task held back only by a decision, and a task in progress that no run has touched, are both
    /// found and read in full without a terminal.
    func observerReachesBlockedAndUnrunWork() async throws {
        print("observer: a decision-only blocked task and unrun started work are reachable with their evidence")
        let gated = try await newStore("observer-decision")
        try await worker(gated, ["claim", "TEST-A"])
        try await worker(gated, ["start", "TEST-A"])
        try await worker(gated, ["submit", "TEST-A"])
        _ = try await cliJSON(gated, ["verify", "TEST-A", "--actor", "human:reviewer"])
        try await withObserver(.database(gated)) { engine, recorder, snapshot in
            guard let group = snapshot.status?.blockers.first(where: { $0.kind == .decision && $0.key == "TEST-GATE" }), let decision = snapshot.inventory.decision(key: "TEST-GATE") else {
                throw SuiteError(description: "the open decision TEST-GATE is not among the blockers: \(snapshot.status?.blockers.map(\.id) ?? [])")
            }
            check(group.tasks >= 1 && decision.status == "Open", "the blocking decision is named with how many tasks it holds back")
            let detail = try await selectAndRead(engine, recorder, .decision(decision.identity), "the decision")
            let titles = detail.detail?.sections.map(\.title) ?? []
            check(titles.contains("Question") && titles.contains { $0.hasPrefix("Work this decision gates") }, "the decision shows its question and the work it gates: \(titles)")
            check(detail.detail?.sections.first { $0.title.hasPrefix("Work this decision gates") }?.rows.contains { $0.label == "TEST-B" } == true, "including TEST-B")
            guard let blocked = snapshot.inventory.items.first(where: { $0.key == "TEST-B" }) else { throw SuiteError(description: "TEST-B is not in the inventory") }
            let found = snapshot.inventory.search("test-b", limit: 100)
            check(found.items.contains { $0.identity == blocked.identity }, "a search by key reaches it though it is ranked nowhere and has no run")
            let work = try await selectAndRead(engine, recorder, .work(blocked.identity), "TEST-B")
            let readiness = work.detail?.sections.first { $0.title == "Readiness" }?.rows.map(\.text).joined(separator: " ") ?? ""
            check(readiness.contains("decision TEST-GATE") && !readiness.contains("TEST-A"), "its readiness names the decision as the only thing in the way: \(readiness)")
            check(work.detail?.sections.contains { $0.title == "Acceptance criteria" } == true && work.detail?.sections.contains { $0.title == "Decisions" } == true, "with its acceptance criteria and the decision's text in full")
        }
        let started = try await newStore("observer-unrun")
        try await worker(started, ["claim", "TEST-A"])
        try await worker(started, ["start", "TEST-A"])
        try await withObserver(.database(started)) { engine, recorder, snapshot in
            guard let identity = identity(of: "TEST-A", in: snapshot) else { throw SuiteError(description: "TEST-A has no identity") }
            check(snapshot.runs.isEmpty, "no run exists, so the lists of runs cannot lead to the started task")
            let work = try await selectAndRead(engine, recorder, .work(identity), "TEST-A")
            let loaded = try await recorder.expect("the task's own runs") { $0.workRuns?.work == identity }
            check(work.detail?.subtitle.contains("InProgress") == true && loaded.workRuns?.runs.isEmpty == true && loaded.workRuns?.coverage.truncated == false, "started work with no run is found by search and says the application holds no run for it")
            check(work.detail?.sections.contains { $0.title == "Acceptance criteria" } == true && work.detail?.sections.contains { $0.title == "Readiness" } == true, "with its contract and readiness in full")
        }
    }

    /// The global list holds the newest runs only; nothing absent from it is claimed absent, and a
    /// run that leaves it while selected stays inspectable by its own identity.
    func observerBoundedCoverage() async throws {
        print("observer: bounded run lists say how much they read and a selected run outlives the list")
        let database = try await newStore("observer-coverage")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let first = try await reportedRun(database)
        try await withObserver(.database(database), adjust: { $0.runLimit = 5 }) { engine, recorder, snapshot in
            guard let identity = identity(of: "TEST-A", in: snapshot) else { throw SuiteError(description: "TEST-A has no identity") }
            check(snapshot.runsCoverage.read == 1 && !snapshot.runsCoverage.truncated, "one run is all of the application's runs, and the list says it read all of them")
            try await selectAndRead(engine, recorder, .run(first), "the first run")
            for _ in 0..<6 { _ = try await reportedRun(database) }
            let crowded = try await recorder.expect("the first run to leave the newest-runs list") { $0.runsCoverage.truncated && !$0.runs.contains { $0.identity == first } }
            check(crowded.runsCoverage.words.contains("older ones were not read"), "the list says it holds only the newest runs: \(crowded.runsCoverage.words)")
            check(crowded.runDetail?.identity == first && crowded.detail?.subject == .run(first) && crowded.detail?.error == nil, "the selected run is still inspected by its own identity after it left the list")
            check(crowded.detail?.sections.contains { $0.title == "Contract observed when the run started (fixed)" } == true, "with the contract it observed")
            let work = try await selectAndRead(engine, recorder, .work(identity), "TEST-A")
            let loaded = try await recorder.expect("the task's own runs, read for it", 20) { $0.workRuns?.work == identity && $0.workRuns?.coverage.truncated == true }
            check(loaded.workRuns?.runs.count == 5 && work.detail?.error == nil, "the task's own query is bounded too, and says it read only the newest 5")
            guard let other = snapshot.inventory.items.first(where: { $0.key == "TEST-C" }) else { throw SuiteError(description: "TEST-C is not in the inventory") }
            try await selectAndRead(engine, recorder, .work(other.identity), "TEST-C")
            let none = try await recorder.expect("TEST-C's own runs") { $0.workRuns?.work == other.identity }
            check(none.workRuns?.runs.isEmpty == true && none.workRuns?.coverage.truncated == false, "a task with no run is told so only because its own complete reading found none")
        }
    }

    /// Must fail: it awaits an event that never comes. The packaged qualification runs it by name
    /// and requires a nonzero exit, so a silently passing wait cannot return unnoticed.
    func observerNegativeControl() async throws {
        print("observer: negative control, an event that never happens must fail the scenario")
        let database = try await newStore("observer-negative")
        try await withObserver(.database(database)) { _, recorder, _ in
            _ = try await recorder.expect("a revision that will never exist", 1) { $0.revision == 999_999 }
        }
    }
}
