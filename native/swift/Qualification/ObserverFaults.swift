// UI-40: the observer under duplicate pages, a lost helper, a repointed locator, a failing helper, a
// large burst and the model's ordering rules. Each scenario fails, with what was showing, when an
// event it needs never arrives, and closes everything it opened.

import Combine
import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    /// Wait on the main actor for a published model snapshot that satisfies `predicate`, or fail.
    @MainActor
    func modelExpect(_ model: ObserverModel, _ what: String, _ seconds: TimeInterval = 20, _ predicate: @escaping (ObserverSnapshot) -> Bool) async throws -> ObserverSnapshot {
        if predicate(model.snapshot) { return model.snapshot }
        let found: ObserverSnapshot? = await withCheckedContinuation { continuation in
            var cancellable: AnyCancellable?
            var done = false
            cancellable = model.$snapshot.sink { snapshot in
                guard !done, predicate(snapshot) else { return }
                done = true
                continuation.resume(returning: snapshot)
                cancellable?.cancel()
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
                guard !done else { return }
                done = true
                continuation.resume(returning: nil)
                cancellable?.cancel()
            }
        }
        guard let shown = found else {
            let now = model.snapshot
            throw SuiteError(description: "timed out after \(Int(seconds)) s waiting for \(what); the model showed: connection \(now.connection), generation \(now.generation), detail \(now.detail?.title ?? "none"), selection \(String(describing: model.selection))")
        }
        return shown
    }

    func observerDuplicates() async throws {
        print("observer: a page delivered twice changes nothing")
        let database = try await newStore("observer-duplicates")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await record(database, run, "progress", 1, "first")
        try await withObserver(.database(database), adjust: { settings in
            settings.intercept = { answer in [answer, answer] }
        }) { engine, recorder, _ in
            try await selectAndRead(engine, recorder, .run(run), "the run")
            for sequence in 2...4 { try await record(database, run, "tool_started", sequence, "step \(sequence)") }
            let caught = try await recorder.expect("the window to catch up with the three new records") { $0.window?.entries.count == 4 }
            let sequences = caught.window?.entries.map(\.sequence) ?? []
            check(Set(sequences).count == sequences.count && sequences == sequences.sorted(), "every record appears once, in order, though every page arrived twice: \(sequences)")
            check(caught.counters.activityDiscarded >= 3, "the repeats were discarded by feed identity and sequence (\(caught.counters.activityDiscarded) discarded)")
            let operations = caught.operations.map(\.sequence)
            check(Set(operations).count == operations.count, "project operations are not doubled either")
            check(caught.window?.gap == nil, "and no gap was claimed where none exists")
        }
    }

    func observerLostHelperReconnects() async throws {
        print("observer: a lost helper is visible, replaced, and the selection and the missed write survive")
        let database = try await newStore("observer-reconnect")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await record(database, run, "progress", 1, "before the loss")
        try await withObserver(.database(database)) { engine, recorder, _ in
            try await selectAndRead(engine, recorder, .run(run), "the run")
            guard let pid = await engine.helperProcessIdentifier else { throw SuiteError(description: "the engine has no helper to lose") }
            kill(pid, SIGKILL)
            let lost = try await recorder.expect("the lost helper to be shown as a reconnection") { if case .reconnecting = $0.connection { return true } else { return false } }
            check(lost.freshness.projectStale && lost.freshness.runsStale && !lost.inventory.items.isEmpty && lost.window?.run == run, "while it is lost, what was shown stays, marked as stale")
            try await record(database, run, "tool_started", 2, "written while no helper ran")
            let back = try await recorder.expect("the observer to reconnect and catch up") { $0.connection == .connected && $0.counters.reconnects >= 1 && $0.window?.entries.count == 2 && $0.freshness.current }
            check(back.window?.entries.map(\.text) == ["before the loss", "written while no helper ran"], "the record written during the outage arrived once, after the old one")
            check(back.detail?.subject == .run(run), "the selection survived the reconnect")
            check(back.counters.resets == 0, "no reset was needed: the cursors continued through the new helper")
        }
    }

    func observerSourceChange() async throws {
        print("observer: a repointed locator is reported and never silently followed")
        let workspace = try await workspaceId(try await newStore("observer-source-seed"))
        let root = try project("observer-repoint", workspace: workspace, selects: "database = 'a.sqlite'")
        let first = root.appendingPathComponent(".dpm/a.sqlite"), second = root.appendingPathComponent(".dpm/b.sqlite")
        _ = try await cliJSON(first, ["import", tools.plan])
        let backup = scratch("observer-source-backup.sqlite")
        _ = try await cliJSON(first, ["backup", "--to", backup.path])
        _ = try await cliJSON(second, ["restore", "--from", backup.path, "--to", second.path])
        try await worker(second, ["claim", "TEST-A"])
        try await withObserver(.project(root)) { engine, recorder, opened in
            check(opened.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "Planned", "the first source is shown")
            try "version = 3\nworkspace = '\(workspace)'\ndatabase = 'b.sqlite'\n".write(to: root.appendingPathComponent(".dpm/project.toml"), atomically: true, encoding: .utf8)
            let changed = try await recorder.expect("the repointed locator to be reported") { if case .sourceChanged = $0.connection { return true } else { return false } }
            check(changed.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "Planned" && changed.freshness.projectStale, "the other source's data is not shown; the previous view stays, marked stale")
            // Following has ended: once the follower's task has finished, nothing more is read.
            await engine.awaitFollower()
            let polls = changed.counters.polls
            try await worker(first, ["claim", "TEST-A"])
            check(recorder.latest.counters.polls == polls && recorder.latest.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "Planned", "following stopped: nothing was read from either source after the change")
            await engine.reload()
            let reopened = try await recorder.expect("the operator's explicit reload to open the new source") { $0.connection == .connected && $0.generation > changed.generation }
            check(reopened.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "Claimed", "only that explicit step displays the other source")
        }
    }

    func observerHelperFailure() async throws {
        print("observer: a failing helper is a visible state, not a crash, and closing it is prompt")
        let database = try await newStore("observer-faulty")
        let helper = try faultyHelper("exit", after: 8)
        let (engine, recorder) = observer { settings in
            settings.helper = helper
            settings.reconnectDelays = [0.05]
        }
        await engine.open(.database(database))
        do {
            let seen = try await recorder.expect("the failing helper to be shown as a repeated reconnection", 30) { if case .reconnecting(let attempt, _) = $0.connection { return attempt >= 2 } else { return false } }
            check(seen.freshness.runsStale || seen.freshness.projectStale || seen.inventory.items.isEmpty, "what was shown, if anything, is marked stale")
        } catch {
            await engine.close()
            throw error
        }
        let started = DispatchTime.now()
        await engine.close()
        check(milliseconds(since: started) < 5000 && recorder.latest.connection == .closed, "closing while it fails is prompt and final")
    }

    func observerBurst() async throws {
        print("observer: a large burst of activity is bounded, ordered, and reads nothing else")
        let database = try await newStore("observer-burst")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await record(database, run, "progress", 1, "start")
        try await withObserver(.database(database), adjust: { settings in
            settings.pageLimit = 10
            settings.maxPagesPerPass = 2
            settings.windowLimit = 50
            settings.reconnectDelays = [3]
        }) { engine, recorder, _ in
            let baseline = try await selectAndRead(engine, recorder, .run(run), "the run")
            // The helper is lost, so the whole burst is waiting in the feed when the observer returns.
            guard let pid = await engine.helperProcessIdentifier else { throw SuiteError(description: "the engine has no helper to lose") }
            kill(pid, SIGKILL)
            _ = try await recorder.expect("the lost helper to be shown") { if case .reconnecting = $0.connection { return true } else { return false } }
            monitor.reset()
            let total = 241
            for sequence in 2...total { try await record(database, run, "tool_result", sequence, "burst \(sequence)") }
            let settled = try await recorder.expect("the window to record all \(total) records", 60) { ($0.window?.recorded ?? 0) >= total && !$0.freshness.catchingUp && $0.freshness.current }
            let window = settled.window
            let kept = window?.entries.map(\.sequence) ?? []
            check(kept.count <= 50 && Set(kept).count == kept.count && kept == kept.sorted(), "at most 50 records are kept, each once, in order (kept \(kept.count))")
            check((window?.droppedFromView ?? 0) + kept.count == total, "what is not shown is counted, never silently lost: \(window?.droppedFromView ?? -1) dropped + \(kept.count) shown = \(total)")
            check(recorder.saw { $0.freshness.catchingUp }, "a pass that could not read it all said it was catching up")
            check(settled.counters.projectReads - baseline.counters.projectReads <= 1, "the burst caused no project reads of its own; the one read after the outage is the reconnect's (\(settled.counters.projectReads - baseline.counters.projectReads) reads for \(total) records)")
            check(settled.counters.runReads - baseline.counters.runReads < 40, "the runs were read a bounded number of times, not once per record (\(settled.counters.runReads - baseline.counters.runReads) reads for \(total) records)")
            let gap = self.monitor.snapshot.maxGapMilliseconds
            check(gap < 500, "the main queue was never held for long during the burst (longest gap \(Int(gap)) ms)")
            print("  measured: \(total) records, main-queue longest gap \(Int(gap)) ms")
        }
    }

    func observerModelRules() async throws {
        print("observer: the model orders publications and restores a selection once a reopened connection is connected")
        await MainActor.run {
            let model = ObserverModel()
            model.minimumInterval = 0
            func snapshot(_ generation: Int, _ sequence: UInt64) -> ObserverSnapshot {
                var value = ObserverSnapshot()
                value.generation = generation
                value.sequence = sequence
                return value
            }
            // Publications that reach the model out of order, with the newer one still waiting.
            model.receive(snapshot(2, 7))
            model.receive(snapshot(2, 6))
            model.receive(snapshot(1, 99))
            model.flush()
            self.check(model.snapshot.generation == 2 && model.snapshot.sequence == 7, "an older publication that arrives while a newer one waits does not replace it (shown \(model.snapshot.generation)/\(model.snapshot.sequence))")
            model.receive(snapshot(2, 5))
            model.receive(snapshot(1, 100))
            model.flush()
            self.check(model.snapshot.sequence == 7, "nor does one that arrives after it was installed")
            model.receive(snapshot(3, 1))
            model.flush()
            self.check(model.snapshot.generation == 3, "a newer generation always replaces")
        }
        let database = try await newStore("observer-model")
        try await worker(database, ["claim", "TEST-A"])
        let model = await MainActor.run { () -> ObserverModel in
            let model = ObserverModel()
            model.minimumInterval = 0
            var settings = ObserverEngine.Settings(helper: helperURL)
            settings.pollInterval = 0.05
            model.configure(settings: settings, problem: nil)
            model.open(.database(database))
            return model
        }
        do {
            let ready = try await modelExpect(model, "the model to open the store") { $0.connection == .connected }
            guard let identity = identity(of: "TEST-A", in: ready) else { throw SuiteError(description: "TEST-A has no identity") }
            await MainActor.run { model.select(.work(identity)) }
            _ = try await modelExpect(model, "the model's selection to be read") { $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
            // Reopening the same workspace: nothing is selected in the new connection until it is connected.
            await MainActor.run { model.open(.database(database)) }
            let restored = try await modelExpect(model, "the selection to be restored once the reopened connection is connected") { $0.generation > ready.generation && $0.connection == .connected && $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
            check(restored.detail?.title.isEmpty == false, "the restored selection has its detail again, by the same persistent identity")
            await MainActor.run { model.close() }
            _ = try await modelExpect(model, "the workspace to close") { $0.connection == .closed }
            let selected = await MainActor.run { model.selection }
            check(selected == nil, "closing the workspace clears the selection")
        } catch {
            await MainActor.run { model.close() }
            throw error
        }
    }
}
