// FEAT-20 correction pass 1, qualified through the real helper, the real model and the real measurement
// log: the schedule read owed after a transient failure, the view operation logged on entry, the item's own
// estimate in the inspection text, the identity and trajectory of scripted scroll requests, and the content
// facts a draw must carry. Where a scenario stamps, it calls the very stamp the draw pass calls with the
// values the model holds; the suite has no Canvas, so a real draw is shown by the packaged scenarios of
// scripts/smoke_observer.py and scripts/measure_gantt.py, not here.

import AppKit
import DPMNative
import DPMObserverCore
import Foundation

/// The events a log emitted, in the order they were emitted, with the clock they were stamped on.
private final class Ledger: @unchecked Sendable {
    struct Entry { let event: String; let generation: String; let time: UInt64 }
    private let lock = NSLock()
    private var entries: [Entry] = []

    func add(_ event: String, _ generation: Any?, _ time: UInt64) {
        lock.lock()
        entries.append(Entry(event: event, generation: generation.map { "\($0)" } ?? "", time: time))
        lock.unlock()
    }

    var all: [Entry] { lock.lock(); defer { lock.unlock() }; return entries }
    func has(_ event: String, generation: String) -> Bool { all.contains { $0.event == event && $0.generation == generation } }
}

private func number(_ json: JSON) -> Double? {
    switch json {
    case .number(let value): return value
    case .integer(let value): return Double(value)
    case .unsigned(let value): return Double(value)
    default: return nil
    }
}

private func events(_ url: URL) throws -> [[String: Any]] {
    try String(contentsOf: url, encoding: .utf8).split(separator: "\n").compactMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any] }
}

extension Context {
    // MARK: F20-P1 the schedule read owed after a transient failure

