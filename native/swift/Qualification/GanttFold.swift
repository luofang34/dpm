// Folding one package by its disclosure leaves every row above it where it was, whether the cursor is on
// the package or was scrolled off screen by a wheel pan (F20-PAN-01 keeps the cursor where it is).
import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func ganttFoldKeepsRowsAbove() async throws {
        let database = try await newStore("gantt-fold", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        await MainActor.run { model.setViewport(rows: 20, width: 600) }
        // A package well below the top, with children, expanded.
        let pkg = await MainActor.run { model.outline.first { $0.position >= 40 && $0.children > 0 && $0.expanded } }
        guard let pkg else { check(false, "no package"); return }
        let h = GanttLayout.rowHeight
        // Case A: cursor on screen at the package (as a row click would put it), fold by chevron.
        await MainActor.run { model.focusRow(pkg.id, selecting: false); model.pan(dx: 0, dy: Double(pkg.position - 5) * h - model.gantt.panY) }
        let a0 = await MainActor.run { model.gantt.panY }
        await MainActor.run { model.toggleDisclosure(pkg.id) }
        let a1 = await MainActor.run { model.gantt.panY }
        print("gantt fold: A cursor on package: panY \(a0) -> \(a1), package position \(pkg.position)")
        check(a0 == a1, "A: fold with cursor on screen keeps panY \(a0) -> \(a1)")
        await MainActor.run { model.toggleDisclosure(pkg.id) }
        // Case B: cursor on row 0 (Home), wheel-pan down so the package is in view, then fold it by its chevron.
        _ = await MainActor.run { self.press(model, KeyDecoder.home) }
        await MainActor.run { model.pan(dx: 0, dy: Double(pkg.position - 5) * h) }
        let b0 = await MainActor.run { (model.gantt.panY, model.gantt.focus ?? "nil") }
        await MainActor.run { model.toggleDisclosure(pkg.id) }
        let b1 = await MainActor.run { (model.gantt.panY, model.gantt.focus ?? "nil") }
        print("gantt fold: B cursor off screen at row 0: panY \(b0.0) -> \(b1.0), cursor \(b0.1) -> \(b1.1)")
        check(b0.0 == b1.0, "B: fold with cursor scrolled off screen keeps panY \(b0.0) -> \(b1.0)")
        await closeGantt(model)
    }
}
