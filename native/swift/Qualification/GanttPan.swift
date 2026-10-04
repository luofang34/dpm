// MODEL-PATH regressions: they call the same `pan` the Shift-arrow keys and the
// wheel call, and the same key handler, on the real MainActor model. They are NOT real keyboard, wheel or
// hosted-view tests; that real-path regression is NOT ASSESSED (UI permission pending).

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    /// F20-PAN-01: a pan survives a cursor that is scrolled out of view; zoom and horizontal pan leave the
    /// vertical pan and the cursor alone; an ordinary arrow brings the cursor back into view; a filter keeps
    /// a visible fallback row and the shared selection; nothing is written.
    func ganttPanSurvivesAnOffscreenCursor() async throws {
        print("gantt: [model path] a pan is kept when the cursor is off screen; zoom and horizontal pan leave the cursor and panY; membership still keeps the cursor visible")
        let database = try await newStore("gantt-pan", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        let before = try await readOnlyRecord(database)
        let step = 4.0 * GanttLayout.rowHeight

        func state() async -> (index: Int, panY: Double, panX: Double, zoom: Int, rows: Int, selection: Subject?) {
            await MainActor.run {
                let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
                return (at, model.gantt.panY, model.gantt.panX, model.gantt.zoomLevel, model.outline.count, model.selection)
            }
        }
        await MainActor.run { model.setViewport(rows: 20, width: 600) }

        // (a) Home, then pan down: panY grows by the step and stays; the cursor stays on row 0.
        await MainActor.run { _ = self.press(model, KeyDecoder.home) }
        let home = await state()
        await MainActor.run { model.pan(dx: 0, dy: step) }
        let down = await state()
        check(home.index == 0 && home.panY == 0 && down.panY == step && down.index == 0, "Home then pan down: panY \(home.panY) -> \(down.panY), cursor index \(down.index)")
        // The wheel path is the same call with a scroll delta.
        await MainActor.run { model.pan(dx: 0, dy: 30) }
        let wheel = await state()
        check(wheel.panY == step + 30 && wheel.index == 0, "a wheel delta pans on: panY \(wheel.panY), cursor index \(wheel.index)")

        // (c) Zoom and horizontal pan leave panY and the cursor untouched.
        await MainActor.run { model.pan(dx: 40, dy: 0) }
        let across = await state()
        await MainActor.run { _ = self.press(model, "+") }
        let zoomed = await state()
        check(across.panY == wheel.panY && across.index == 0 && zoomed.panY == wheel.panY && zoomed.index == 0,
              "horizontal pan and zoom leave panY \(wheel.panY) and the cursor: \(across.panY), \(zoomed.panY), index \(zoomed.index)")

        // (d) An ordinary cursor arrow makes its target row visible again.
        await MainActor.run { _ = self.press(model, KeyDecoder.down) }
        let arrowed = await MainActor.run { () -> (Int, Double, Int) in
            let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
            return (at, model.gantt.panY, model.gantt.viewportRows)
        }
        let top = Double(arrowed.0) * GanttLayout.rowHeight
        check(arrowed.0 == 1 && top >= arrowed.1 && top + GanttLayout.rowHeight <= arrowed.1 + Double(arrowed.2) * GanttLayout.rowHeight,
              "an arrow after the pan makes row \(arrowed.0) visible again: panY \(arrowed.1)")

        // End, then pan up: the pan stays, the cursor stays on the last row.
        await MainActor.run { _ = self.press(model, KeyDecoder.end) }
        let end = await state()
        await MainActor.run { model.pan(dx: 0, dy: -step) }
        let up = await state()
        check(end.index == end.rows - 1 && up.panY == end.panY - step && up.index == end.rows - 1, "End then pan up: panY \(end.panY) -> \(up.panY), cursor index \(up.index) of \(end.rows)")

        // (b) Membership operations still bring the cursor back (collapse-all, End, expand-all).
        await MainActor.run {
            _ = self.press(model, ",")
            _ = self.press(model, KeyDecoder.end)
            _ = self.press(model, ".")
        }
        let reopened = await MainActor.run { () -> (Int, Double, Int) in
            let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
            return (at, model.gantt.panY, model.gantt.viewportRows)
        }
        let rowTop = Double(reopened.0) * GanttLayout.rowHeight
        check(reopened.0 >= 0 && rowTop >= reopened.1 && rowTop + GanttLayout.rowHeight <= reopened.1 + Double(reopened.2) * GanttLayout.rowHeight,
              "expand-all keeps the cursor row \(reopened.0) inside the viewport at panY \(reopened.1)")

        // (e) A filter that removes the cursor row leaves a visible fallback row and the shared selection.
        let selected = await state().selection
        await MainActor.run { _ = self.press(model, "c") }
        let filtered = await MainActor.run { () -> (Int, Double, Int, Subject?) in
            let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
            return (at, model.gantt.panY, model.gantt.viewportRows, model.selection)
        }
        let filteredTop = Double(filtered.0) * GanttLayout.rowHeight
        check(filtered.0 >= 0 && filteredTop >= filtered.1 && filteredTop + GanttLayout.rowHeight <= filtered.1 + Double(filtered.2) * GanttLayout.rowHeight && filtered.3 == selected,
              "the filter leaves a visible fallback row \(filtered.0) and the shared selection unchanged")
        await MainActor.run { _ = self.press(model, "c") }

        // (e2) The removed-cursor path, chosen from the real query: the critical-only filter keeps the critical rows
        // and the packages that contain them. Put the cursor on the last row of the full outline that the filter drops,
        // then apply the filter through the shared key handler and clear it again. The selection is whatever this case
        // carries (none); that a real selected task persists through filter and clear is covered in Gantt.swift.
        struct Cursor { var id: String?; var index: Int; var visible: Bool; var rendered: Bool; var rows: Int; var selection: Subject? }
        func cursor() async -> Cursor {
            await MainActor.run {
                let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
                let top = Double(at) * GanttLayout.rowHeight
                let panY = model.gantt.panY
                let height = Double(model.gantt.viewportRows) * GanttLayout.rowHeight
                // The rendered rows are the range the view gives the labels that carry the accessibility text.
                let first = Int(max(0, panY) / GanttLayout.rowHeight)
                let drawn = max(0, min(model.outline.count - first, model.gantt.viewportRows + 2))
                return Cursor(id: model.gantt.focus, index: at, visible: at >= 0 && top >= panY && top + GanttLayout.rowHeight <= panY + height,
                              rendered: at >= first && at < first + drawn, rows: model.outline.count, selection: model.selection)
            }
        }
        let plan = await MainActor.run { () -> (all: [String], kept: Set<String>)? in
            guard let schedule = model.snapshot.gantt else { return nil }
            var critical = GanttFilter()
            critical.criticalOnly = true
            let everything = GanttLayout.outline(schedule, collapsed: [], filter: GanttFilter()).map(\.id)
            return (everything, Set(GanttLayout.outline(schedule, collapsed: [], filter: critical).map(\.id)))
        }
        guard let plan, let dropped = plan.all.last(where: { !plan.kept.contains($0) }) else {
            check(false, "the dense fixture has a row the critical-only filter drops, to put the cursor on")
            await closeGantt(model)
            return
        }
        let selectionBefore = await state().selection
        await MainActor.run { model.focusRow(dropped, selecting: false) }
        let placed = await cursor()
        check(placed.id == dropped && placed.index >= 0 && placed.visible && placed.rendered && placed.rows == plan.all.count && placed.selection == selectionBefore,
              "before the filter the cursor is on \(dropped), a member of the \(placed.rows) rows, visible and rendered (index \(placed.index))")
        await MainActor.run { _ = self.press(model, "c") }
        let narrowed = await cursor()
        let narrowedIDs = await MainActor.run { Set(model.outline.map(\.id)) }
        check(!narrowedIDs.contains(dropped) && narrowedIDs == plan.kept && narrowed.rows == plan.kept.count,
              "with the filter on, the old cursor row \(dropped) is not among the filtered rows (\(narrowed.rows) of \(plan.all.count), the query's own set)")
        check(narrowed.id != dropped && narrowed.id.map(narrowedIDs.contains) == true && narrowed.visible && narrowed.rendered,
              "the fallback cursor row \(narrowed.id ?? "none") is a filtered row, inside the viewport and among the rendered rows (index \(narrowed.index))")
        check(narrowed.selection == selectionBefore, "the filter left the shared selection as it was")
        await MainActor.run { _ = self.press(model, "c") }
        let restored = await cursor()
        check(restored.rows == plan.all.count && restored.id.map(plan.all.contains) == true && restored.visible && restored.rendered && restored.selection == selectionBefore,
              "clearing the filter restores all \(restored.rows) rows and the current cursor row \(restored.id ?? "none") is visible and rendered")

        // (f) Nothing was written.
        let after = try await readOnlyRecord(database)
        check(after == before, "none of it wrote anything: before \(before), after \(after)")
        await closeGantt(model)
    }

    /// The finish and p50 / p80 / p95 labels of the shared drawing path: close markers never overlap, far
    /// markers keep every label, and the label text is exactly what the drawing passes.
    func ganttAxisLabelsDoNotCollide() async throws {
        print("gantt: the forecast labels of the shared drawing function do not overlap, and keep their exact text")
        let far = GanttAxisLabels.place([(x: 10, text: "finish 120 h"), (x: 300, text: "p50"), (x: 400, text: "p80"), (x: 500, text: "p95")], width: 800)
        check(far.map(\.text) == ["finish 120 h", "p50", "p80", "p95"], "far markers keep all four labels: \(far.map(\.text))")
        let close = GanttAxisLabels.place([(x: 100, text: "finish 120 h"), (x: 104, text: "p50"), (x: 110, text: "p80"), (x: 118, text: "p95")], width: 800)
        check(close.map(\.text) == ["finish 120 h"], "close markers omit the labels that would touch: \(close.map(\.text))")
        let mixed = GanttAxisLabels.place([(x: 100, text: "finish 120 h"), (x: 104, text: "p50"), (x: 400, text: "p80"), (x: 405, text: "p95")], width: 800)
        check(mixed.map(\.text) == ["finish 120 h", "p80"], "a label that fits is kept beside one that does not: \(mixed.map(\.text))")
        for group in [far, close, mixed] {
            let sorted = group.sorted { $0.x < $1.x }
            check(zip(sorted, sorted.dropFirst()).allSatisfy { $0.maxX <= $1.minX }, "no two placed label rectangles overlap: \(sorted.map(\.x))")
        }
        let outside = GanttAxisLabels.place([(x: -5, text: "p50"), (x: 900, text: "p80")], width: 800)
        check(outside.isEmpty, "markers outside the chart have no label")
    }
}