    func ganttScheduleDebtSurvivesATransientFailure() async throws {
        print("gantt: a schedule read that fails once after the status read succeeded stays owed and is installed, with the view marked stale until then")
        let database = try await newStore("gantt-debt", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let failures = Tally()
        let armed = Flag()
        try await withObserver(.database(database), clock: pinned, adjust: { settings in
            settings.fault = { query in
                guard query["query"].string == "schedule", armed.take() else { return nil }
                failures.bump()
                return BridgeError.refused(NativeError(code: .workspaceChanging, message: "the project changed under a read", details: nil))
            }
        }) { engine, recorder, _ in
            await engine.showSchedule(true)
            let shown = try await recorder.expect("the schedule to be installed and the client current", 30) { $0.gantt != nil && $0.freshness.current && $0.gantt?.revision == $0.revision }
            // One external semantic change; status and next succeed, the next schedule read fails once, and nothing else happens.
            armed.arm()
            try await worker(database, ["claim", "TEST-A"])
            let after = try await currentRevision(database)
            check(after != shown.revision, "the external change moved the store to revision \(after)")
            let stale = try await recorder.expect("the new status with the old schedule, marked stale with the failure", 30) { $0.revision == after && $0.gantt?.revision != after && $0.freshness.lastError != nil }
            check(stale.freshness.projectStale && !stale.freshness.current, "while the schedule read is owed the view is marked stale, not current")
            let healed = try await recorder.expect("the owed schedule read to be retried and installed at the new revision", 30) { $0.gantt?.revision == after && $0.freshness.current && $0.freshness.lastError == nil }
            check(healed.gantt?.rows.first { $0.key == "TEST-A" }?.status == "Claimed", "the schedule carries the committed change: TEST-A is \(healed.gantt?.rows.first { $0.key == "TEST-A" }?.status ?? "missing")")
            check(failures.count == 1, "exactly one transient schedule failure was injected (\(failures.count)) and no further commit or page change was needed")
            let coherent = recorder.snapshots.allSatisfy { snapshot in !(snapshot.revision == after && snapshot.gantt?.revision != after && snapshot.freshness.current) }
            check(coherent, "no snapshot showed the new revision as current while its schedule was still the old one")
        }
    }

    // MARK: F20-P2 the view operation is logged on entry

    func ganttViewOperationIsLoggedOnEntry() async throws {
        print("gantt: the real handler logs each view operation's call on entry, before the change is published, and every generation reaches its expected state and its draw")
        let database = try await newStore("gantt-entry", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        let directory = scratch("view-entry")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("entry.jsonl")
        guard let log = MeasureLog(path: url.path, id: "entry", freeze: [], header: [:]) else { throw SuiteError(description: "the log could not be opened") }
        let ledger = Ledger()
        log.onEmit = { ledger.add($0, $1, $2) }
        MeasureLog.shared = log
        let steps: [(String, Bool, String)] = [(",", false, "collapse_all"), (".", false, "expand_all"), ("=", false, "zoom"), ("-", false, "zoom"),
                                               (KeyDecoder.right, true, "pan"), (KeyDecoder.left, true, "pan"), ("c", false, "filter"), ("x", false, "filter")]
        let ran = await MainActor.run { () -> (generations: [Int], rows: [Int], observed: [(sequence: Int, called: Bool)], drawn: [Bool], handled: [Bool]) in
            model.setViewport(rows: 20, width: 600)
            var observed: [(sequence: Int, called: Bool)] = []
            var last = model.gantt.sequence
            // The observer of the published mutation runs inside the assignment: the call must be in the log already.
            let watch = model.$gantt.sink { new in
                guard new.sequence != last else { return }
                last = new.sequence
                observed.append((new.sequence, ledger.has("view_op_call", generation: String(new.sequence))))
            }
            var generations: [Int] = [], rows: [Int] = [], drawn: [Bool] = [], handled: [Bool] = []
            for (characters, shift, _) in steps {
                handled.append(self.press(model, characters, shift: shift) != .ignored)
                let state = model.gantt
                generations.append(state.sequence)
                rows.append(model.outline.count)
                // The draw endpoint: the stamp the draw pass calls, with the facts the draw would read from this state.
                let shown = GanttFingerprint.facts(zoom: state.zoomLevel, offsetX: state.panX, offsetY: state.panY, filter: state.filter, collapsed: state.collapsed.count)
                drawn.append(log.stamp("view_op_drawn", key: "view_op:\(state.sequence)", gen: state.sequence, facts: shown.merging(["sequence": String(state.sequence), "rows": String(model.outline.count)]) { first, _ in first },
                                       surface: "gantt", detail: ["rows_drawn_model": model.outline.count], mismatch: "view_op_mismatch"))
            }
            watch.cancel()
            return (generations, rows, observed, drawn, handled)
        }
        MeasureLog.shared = nil
        log.close()
        check(ran.handled.allSatisfy { $0 } && ran.generations.count == 8, "every key reached the handler")
        check(Set(ran.generations).count == 8 && zip(ran.generations, ran.generations.dropFirst()).allSatisfy { $1 == $0 + 1 }, "each operation has its own generation, one after the other: \(ran.generations)")
        check(ran.observed.count == 8 && ran.observed.allSatisfy { $0.called }, "a synchronous observer of the published mutation already saw the call logged, for all \(ran.observed.count) generations: \(ran.observed.map { $0.called })")
        check(ran.drawn.allSatisfy { $0 }, "each generation's draw completed it once with the expected state")
        let all = ledger.all
        var ordered = true
        for generation in ran.generations {
            let name = String(generation)
            guard let call = all.firstIndex(where: { $0.event == "view_op_call" && $0.generation == name }), let state = all.firstIndex(where: { $0.event == "state_set" && $0.generation == name }),
                  let draw = all.firstIndex(where: { $0.event == "view_op_drawn" && $0.generation == name }) else { ordered = false; continue }
            if !(call < state && state < draw && all[call].time <= all[state].time && all[state].time <= all[draw].time) { ordered = false }
        }
        check(ordered, "for every generation the call, the expected state and the matching draw come in that order on one clock")
        check(ran.rows[0] == 100 && ran.rows[1] == 1100, "collapse-all leaves the 100 packages and expand-all the 1100 rows: \(ran.rows.prefix(2))")
        let written = try events(url)
        let calls = written.filter { $0["event"] as? String == "view_op_call" }
        let states = written.filter { $0["event"] as? String == "state_set" && ($0["detail"] as? [String: Any])?["kind"] as? String == "view_op" }
        check(calls.count == 8 && states.count == 8 && calls.map { ($0["detail"] as? [String: Any])?["op"] as? String ?? "" } == steps.map { $0.2 }, "the log holds the eight calls, in order, by operation: \(calls.map { ($0["detail"] as? [String: Any])?["op"] as? String ?? "" })")
        let zooms = states.compactMap { ($0["detail"] as? [String: Any])?["zoom"] as? String }
        check(zooms.count == 8 && zooms[3] == zooms[1] && Int(zooms[2]) == (Int(zooms[1]) ?? 0) + 1, "the expected state carries the applied zoom: \(zooms)")
        await closeGantt(model)
    }

    // MARK: F20-P5 the item's own estimate

    func ganttEstimatesArePresented() async throws {
        print("gantt: each item's own optimistic, likely and pessimistic hours, or their absence, are in the row words and in Detail, exactly as the plan supplies them")
        let database = try await newStore("gantt-estimates", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        guard case .object(let items) = try await cliJSON(database, ["export"])["data"]["work_items"] else { throw SuiteError(description: "the export has no work items") }
        var supplied: [String: (Double, Double, Double)] = [:]
        var absent: Set<String> = []
        for item in items.values {
            guard let key = item["key"].string else { continue }
            let estimate = item["schedule"]["estimate"]
            if let o = number(estimate["optimistic_hours"]), let l = number(estimate["likely_hours"]), let p = number(estimate["pessimistic_hours"]) { supplied[key] = (o, l, p) } else { absent.insert(key) }
        }
        check(supplied.count >= 3 && Set(supplied.values.map { "\($0)" }).count >= 3 && supplied.values.allSatisfy { $0.0 < $0.1 && $0.1 < $0.2 }, "the plan has several different three-point estimates, each with three distinct values: \(supplied)")
        check(!absent.isEmpty, "the plan has items without an estimate: \(absent.sorted())")
        let model = try await openGanttModel(.database(database))
        let (found, outline, inventory) = await MainActor.run { (model.snapshot.gantt, model.outline, model.snapshot.inventory) }
        guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
        var differences: [String] = []
        for item in outline {
            let words = GanttWords.row(item, schedule: schedule, inventory: inventory, selected: nil)
            if let (o, l, p) = supplied[item.row.key] {
                let expected = "own estimate: optimistic \(o) h, likely \(l) h, pessimistic \(p) h"
                if !words.value.contains(expected) { differences.append("\(item.row.key): \(words.value)") }
                if let held = inventory.byIdentity[item.id]?.estimate, held.optimisticHours != o || held.likelyHours != l || held.pessimisticHours != p { differences.append("\(item.row.key): held estimate differs") }
            } else if !words.value.contains("no duration estimate is recorded for this item") || words.value.contains("own estimate:") {
                differences.append("\(item.row.key) has no estimate and its words do not say so: \(words.value)")
            }
        }
        check(differences.isEmpty, "every row says its own estimate exactly as supplied, or its absence: \(differences)")
        if let item = outline.first(where: { supplied[$0.row.key] != nil && !$0.row.priority.isEmpty }) {
            let words = GanttWords.row(item, schedule: schedule, inventory: inventory, selected: nil)
            check(words.value.contains("human priority \(item.row.priority)") && words.value.contains("own estimate:") && !GanttWords.estimate(inventory.byIdentity[item.id]?.estimate).contains("priority"),
                  "the estimate is its own statement, apart from priority, float and criticality: \(words.value)")
        } else {
            check(false, "no estimated row carries a priority")
        }
        // The shared Detail path, from the application's explain.
        guard let estimated = supplied.keys.sorted().first, let bare = absent.sorted().first(where: { key in outline.contains { $0.row.key == key && $0.row.isMilestone } }) ?? absent.sorted().first,
              let withId = inventory.items.first(where: { $0.key == estimated })?.identity, let withoutId = inventory.items.first(where: { $0.key == bare })?.identity else { throw SuiteError(description: "no estimated and unestimated task to open") }
        await MainActor.run { model.select(.work(withId)) }
        let one = try await modelExpect(model, "the estimated task's detail", 30) { $0.detail?.subject == .work(withId) && $0.detail?.loading == false }
        let text = one.detail?.sections.first { $0.title.hasPrefix("Estimate") }?.rows.map(\.text).joined(separator: " | ") ?? ""
        if let (o, l, p) = supplied[estimated] {
            check(text.contains("optimistic \(o) h") && text.contains("likely \(l) h") && text.contains("pessimistic \(p) h"), "Detail of \(estimated) keeps all three exact values: \(text)")
        }
        await MainActor.run { model.select(.work(withoutId)) }
        let none = try await modelExpect(model, "the unestimated task's detail", 30) { $0.detail?.subject == .work(withoutId) && $0.detail?.loading == false }
        let absence = none.detail?.sections.first { $0.title.hasPrefix("Estimate") }?.rows.map(\.text).joined(separator: " | ") ?? ""
        check(absence.contains("no duration estimate is recorded for this item") && !absence.contains("optimistic"), "Detail of \(bare), which has no estimate, says so: \(absence)")
        await closeGantt(model)
    }

    // MARK: F20-P3 and F20-P4 the trajectory and the identity of scripted scroll requests

    func ganttScrollTrajectoryAndIdentity() async throws {
        print("gantt: a scripted scroll over finite content wraps deterministically, its request identities are unique across axes and windows, and a delayed stamp completes only its own window")
        // The drawing side and the driver's side are separate functions; they must agree across the wrap, and a zero range is not scrollable.
        var disagreements = 0, wraps = 0, previous = 0.0, unmoved = 0
        for (extent, viewport) in [(3080.0, 616.0), (1234.5678, 700.0), (80_000.25, 600.0), (900.0, 700.0)] {
            let range = ScrollGeometry.range(extent: extent, viewport: viewport)
            previous = 0
            for step in 1...400 {
                let logical = GanttDrive.step * Double(step)
                let drawn = ScrollGeometry.effectiveOffset(logical: logical, extent: extent, viewport: viewport)
                guard let expected = ScrollPlan.expectedOffset(distance: logical, range: range) else { disagreements += 1; continue }
                if Measure.format(drawn) != Measure.format(expected) || drawn < 0 || drawn >= range { disagreements += 1 }
                if drawn < previous { wraps += 1 }
                if drawn == previous { unmoved += 1 }
                previous = drawn
            }
        }
        check(disagreements == 0 && wraps >= 4 && unmoved == 0, "the drawn offsets equal the independently expected ones across \(wraps) wraps of the content extent (\(disagreements) disagreements, \(unmoved) requests that did not move it)")
        check(ScrollPlan.expectedOffset(distance: 40, range: 0) == nil && ScrollGeometry.effectiveOffset(logical: 40, extent: 500, viewport: 700) == 0, "content no larger than its viewport has no scroll range: nothing is expected and nothing is drawn as scrolled")
        check(ScrollGeometry.effectiveOffset(logical: 40, extent: 40, viewport: 0) == 0 && ScrollGeometry.effectiveOffset(logical: 80, extent: 40, viewport: 0) == 0, "a range that divides the step leaves the offset unchanged, which the analyzer refuses as movement")
        // Identity: window, axis and number.
        var keys = Set<String>()
        var total = 0
        for window in 1...3 {
            for axis in [GanttDrive.Axis.vertical, .horizontal] {
                for sequence in 1...50 { keys.insert(GanttDrive(axis: axis, window: window, sequence: sequence).key); total += 1 }
            }
        }
        check(keys.count == total && total == 300, "300 requests across two axes and three windows have 300 distinct identities (\(keys.count))")
        // A delayed stamp of an earlier window cannot complete a later one.
        let directory = scratch("scroll-identity")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("identity.jsonl")
        guard let log = MeasureLog(path: url.path, id: "identity", freeze: [], header: [:]) else { throw SuiteError(description: "the log could not be opened") }
        func facts(_ drive: GanttDrive, offset: Double) -> [String: String] {
            ["axis": drive.axis.rawValue, "window_id": String(drive.window), "seq": String(drive.sequence), "logical_distance": Measure.format(drive.logicalDistance),
             "extent": Measure.format(3080), "viewport": Measure.format(616), "offset": Measure.format(offset)]
        }
        let first = GanttDrive(axis: .vertical, window: 1, sequence: 1), second = GanttDrive(axis: .vertical, window: 2, sequence: 1), other = GanttDrive(axis: .horizontal, window: 3, sequence: 1)
        for drive in [first, second, other] { log.expect(drive.key, facts: facts(drive, offset: 40), record: ["expected_effective_offset": 40.0, "window_id": drive.window]) }
        let early = log.stamp("frame", key: first.key, gen: first.generation, facts: facts(first, offset: 40), surface: "gantt", mismatch: "frame_mismatch")
        let again = log.stamp("frame", key: first.key, gen: first.generation, facts: facts(first, offset: 40), surface: "gantt", mismatch: "frame_mismatch")
        let crossed = log.stamp("frame", key: second.key, gen: second.generation, facts: facts(first, offset: 40), surface: "gantt", mismatch: "frame_mismatch")
        check(early && !again && !crossed && !log.isComplete(second.key) && !log.isComplete(other.key), "a stamp completes its own window's request once; the same request in another window is not completed by it")
        let moved = log.stamp("frame", key: second.key, gen: second.generation, facts: facts(second, offset: 47), surface: "gantt", mismatch: "frame_mismatch")
        let right = log.stamp("frame", key: second.key, gen: second.generation, facts: facts(second, offset: 40), surface: "gantt", detail: ["geometry_drawn_offset": 40.0], mismatch: "frame_mismatch")
        check(!moved && right, "a draw at another coordinate than expected is refused with a mismatch, and the draw at the expected one completes it")
        log.close()
        let seen = try events(url)
        let mismatches = seen.filter { $0["event"] as? String == "frame_mismatch" }
        check(mismatches.count == 2 && mismatches.allSatisfy { ($0["detail"] as? [String: Any])?["expected_facts"] != nil && ($0["detail"] as? [String: Any])?["drawn_facts"] != nil }, "each refused draw is logged with the expected and the drawn facts: \(mismatches.count)")
        let completed = seen.filter { $0["event"] as? String == "frame" }
        let recorded = completed.map { ($0["detail"] as? [String: Any])?["expected_effective_offset"] as? Double }
        check(completed.count == 2 && recorded == [40.0, 40.0] && Set(completed.compactMap { $0["gen"] as? String }).count == 2, "the completed frames carry their expected offset and distinct identities: \(completed.compactMap { $0["gen"] as? String })")
    }

    // MARK: F20-P8 the content facts of BF4 and BF7

    func ganttContentFactsAreCompared() async throws {
        print("gantt: a draw of the right generation with the wrong zoom, pan or selection is refused, and the right state is accepted")
        let directory = scratch("content-facts")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("facts.jsonl")
        guard let log = MeasureLog(path: url.path, id: "facts", freeze: [], header: [:]) else { throw SuiteError(description: "the log could not be opened") }
        var filter = GanttFilter()
        filter.criticalOnly = true
        func view(zoom: Int, panX: Double, panY: Double, rows: Int) -> [String: String] {
            GanttFingerprint.facts(zoom: zoom, offsetX: panX, offsetY: panY, filter: filter, collapsed: 3).merging(["sequence": "7", "rows": String(rows)]) { first, _ in first }
        }
        log.expect("view_op:7", facts: view(zoom: 4, panX: 160, panY: 0, rows: 50))
        let wrongZoom = log.stamp("view_op_drawn", key: "view_op:7", gen: 7, facts: view(zoom: 3, panX: 160, panY: 0, rows: 50), surface: "gantt", mismatch: "view_op_mismatch")
        let wrongPan = log.stamp("view_op_drawn", key: "view_op:7", gen: 7, facts: view(zoom: 4, panX: 0, panY: 0, rows: 50), surface: "gantt", mismatch: "view_op_mismatch")
        let wrongFilter = log.stamp("view_op_drawn", key: "view_op:7", gen: 7, facts: GanttFingerprint.facts(zoom: 4, offsetX: 160, offsetY: 0, filter: GanttFilter(), collapsed: 3).merging(["sequence": "7", "rows": "50"]) { first, _ in first },
                                    surface: "gantt", mismatch: "view_op_mismatch")
        let right = log.stamp("view_op_drawn", key: "view_op:7", gen: 7, facts: view(zoom: 4, panX: 160, panY: 0, rows: 50), surface: "gantt", mismatch: "view_op_mismatch")
        check(!wrongZoom && !wrongPan && !wrongFilter && right, "the right row count with the wrong zoom, pan or filter completes nothing; the right state completes the generation")
        log.expect("commit:12", facts: ["revision": "12", "selected": "TEST-B"])
        let other = log.stamp("commit_drawn", key: "commit:12", gen: 12, facts: ["revision": "12", "selected": "TEST-C"], surface: "gantt", mismatch: "commit_mismatch")
        let none = log.stamp("commit_drawn", key: "commit:12", gen: 12, facts: ["revision": "12", "selected": "none"], surface: "gantt", mismatch: "commit_mismatch")
        let kept = log.stamp("commit_drawn", key: "commit:12", gen: 12, facts: ["revision": "12", "selected": "TEST-B"], surface: "gantt", mismatch: "commit_mismatch")
        check(!other && !none && kept, "the expected revision drawn with another or no selection completes nothing; with the selection kept it completes")
        log.close()
        let seen = try events(url)
        check(seen.filter { $0["event"] as? String == "view_op_mismatch" }.count == 3 && seen.filter { $0["event"] as? String == "commit_mismatch" }.count == 2, "every refused draw is a logged mismatch")
        // The model's own expectation names the task that was selected before the commit.
        let database = try await newStore("gantt-facts", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database))
        guard let identity = await MainActor.run(body: { model.snapshot.inventory.items.first { $0.key == "TEST-B" }?.identity }) else { throw SuiteError(description: "TEST-B has no identity") }
        let live = directory.appendingPathComponent("live.jsonl")
        guard let modelLog = MeasureLog(path: live.path, id: "live", freeze: [], header: [:]) else { throw SuiteError(description: "the log could not be opened") }
        MeasureLog.shared = modelLog
        await MainActor.run { model.select(.work(identity)) }
        _ = try await modelExpect(model, "the selection's detail", 30) { $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
        try await worker(database, ["claim", "TEST-A"])
        let after = try await currentRevision(database)
        _ = try await modelExpect(model, "the committed revision to be installed", 30) { $0.revision == after }
        MeasureLog.shared = nil
        modelLog.close()
        let commit = try events(live).first { $0["event"] as? String == "commit_seen" && ($0["gen"] as? Int).map { UInt64($0) } == after }
        check((commit?["detail"] as? [String: Any])?["selected"] as? String == "TEST-B", "the commit is expected with the task selected before it, TEST-B: \((commit?["detail"] as? [String: Any])?["selected"] as? String ?? "absent")")
        await closeGantt(model)
    }
}
