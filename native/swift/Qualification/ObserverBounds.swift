// UI-40: the limits that are real, not simulated. Retention gaps come from the application's own
// 10,000-record retention, reached by writing through the shared application into disposable
// storage. A helper whose handshake is held is a real child of the engine, ended by a close or a
// replacement without ever being released. The state file's writer is held and burst.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    private func fifo(_ name: String) async throws -> URL {
        let url = scratch(name)
        let made = try await runProcess("/usr/bin/mkfifo", [url.path])
        guard made.status == 0 else { throw SuiteError(description: "mkfifo \(url.path) failed: \(made.text)") }
        return url
    }

    /// Records written through one agent connection, which is how a store reaches its retention limit
    /// in seconds. They go through the shared application exactly as `dpm run record` does.
    func writeActivity(_ database: URL, run: String, from first: Int, count: Int) async throws {
        guard !tools.writer.isEmpty else { throw SuiteError(description: "the suite was not given --writer, so no store can be taken past its retention limit") }
        let directory = URL(fileURLWithPath: tools.writer).deletingLastPathComponent()
        let result = try await runProcess("/usr/bin/env", ["python3", tools.writer, database.path, run, "\(first)", "\(count)"], directory: directory, timeout: 300)
        guard result.status == 0 else { throw SuiteError(description: "writing \(count) records exited \(result.status): \(result.text) \(String(decoding: result.stderr, as: UTF8.self))") }
    }

    /// A launcher that, when it is to hold (for one store, or while `armed` exists), says its process
    /// identifier on `ready` and then waits on `gate`, which nothing opens unless the test releases it.
    private func holdingHelper(ready: URL, gate: URL, store: URL? = nil, armed: URL? = nil) throws -> URL {
        let script = scratch("holding-helper")
        let hold = "echo $$ > '\(ready.path)'; read _ < '\(gate.path)'"
        let body: String
        if let store = store {
            body = "case \"$*\" in *'\(store.path)'*) \(hold);; esac"
        } else if let armed = armed {
            body = "if [ -e '\(armed.path)' ]; then \(hold); fi"
        } else {
            body = hold
        }
        try "#!/bin/sh\n\(body)\nexec '\(tools.helper)' \"$@\"\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script
    }

    /// The process identifier a held helper announces: it is the engine's own child, caught in its handshake.
    private func heldPid(_ ready: URL) async throws -> Int32 {
        let said = try await runProcess("/bin/cat", [ready.path], timeout: 20)
        guard said.status == 0, let pid = Int32(said.text.trimmingCharacters(in: .whitespacesAndNewlines)) else { throw SuiteError(description: "no held helper announced itself: \(said.text)") }
        return pid
    }

    /// The retention gap the application reports to a reader that starts far behind.
    func observerRetentionGapAtStartup() async throws {
        print("observer: a retention gap the helper really reports is kept and named at startup")
        let database = try await newStore("observer-gap-start")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await writeActivity(database, run: run, from: 1, count: 10_500)
        try await withObserver(.database(database)) { engine, recorder, _ in
            let selected = try await selectAndRead(engine, recorder, .run(run), "the run with 10,500 records")
            let window = selected.window
            check(window?.gap?.contains("Retention dropped") == true, "the gap the application reported is in the window: \(window?.gap ?? "none")")
            check(window?.recorded == 10_500 && (window?.droppedFromView ?? 0) + (window?.entries.count ?? 0) == 10_000, "of 10,500 recorded, the 10,000 it still holds are counted: \(window?.droppedFromView ?? -1) not shown + \(window?.entries.count ?? -1) shown")
            check(window?.incomplete == nil, "and the paging bound was not what limited the reading")
            check(window?.entries.last?.sourceSequence == 10_500, "the newest record is the newest written")
        }
    }

    /// A gap that opens while the helper is away: records written past retention during an outage.
    func observerRetentionGapAfterReconnect() async throws {
        print("observer: a retention gap that opens while the helper is away is named after the reconnect")
        let database = try await newStore("observer-gap-reconnect")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await writeActivity(database, run: run, from: 1, count: 300)
        let ready = try await fifo("gap-ready.fifo"), gate = try await fifo("gap-gate.fifo")
        let armed = scratch("gap-armed")
        let helper = try holdingHelper(ready: ready, gate: gate, armed: armed)
        let (engine, recorder) = observer { settings in
            settings.helper = helper
            settings.reconnectDelays = [0.05]
        }
        await engine.open(.database(database))
        do {
            _ = try await recorder.connected("the observer to connect")
            try await selectAndRead(engine, recorder, .run(run), "the run with 300 records")
            // From here a replacement helper is held in its handshake until the test lets it go.
            try Data().write(to: armed)
            guard let pid = await engine.helperProcessIdentifier else { throw SuiteError(description: "the engine has no helper to lose") }
            kill(pid, SIGKILL)
            _ = try await recorder.expect("the lost helper to be shown") { if case .reconnecting = $0.connection { return true } else { return false } }
            let held = try await heldPid(ready)
            try await writeActivity(database, run: run, from: 301, count: 10_500)
            check(!recorder.saw { $0.connection == .connected && $0.counters.reconnects >= 1 }, "while the replacement is held, the observer stays disconnected and says so")
            try "go".write(to: gate, atomically: false, encoding: .utf8)
            let back = try await recorder.expect("the observer to reconnect past the gap", 60) { $0.connection == .connected && $0.counters.reconnects >= 1 && ($0.window?.recorded ?? 0) >= 10_800 && $0.window?.gap != nil }
            check(back.window?.gap?.contains("Retention dropped") == true, "the gap that opened during the outage is named: \(back.window?.gap ?? "none")")
            check(back.window?.entries.last?.sourceSequence == 10_800 && back.freshness.current, "and the window is current at the newest record")
            _ = held
        } catch {
            await engine.close()
            throw error
        }
        await engine.close()
    }

    /// Whether a process still exists. A helper that was ended and reaped does not.
    private func exists(_ pid: Int32) -> Bool { kill(pid, 0) == 0 }

    /// A launcher for the store it is to hold: it notes any earlier held helper still alive as an
    /// overlap, records its own process, and then waits on `gate`, which nothing opens.
    private func countingHelper(ready: URL, gate: URL, log: URL, violations: URL, store: URL) throws -> URL {
        let script = scratch("counting-helper")
        let hold = "for p in $(cat '\(log.path)' 2>/dev/null); do kill -0 \"$p\" 2>/dev/null && echo \"$p\" >> '\(violations.path)'; done; echo $$ >> '\(log.path)'; echo $$ > '\(ready.path)'; read _ < '\(gate.path)'"
        try "#!/bin/sh\ncase \"$*\" in *'\(store.path)'*) \(hold);; esac\nexec '\(tools.helper)' \"$@\"\n".write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script
    }

    /// A close, and a replacement, while a real helper is held in its handshake: the held child is
    /// ended and reaped by the time the close or the replacement returns, without ever being
    /// released, and nothing old is published. Rapid superseding opens leave no helper behind and
    /// never have two held helpers alive at once.
    func observerPendingOpenIsEnded() async throws {
        print("observer: closing or superseding an open whose helper handshake is held ends that helper before it returns")
        let held = try await newStore("pending-held"), other = try await newStore("pending-other")
        try await worker(other, ["claim", "TEST-A"])
        // Closed while held.
        let ready = try await fifo("pending-ready.fifo"), gate = try await fifo("pending-gate.fifo")
        let helper = try holdingHelper(ready: ready, gate: gate, store: held)
        let (engine, recorder) = observer { $0.helper = helper }
        let opening = Task { await engine.open(.database(held)) }
        let pid = try await heldPid(ready)
        check(exists(pid), "the helper's handshake is held: its process \(pid) is alive and nothing has released it")
        let closing = DispatchTime.now()
        await engine.close()
        let closed = milliseconds(since: closing)
        check(!exists(pid), "when close returned, the held helper (process \(pid)) was already ended and reaped")
        check(closed < (2 * 3.0 + 2) * 1000, "and the close took \(Int(closed)) ms, within the bound of two shutdown steps")
        check(recorder.latest.connection == .closed, "and the close is shown")
        await opening.value
        check(!recorder.snapshots.contains { $0.connection == .connected } && recorder.latest.connection == .closed, "the cancelled open never published a connection, and the close is still the last word")

        // Superseded while held.
        let ready2 = try await fifo("pending-ready2.fifo"), gate2 = try await fifo("pending-gate2.fifo")
        let helper2 = try holdingHelper(ready: ready2, gate: gate2, store: held)
        let (second, secondRecorder) = observer { $0.helper = helper2 }
        let older = Task { await second.open(.database(held)) }
        let olderPid = try await heldPid(ready2)
        let replacing = DispatchTime.now()
        await second.open(.database(other))
        let replaced = milliseconds(since: replacing)
        do {
            check(!exists(olderPid), "when the newer open returned, the older held helper (process \(olderPid)) was already ended and reaped")
            check(replaced < (2 * 3.0 + 2) * 1000 + 10_000, "and replacing it, including connecting the newer open, took \(Int(replaced)) ms")
            let newest = try await secondRecorder.expect("the newer open to connect") { $0.connection == .connected && $0.generation == 2 }
            check(newest.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "Claimed", "the newer open shows its own store")
            await older.value
            check(!secondRecorder.snapshots.contains { $0.generation == 1 && $0.connection == .connected }, "the superseded open never published a connection")
        } catch {
            await second.close()
            throw error
        }
        await second.close()

        // A storm of superseding opens, none released. The first one is waited for until its helper is
        // really held, so the storm cannot pass without a held child; the rest are all admitted
        // before the final open is made, so the final open is the newest by construction.
        let bound = 3.0
        let log = scratch("storm.pids"), violations = scratch("storm.violations")
        let stormReady = try await fifo("storm-ready.fifo"), stormGate = try await fifo("storm-gate.fifo")
        let counting = try countingHelper(ready: stormReady, gate: stormGate, log: log, violations: violations, store: held)
        let (storm, stormRecorder) = observer { $0.helper = counting }
        let first = Task { await storm.open(.database(held)) }
        let firstPid = try await heldPid(stormReady)
        check(exists(firstPid), "a held helper (process \(firstPid)) really is in its handshake before the storm begins")
        let rest = (0..<7).map { _ in Task { await storm.open(.database(held)) } }
        _ = try await stormRecorder.expect("all eight opens to be admitted", 20) { $0.generation >= 8 }
        let begun = DispatchTime.now()
        await storm.open(.database(other))
        let elapsed = milliseconds(since: begun)
        do {
            let launched = ((try? String(contentsOf: log, encoding: .utf8)) ?? "").split(separator: "\n").compactMap { Int32($0) }
            check(launched.contains(firstPid) && launched.allSatisfy { !exists($0) }, "when the final open returned, none of the \(launched.count) held helpers it superseded was left alive")
            check(elapsed < Double(launched.count) * (2 * bound + 2) * 1000, "and ending them was bounded: \(Int(elapsed)) ms for \(launched.count) held helpers")
            _ = try await stormRecorder.expect("the final open to connect") { $0.connection == .connected && $0.generation == 9 && $0.inventory.items.first(where: { $0.key == "TEST-A" })?.status == "Claimed" }
            await first.value
            for attempt in rest { await attempt.value }
            let overlaps = (try? String(contentsOf: violations, encoding: .utf8)) ?? ""
            check(overlaps.isEmpty, "no held helper was ever started while an earlier one was still alive: \(overlaps)")
            check(!stormRecorder.snapshots.contains { $0.generation < 9 && $0.connection == .connected }, "no superseded open published a connection")
        } catch {
            await storm.close()
            throw error
        }
        await storm.close()
    }

    /// A run with more history than one reading takes in says so, for its lifecycle as for its activity,
    /// and never presents what it read as the newest.
    func observerPageCapsAreSaid() async throws {
        print("observer: a run read only in part says so, for its lifecycle and its activity")
        let database = try await newStore("observer-caps")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let long = try await managedRun(database)
        for sequence in 1...12 { try await record(database, long, "tool_result", sequence, "record \(sequence)") }
        for turn in 0..<12 { _ = try await cliJSON(database, ["run", "report", long, turn % 2 == 0 ? "waiting" : "working", "--actor", "service:dpm-claude"]) }
        let short = try await managedRun(database)
        for sequence in 1...3 { try await record(database, short, "tool_result", sequence, "short \(sequence)") }
        try await withObserver(.database(database), adjust: { settings in
            settings.lifecyclePageSize = 5
            settings.lifecyclePages = 2
            settings.activityPageSize = 5
            settings.activityPages = 2
        }) { engine, recorder, _ in
            let partial = try await selectAndRead(engine, recorder, .run(long), "the long run")
            check(partial.window?.incomplete?.contains("Reading stopped after 10 records") == true, "activity beyond the page bound is said to be unread: \(partial.window?.incomplete ?? "none")")
            check(partial.window?.lifecycleIncomplete?.contains("not its newest") == true, "so is lifecycle, and what was read is not called the newest: \(partial.window?.lifecycleIncomplete ?? "none")")
            check(partial.detail?.sections.first { $0.title == "Lifecycle" }?.rows.first?.text.contains("not its newest") == true, "the run's detail carries the same coverage statement")
            let whole = try await selectAndRead(engine, recorder, .run(short), "the short run")
            check(whole.window?.incomplete == nil && whole.window?.lifecycleIncomplete == nil, "a run the bound covers claims nothing is missing")
        }
    }

    /// A selection's views leave the displayed set with it, so switching away from a task leaves
    /// nothing to hold the client stale or to schedule a read.
    func observerSelectionOwnedViewsLeave() async throws {
        print("observer: switching away from a task leaves none of its views displayed, and freshness settles")
        let database = try await newStore("observer-slots")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await record(database, run, "progress", 1, "first")
        try await withObserver(.database(database)) { engine, recorder, snapshot in
            guard let identity = identity(of: "TEST-A", in: snapshot), let decision = snapshot.inventory.decision(key: "TEST-GATE") else { throw SuiteError(description: "TEST-A or TEST-GATE is missing") }
            let owned = ["Selected task's runs", "Selected detail"]
            func shows(_ s: ObserverSnapshot) -> Bool { s.inspection.views.contains { owned.contains($0.slot) } }
            let task = try await selectAndRead(engine, recorder, .work(identity), "TEST-A")
            _ = try await recorder.expect("the task's own runs to be displayed") { $0.workRuns?.work == identity && $0.inspection.views.contains { $0.slot == "Selected task's runs" } }
            check(shows(task) || recorder.latest.inspection.views.contains { $0.slot == "Selected task's runs" }, "while a task is selected its own views are on display")
            let onRun = try await selectAndRead(engine, recorder, .run(run), "the run")
            check(!onRun.inspection.views.contains { $0.slot == "Selected task's runs" } && onRun.workRuns == nil, "selecting a run removes the task's runs from the displayed set")
            try await record(database, run, "tool_started", 2, "a real run event")
            let settled = try await recorder.expect("the window to take the event and freshness to settle") { $0.window?.entries.count == 2 && $0.freshness.current }
            check(!settled.freshness.clockDue && !settled.inspection.views.contains { $0.slot == "Selected task's runs" }, "freshness settled with no hidden view left to hold it stale")
            let timeReads = settled.counters.timeReads
            _ = try await selectAndRead(engine, recorder, .work(identity), "TEST-A again")
            _ = try await selectAndRead(engine, recorder, .decision(decision.identity), "the decision")
            check(recorder.latest.inspection.views.allSatisfy { !owned.contains($0.slot) || $0.slot == "Selected detail" }, "selecting a decision leaves no task views displayed")
            await engine.select(nil)
            try await record(database, run, "tool_started", 3, "a run event with nothing selected")
            let bare = try await recorder.expect("freshness to settle with nothing selected") { $0.freshness.current && $0.detail == nil && $0.counters.runReads > settled.counters.runReads }
            check(bare.inspection.views.allSatisfy { !owned.contains($0.slot) } && bare.counters.timeReads == timeReads, "with nothing selected only the shared views remain, and no clock-driven read was scheduled by a hidden one")
        }
    }

    /// A record written between the run view and the activity pages is counted once.
    func observerWindowTotalAfterRacingWrite() async throws {
        print("observer: a record that lands between a run's view and its activity is counted once")
        let database = try await newStore("observer-window-race")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let run = try await managedRun(database)
        try await record(database, run, "progress", 1, "one")
        try await record(database, run, "tool_started", 2, "two")
        let armed = Flag()
        let dpm = tools.dpm
        try await withObserver(.database(database), adjust: { settings in
            settings.fault = { query in
                // The lifecycle page is read right after the run view and before the activity pages.
                if query["query"].string == "run_lifecycle", armed.take() {
                    runSynchronously(dpm, ["--database", database.path, "--json", "run", "record", run, "tool_result", "--sequence", "3", "--text", "raced", "--actor", "service:dpm-claude"])
                }
                return nil
            }
        }) { engine, recorder, _ in
            armed.arm()
            let selected = try await selectAndRead(engine, recorder, .run(run), "the run")
            check(selected.window?.entries.map(\.sourceSequence) == [1, 2, 3], "the racing record is in the window: \(selected.window?.entries.map(\.sourceSequence) ?? [])")
            let settled = try await recorder.expect("the feed to deliver the same record and the total to settle") { $0.counters.activityDiscarded >= 1 && $0.freshness.current }
            let answer = try await cliJSON(database, ["run", "show", run])
            let shown = (answer["data"]["activity"]["recorded"].uint ?? answer["activity"]["recorded"].uint).map { Int($0) }
            check(settled.window?.recorded == 3 && shown == 3 && settled.window?.entries.count == 3, "the total is 3 as the application says, with the record counted once (window \(settled.window?.recorded ?? -1), application \(shown ?? -1))")
        }
    }

    /// A held writer and a burst: one write in flight, one latest waiting, the main thread free.
    func observerStateWriterIsBounded() async throws {
        print("observer: the state file's writer holds at most one write and one waiting state")
        let file = scratch("state.json")
        let release = DispatchSemaphore(value: 0)
        let first = Event(), second = Event()
        let calls = Counter()
        let written = Mutexed<[Data]>([])
        let sink: StateReporter.Sink = { data, url in
            let call = calls.next()
            written.append(data)
            if call == 1 {
                first.signal()
                release.wait()
            } else if call == 2 {
                second.signal()
            }
            try data.write(to: url, options: .atomic)
        }
        let (model, reporter) = await MainActor.run { () -> (ObserverModel, StateReporter) in
            let model = ObserverModel()
            model.minimumInterval = 0
            let reporter = StateReporter(file: file, sink: sink)
            reporter.attach(model)
            return (model, reporter)
        }
        await MainActor.run { reporter.write() }
        guard await first.wait(timeout: 10) else { throw SuiteError(description: "the first write never reached the held sink") }
        let burst = await MainActor.run { () -> (Double, Int) in
            let started = DispatchTime.now()
            for index in 0..<1000 {
                var snapshot = ObserverSnapshot()
                snapshot.sequence = UInt64(index + 1)
                model.receive(snapshot)
                model.flush()
                reporter.write()
            }
            return (Double(DispatchTime.now().uptimeNanoseconds - started.uptimeNanoseconds) / 1_000_000, reporter.writes)
        }
        check(burst.0 < 1000, "a burst of 1,000 snapshots behind a held write did not block the main thread (\(Int(burst.0)) ms)")
        check(burst.1 == 1, "and handed nothing more to the writer while one write was held (writes: \(burst.1))")
        release.signal()
        guard await second.wait(timeout: 10) else { throw SuiteError(description: "the waiting state was never written after the held write finished") }
        let all = written.value
        check(all.count == 2, "exactly one write was in flight and one waited: \(all.count) writes reached the sink for 1,001 snapshots")
        let latest = (try? JSONSerialization.jsonObject(with: all.last ?? Data())) as? [String: Any]
        check((latest?["sequence"] as? Int) == 1000, "the one that waited was the latest snapshot: sequence \(String(describing: latest?["sequence"]))")

        // A write that fails is counted and its reason is carried in the next state.
        let failing = Event(), recovered = Event()
        let seen = Mutexed<[Data]>([])
        let count = Counter()
        let hold = DispatchSemaphore(value: 0)
        struct Refused: Error, CustomStringConvertible { var description: String { "the disk refused the write" } }
        let broken: StateReporter.Sink = { data, _ in
            let call = count.next()
            seen.append(data)
            if call == 1 {
                failing.signal()
                hold.wait()
                throw Refused()
            }
            recovered.signal()
        }
        let reporting = await MainActor.run { () -> StateReporter in
            let reporter = StateReporter(file: file, sink: broken)
            reporter.attach(model)
            reporter.write()
            return reporter
        }
        guard await failing.wait(timeout: 10) else { throw SuiteError(description: "the failing write never started") }
        await MainActor.run { reporting.write() }
        hold.signal()
        guard await recovered.wait(timeout: 10) else { throw SuiteError(description: "the state after a failed write was never written") }
        let after = (try? JSONSerialization.jsonObject(with: seen.value.last ?? Data())) as? [String: Any]
        check((after?["state_write_failures"] as? Int) == 1 && (after?["last_write_error"] as? String)?.contains("refused") == true, "the failure was counted and its reason carried in the next state: \(String(describing: after?["last_write_error"]))")

        // A state that cannot be encoded fails the same way, off the main thread, and is reported.
        struct Unwritable: Error, CustomStringConvertible { var description: String { "the state could not be encoded" } }
        let encodes = Counter(), carried = Event()
        let captured = Mutexed<[Data]>([])
        let encoding = await MainActor.run { () -> StateReporter in
            let reporter = StateReporter(file: file, sink: { data, _ in
                captured.append(data)
                carried.signal()
            }, encode: { state in
                if encodes.next() == 1 { throw Unwritable() }
                return try JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
            })
            reporter.attach(model)
            reporter.write()
            return reporter
        }
        await MainActor.run { encoding.write() }
        guard await carried.wait(timeout: 10) else { throw SuiteError(description: "the state after an encoding failure was never written") }
        let encoded = (try? JSONSerialization.jsonObject(with: captured.value.last ?? Data())) as? [String: Any]
        check((encoded?["state_write_failures"] as? Int) == 1 && (encoded?["last_write_error"] as? String)?.contains("encoded") == true, "an encoding failure is counted and shown in the next state written: \(String(describing: encoded?["last_write_error"]))")
    }
}

/// A value shared between the suite and a sink that runs on another thread.
final class Mutexed<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var stored: Value
    init(_ value: Value) { stored = value }
    var value: Value { lock.lock(); defer { lock.unlock() }; return stored }
    func append<Element>(_ element: Element) where Value == [Element] { lock.lock(); stored.append(element); lock.unlock() }
}
