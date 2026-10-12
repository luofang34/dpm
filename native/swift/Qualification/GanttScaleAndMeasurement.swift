// The Gantt at scale and the instrument that measures it: a 1000-task plan with a dense overlay stays complete
// and operable, and the measurement log completes a generation only when a draw shows it, so a withheld draw
// is refused rather than counted.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    // MARK: A dense plan and a 1000-task plan

    func ganttDenseAndLargePlans() async throws {
        print("gantt: a 1000-task plan with the dense-DAG overlay stays complete and operable")
        let database = try await newStore("gantt-dense", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        let before = try await readOnlyRecord(database)
        let (found, inventory, outline) = await MainActor.run { (model.snapshot.gantt, model.snapshot.inventory, model.outline) }
        guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
        let packages = schedule.rows.filter(\.isPackage).count
        check(schedule.rows.count == 1100 && packages == 100, "1000 tasks and 100 packages are 1100 rows: \(schedule.rows.count) rows, \(packages) packages")
        let soft = inventory.relations.filter { $0.policy == "Soft" }.count
        check(inventory.relations.count == 1994 && soft == 249, "the overlay's edges are all here: \(inventory.relations.count) relations, \(soft) Soft")
        let incoming = Dictionary(grouping: inventory.relations, by: \.successor).mapValues(\.count)
        check(incoming.count == 997 && incoming.values.allSatisfy { $0 == 2 }, "997 tasks have two predecessors each, as the dense overlay defines (\(incoming.count) tasks have any)")
        check(outline.count == 1100 && outline.filter { $0.depth == 1 }.count == 1000, "expanded, every package and task is a row, with the tasks one level in: \(outline.count)")
        if let item = outline.first(where: { $0.depth == 1 && inventory.relations(of: $0.id).count >= 3 }) {
            let said = GanttWords.row(item, schedule: schedule, inventory: inventory, selected: nil)
            check(said.value.contains("relations: ") && said.value.components(separatedBy: " → ").count >= 3, "a task in the dense graph reads its relations in words: \(said.value.suffix(200))")
        } else {
            check(false, "no task with several relations was found in the dense plan")
        }
        // Operate it by keys: collapse everything, page through, jump to the end, expand.
        let collapsed = await MainActor.run { () -> Int in
            model.setViewport(rows: 20, width: 600)
            _ = self.press(model, ",")
            return model.outline.count
        }
        check(collapsed == 100, "collapsing every package leaves the 100 packages: \(collapsed)")
        let (paged, last) = await MainActor.run { () -> (String?, String?) in
            _ = self.press(model, KeyDecoder.pageDown)
            let paged = model.focusTarget
            _ = self.press(model, KeyDecoder.end)
            return (paged, model.focusTarget)
        }
        check(paged == "gantt.row.PERF-P20" && last == "gantt.row.PERF-P100", "page down moves one page of rows and end goes to the last: \(paged ?? "none"), \(last ?? "none")")
        let expanded = await MainActor.run { () -> Int in
            _ = self.press(model, ".")
            return model.outline.count
        }
        check(expanded == 1100, "expanding every package restores all 1100 rows: \(expanded)")
        // F20-UI-02, additive: after collapse-all, End, expand-all the cursor row is inside the visible range and among
        // the rows the view renders (the same range `GanttGeometry` gives the labels that carry the accessibility text:
        // from Int(panY / rowHeight), viewport rows + 2). A model-path check; the hosted view is not instantiated here.
        let (cursorIndex, firstRendered, renderedCount, visibleRows) = await MainActor.run { () -> (Int, Int, Int, Int) in
            let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
            let first = Int(max(0, model.gantt.panY) / GanttLayout.rowHeight)
            let drawn = max(0, min(model.outline.count - first, model.gantt.viewportRows + 2))
            return (at, first, drawn, model.gantt.viewportRows)
        }
        check(cursorIndex >= 0 && cursorIndex >= firstRendered && cursorIndex < firstRendered + min(renderedCount, visibleRows),
              "after collapse-all, End and expand-all the cursor row (index \(cursorIndex)) is inside the visible rows \(firstRendered)..<\(firstRendered + visibleRows) and among the \(renderedCount) rendered rows")
        let (criticalRows, sound) = await MainActor.run { () -> (Int, Bool) in
            _ = self.press(model, "c")
            let rows = model.outline
            let parents = Set(rows.compactMap(\.row.parent))
            return (rows.count, rows.allSatisfy { $0.row.span?.critical == true || parents.contains($0.id) })
        }
        check(criticalRows <= 1100 && sound, "critical-only keeps the critical rows and the packages that contain them (\(criticalRows) rows)")
        let after = try await readOnlyRecord(database)
        check(after == before, "none of it wrote anything: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: The measurement instrument refuses to complete a withheld draw

    func ganttMeasurementNegativeControl() async throws {
        print("gantt: the measurement log completes a generation only when it is drawn, and refuses when the draw is withheld")
        let directory = scratch("measure-control")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        func events(_ url: URL) throws -> [[String: Any]] {
            try String(contentsOf: url, encoding: .utf8).split(separator: "\n").compactMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any] }
        }
        func scenario(_ name: String, frozen: Set<String>) throws -> [[String: Any]] {
            let url = directory.appendingPathComponent("\(name).jsonl")
            guard let log = MeasureLog(path: url.path, id: "control-\(name)", freeze: frozen, header: ["hardware_model": "suite"]) else { throw SuiteError(description: "the log could not be opened") }
            for number in 0..<20 { log.emit("tick", detail: ["n": number]) }
            log.expect("view_op:1", facts: ["sequence": "1", "rows": "5"])
            log.emit("view_op_call", gen: 1, detail: ["op": "collapse_all", "expected_rows": 5])
            log.emit("state_set", gen: 1, detail: ["kind": "view_op"])
            // A draw of other content (an older state) completes nothing, whatever the gate.
            log.stamp("view_op_drawn", key: "view_op:1", gen: 1, facts: ["sequence": "0", "rows": "5"], surface: "gantt")
            log.stamp("view_op_drawn", key: "view_op:1", gen: 1, facts: ["sequence": "1", "rows": "5"], surface: "gantt", detail: ["rows_drawn_model": 5])
            log.stamp("first_draw", key: "never-expected", gen: "x", facts: [:], surface: "gantt")
            log.close()
            return try events(url)
        }
        let open = try scenario("open", frozen: [])
        let withheld = try scenario("withheld", frozen: ["gantt"])
        func count(_ list: [[String: Any]], _ name: String) -> Int { list.filter { $0["event"] as? String == name }.count }
        check(count(open, "view_op_drawn") == 1 && count(open, "view_op_call") == 1 && count(open, "tick") == 20 && count(open, "first_draw") == 0, "NC-0, the gate open: the generation completes once, and only for the content it expected")
        let drawn = open.first { $0["event"] as? String == "view_op_drawn" }
        let called = open.first { $0["event"] as? String == "view_op_call" }
        let drawnAt = drawn?["t_ns"] as? Int ?? 0
        let calledAt = called?["t_ns"] as? Int ?? Int.max
        check(drawnAt > calledAt && (drawn?["detail"] as? [String: Any])?["rows_drawn_model"] as? Int == 5, "its latency exists, is positive, and was measured on the app's own monotonic clock (\(drawnAt - calledAt) ns)")
        check(count(withheld, "view_op_drawn") == 0 && count(withheld, "first_draw") == 0, "NC-1, the gate closed: no drawn event exists")
        check(count(withheld, "tick") == 20 && count(withheld, "state_set") == 1 && count(withheld, "view_op_call") == 1, "yet the callbacks fired and the model advanced: 20 ticks, a state_set and the call are all there")
        let closing = withheld.last
        check(closing?["event"] as? String == "log_closed" && closing?["dropped"] as? Int == 0, "the log says how many events it dropped: none")
        // A full buffer drops and says so: such a log is invalid and never short in silence.
        let url = directory.appendingPathComponent("full.jsonl")
        guard let full = MeasureLog(path: url.path, id: "control-full", freeze: [], header: [:], capacity: 0) else { throw SuiteError(description: "the log could not be opened") }
        for number in 0..<10 { full.emit("tick", detail: ["n": number]) }
        full.close()
        let dropped = try events(url).last
        check(dropped?["event"] as? String == "log_closed" && dropped?["dropped"] as? Int == 10, "events beyond the bounded buffer are counted as dropped (\(dropped?["dropped"] as? Int ?? -1)), which makes a run invalid")
        // The model's own events: a collapse advances the model and logs, and completes nothing without a draw.
        let database = try await newStore("gantt-measure", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database))
        let modelLog = directory.appendingPathComponent("model.jsonl")
        guard let live = MeasureLog(path: modelLog.path, id: "control-model", freeze: [], header: [:]) else { throw SuiteError(description: "the log could not be opened") }
        MeasureLog.shared = live
        let sequence = await MainActor.run { () -> Int in
            _ = self.press(model, ",")
            _ = self.press(model, " ")
            return model.gantt.sequence
        }
        MeasureLog.shared = nil
        live.close()
        let seen = try events(modelLog)
        let completed = live.isComplete("view_op:\(sequence)")
        check(count(seen, "view_op_call") == 1 && count(seen, "state_set") >= 1 && count(seen, "select_call") == 1 && count(seen, "view_op_drawn") == 0 && !completed,
              "the model logs a view operation and a selection as called and applied, and with no draw pass neither is ever completed")
        await closeGantt(model)
    }
}
