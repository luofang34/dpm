// The Gantt's keyboard and view state, qualified through the real helper and the real model: real `NSEvent`
// values reduced by `KeyInput(event:)` and applied by `ObserverModel.handleKey`, the one selection kept through
// page changes and reopening, and the view kept while the schedule is hidden, stale or lost. Each flow is shown
// read-only by the plan's revision, its history count and the store's bytes, taken before and after.

import DPMNative
import DPMObserverCore
import Foundation

/// Everything a step of a flow asserts, read from the model in one turn of the main actor.
struct GanttSnap: Sendable {
    var focus: String?
    var selection: Subject?
    var page: ObserverModel.Page
    var rows: Int
    var zoom: Int
    var panX: Double
    var panY: Double
    var criticalOnly: Bool
    var status: String?
    var editing: Bool
    var origin: ObserverModel.DetailReturn?
}

@MainActor
private func snap(_ model: ObserverModel) -> GanttSnap {
    GanttSnap(focus: model.focusTarget, selection: model.selection, page: model.page, rows: model.outline.count, zoom: model.gantt.zoomLevel, panX: model.gantt.panX, panY: model.gantt.panY,
              criticalOnly: model.gantt.filter.criticalOnly, status: model.gantt.filter.status, editing: model.gantt.editingFilter, origin: model.detailReturn)
}

extension Context {
    // MARK: The keyboard flow, through the shared handler

    func ganttKeysFlow() async throws { try await keyFlow(disabled: false) }

    /// Must fail: the same flow with key handling disabled. The packaged qualification runs it by name and
    /// requires a nonzero exit, so a flow that passes without keys cannot return unnoticed.
    func ganttNegativeControl() async throws { try await keyFlow(disabled: true) }

    private func keyFlow(disabled: Bool) async throws {
        print(disabled ? "gantt: negative control, the key flow with key handling disabled must fail" : "gantt: real key events through the shared handler: move, expand, collapse, select, Detail, return, zoom, pan, filter, and nothing written")
        let database = try await newStore(disabled ? "gantt-control" : "gantt-keys", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let work = try await cliJSON(database, ["--clock", tools.clock, "schedule"])["data"]["work"].array ?? []
        let order = expectedOrder(work)
        guard let packageAt = order.firstIndex(of: "TEST-PKG"), packageAt + 3 < order.count else { throw SuiteError(description: "the plan has no package with children: \(order)") }
        let model = try await openGanttModel(.database(database))
        let before = try await readOnlyRecord(database)
        let first = await MainActor.run { model.snapshot }
        guard let nested = identity(of: order[packageAt + 1], in: first), let sibling = identity(of: order[packageAt + 2], in: first) else { throw SuiteError(description: "no identity for the nested tasks") }
        let nestedKey = order[packageAt + 1]
        func key(_ characters: String, shift: Bool = false, command: Bool = false) async -> KeyOutcome {
            await MainActor.run { self.press(model, characters, shift: shift, command: command, disabled: disabled) }
        }
        func now() async -> GanttSnap { await MainActor.run { snap(model) } }
        await MainActor.run { model.setViewport(rows: 3, width: 100) }
        var state = await now()
        check(state.focus == "gantt.row.\(order[0])", "the cursor starts on the first row, \(order[0]): \(state.focus ?? "none")")
        // Arrow movement down to the package, then in: a package that is open moves the cursor to its first child.
        var moves: [KeyOutcome] = []
        for _ in 0..<packageAt { moves.append(await key(KeyDecoder.down)) }
        state = await now()
        check(state.focus == "gantt.row.TEST-PKG" && moves.allSatisfy { $0 == .handled(.moveDown) }, "down arrow moves the cursor to the package: \(state.focus ?? "none")")
        var outcome = await key(KeyDecoder.right)
        state = await now()
        check(outcome == .handled(.expand) && state.focus == "gantt.row.\(nestedKey)", "right arrow on an open package goes in to its first child, a nested task: \(state.focus ?? "none")")
        // Select it: the shared selection, and its detail is read from the application.
        outcome = await key(" ")
        state = await now()
        check(outcome == .handled(.select) && state.selection == .work(nested), "space selects the nested task in the shared selection")
        if state.selection == .work(nested) {
            _ = try await modelExpect(model, "the selected task's detail", 30) { $0.detail?.subject == .work(nested) && $0.detail?.loading == false }
        }
        // Left on a leaf goes to its parent; left on an open package collapses it, and the selection stays on a hidden row.
        let full = state.rows
        outcome = await key(KeyDecoder.left)
        state = await now()
        check(outcome == .handled(.collapse) && state.focus == "gantt.row.TEST-PKG", "left arrow on a leaf returns to its package: \(state.focus ?? "none")")
        outcome = await key(KeyDecoder.left)
        state = await now()
        check(outcome == .handled(.collapse) && state.rows == full - 3 && state.focus == "gantt.row.TEST-PKG" && state.selection == .work(nested),
              "left arrow on an open package collapses it: its three children are hidden, the cursor stays, and the selection persists on the hidden row (\(state.rows) of \(full) rows)")
        outcome = await key(KeyDecoder.right)
        state = await now()
        check(outcome == .handled(.expand) && state.rows == full && state.selection == .work(nested), "right arrow on a collapsed package expands it and the selection is as it was")
        // Open Detail from the row, change the selection as a followed link does, and close: focus returns to the row.
        _ = await key(KeyDecoder.down)
        let rowBefore = (await now()).focus
        outcome = await key("\r")
        state = await now()
        check(outcome == .handled(.openDetail) && state.page == .detail && state.origin?.element == rowBefore && state.origin?.page == .gantt, "return opens Detail and it remembers the row that opened it: \(String(describing: state.origin))")
        if state.page == .detail {
            _ = try await modelExpect(model, "the opened row's detail", 30) { $0.detail?.loading == false && $0.detail != nil }
            await MainActor.run { model.select(.work(sibling)) }
        }
        outcome = await key("\u{1B}")
        state = await now()
        check(outcome == .handled(.closeDetail) && state.page == .gantt && state.focus == rowBefore, "escape closes Detail and returns focus to the row that opened it, after a followed link: page \(state.page), focus \(state.focus ?? "none")")
        check(state.selection == .work(sibling), "the selection is the followed link's, not reset by returning")
        // The schedule was not displayed while Detail was; once it is read again the cursor is still on the row.
        if state.page == .gantt { _ = try await modelExpect(model, "the rows to be read again after Detail closed", 30) { $0.gantt != nil } }
        state = await now()
        check(state.focus == rowBefore, "the cursor is still on the row that opened Detail once the rows are back: \(state.focus ?? "none")")
        // Zoom and pan: only the view changes.
        let zoom0 = state.zoom
        for _ in 0..<3 { _ = await key("=") }
        state = await now()
        let zoomed = state.zoom
        check(zoomed == min(GanttLayout.zoomSteps.count - 1, zoom0 + 3) && zoomed > zoom0, "the plus key zooms in by one level each press: \(zoom0) to \(zoomed)")
        _ = await key("-")
        state = await now()
        check(state.zoom == zoomed - 1, "the minus key zooms out")
        // Back to the origin first, so the pan has room to move on both axes whatever the cursor did to it.
        for _ in 0..<3 {
            _ = await key(KeyDecoder.up, shift: true)
            _ = await key(KeyDecoder.left, shift: true)
        }
        state = await now()
        let panBefore = (state.panX, state.panY)
        _ = await key(KeyDecoder.right, shift: true)
        _ = await key(KeyDecoder.down, shift: true)
        state = await now()
        check(state.panX > panBefore.0 && state.panY > panBefore.1, "shift with the arrows pans along both axes: \(panBefore) to \((state.panX, state.panY))")
        check(state.selection == .work(sibling), "zoom and pan left the selection alone")
        // Filter: critical only, status, clear; the selection survives a filter that hides its row.
        let all = state.rows
        outcome = await key("c")
        state = await now()
        check(outcome == .handled(.toggleCritical) && state.criticalOnly && state.rows <= all, "c toggles the critical-only filter, which keeps no more rows than there are (\(state.rows) of \(all))")
        outcome = await key("x")
        state = await now()
        check(outcome == .handled(.clearFilter) && !state.criticalOnly && state.rows == all, "x clears the filter and every row is back")
        outcome = await key("s")
        state = await now()
        check(outcome == .handled(.cycleStatus) && state.status != nil, "s cycles the status filter: \(state.status ?? "none")")
        _ = await key("x")
        outcome = await key("/")
        state = await now()
        check(outcome == .handled(.focusFilter) && state.focus == "gantt.filter" && state.editing, "slash moves focus to the filter field: \(state.focus ?? "none")")
        outcome = await key("c")
        state = await now()
        check(outcome == .ignored && !state.criticalOnly, "while the filter field has the keyboard, a letter is text and not a command")
        outcome = await key("\u{1B}")
        state = await now()
        check(outcome == .handled(.closeDetail) && state.focus?.hasPrefix("gantt.row.") == true && !state.editing, "escape leaves the filter field and focus returns to a row: \(state.focus ?? "none")")
        check(state.selection == .work(sibling), "the selection persisted through every filter")
        // Collapse and expand every package.
        _ = await key(",")
        let collapsed = (await now()).rows
        _ = await key(".")
        let reopened = (await now()).rows
        check(collapsed < reopened && reopened == full, "comma collapses every package and period expands them again: \(collapsed) then \(reopened) rows")
        // Nothing was written, as the real helper and the disk say.
        let after = try await readOnlyRecord(database)
        check(after == before, "no revision, history entry or store byte changed through any of it: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: Selection through page changes and reopening

    func ganttSelectionPersists() async throws {
        print("gantt: the one shared selection persists through page changes, collapse, filter and a reopened workspace")
        let database = try await newStore("gantt-selection", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database))
        let first = await MainActor.run { model.snapshot }
        guard let identity = identity(of: "TEST-B", in: first) else { throw SuiteError(description: "TEST-B has no identity") }
        await MainActor.run { model.focusRow(identity) }
        var state = await MainActor.run { snap(model) }
        check(state.selection == .work(identity) && state.focus == "gantt.row.TEST-B", "choosing a row selects it and puts the cursor on it")
        _ = try await modelExpect(model, "the detail of TEST-B", 30) { $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
        for page in [ObserverModel.Page.now, .detail, .live, .gantt] {
            await MainActor.run { model.show(page) }
            state = await MainActor.run { snap(model) }
            check(state.selection == .work(identity), "the selection survives showing \(page.rawValue)")
        }
        _ = try await modelExpect(model, "the rows to be read again after the page returned", 30) { $0.gantt != nil }
        await MainActor.run {
            _ = self.press(model, ",")
            _ = self.press(model, "c")
        }
        state = await MainActor.run { snap(model) }
        check(state.selection == .work(identity), "collapsing every package and filtering to the critical path leave the selection")
        await MainActor.run { model.open(.database(database)) }
        _ = try await modelExpect(model, "the reopened workspace with the selection restored", 60) { $0.connection == .connected && $0.detail?.subject == .work(identity) && $0.detail?.loading == false && $0.gantt != nil }
        state = await MainActor.run { snap(model) }
        check(state.selection == .work(identity), "reopening the same workspace restores the selection by its persistent identity, with the Gantt read again")
        await closeGantt(model)
    }

    // MARK: The schedule is displayed only while the Gantt is shown

    func ganttScheduleLeavesTheDisplayedSet() async throws {
        print("gantt: the schedule is a displayed view, judged for freshness, only while the Gantt is the page shown")
        let database = try await newStore("gantt-slots", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let reads = Tally()
        try await withObserver(.database(database), clock: pinned, adjust: { settings in
            settings.fault = { query in
                if query["query"].string == "schedule" { reads.bump() }
                return nil
            }
        }) { engine, recorder, first in
            check(first.gantt == nil && !first.inspection.views.contains { $0.slot == "Schedule (Gantt)" } && reads.count == 0, "a page that is not the Gantt reads no schedule and displays none")
            await engine.showSchedule(true)
            let shown = try await recorder.expect("the schedule to be displayed") { $0.gantt != nil && $0.inspection.views.contains { $0.slot == "Schedule (Gantt)" } }
            check((shown.gantt?.rows.count ?? 0) > 0 && reads.count >= 1, "shown, the schedule is read and listed under the observation basis (\(shown.gantt?.rows.count ?? 0) rows, \(reads.count) reads)")
            let settled = try await recorder.expect("the client to be current with the schedule shown") { $0.gantt != nil && $0.freshness.current }
            check(settled.freshness.current, "with the schedule shown the client is current")
            await engine.showSchedule(false)
            let hidden = try await recorder.expect("the schedule to leave the displayed set") { $0.gantt == nil && !$0.inspection.views.contains { $0.slot == "Schedule (Gantt)" } }
            check(hidden.gantt == nil, "not shown, it leaves the displayed set")
            let beforeCommit = reads.count
            try await worker(database, ["claim", "TEST-A"])
            _ = try await recorder.expect("the external commit to be seen and read", 30) { ($0.revision ?? 0) > (first.revision ?? 0) && $0.freshness.current && $0.inventory.byIdentity.values.contains { $0.key == "TEST-A" && $0.status == "Claimed" } }
            check(reads.count == beforeCommit, "an external commit while the Gantt is hidden causes no schedule read (\(beforeCommit) reads before, \(reads.count) after)")
            await engine.showSchedule(true)
            let back = try await recorder.expect("the schedule to be read again at the new revision", 30) { $0.gantt != nil && $0.gantt?.revision == $0.revision }
            let claimed = back.gantt?.rows.first { $0.key == "TEST-A" }?.status
            check(claimed == "Claimed" && reads.count > beforeCommit, "shown again, it is read again and carries the committed change: TEST-A is \(claimed ?? "missing")")
        }
    }

    // MARK: Stale and disconnected, with the last view kept

    func ganttKeepsItsViewWhenStaleOrLost() async throws {
        print("gantt: a lost helper and a changed revision mark the data and keep the last view, the selection and the cursor")
        let database = try await newStore("gantt-stale", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database)) { $0.reconnectDelays = [0.4, 0.6] }
        let (rows, found) = await MainActor.run { () -> (Int, String?) in (model.outline.count, model.snapshot.inventory.items.first { $0.key == "TEST-B" }?.identity) }
        guard let identity = found else { throw SuiteError(description: "TEST-B has no identity") }
        await MainActor.run { model.focusRow(identity) }
        guard let pid = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "the snapshot names no helper process") }
        kill(pid, SIGKILL)
        let lost = try await modelExpect(model, "the lost helper to be shown", 30) { if case .reconnecting = $0.connection { return true } else { return false } }
        check(lost.gantt != nil && lost.gantt?.rows.count == rows && !lost.freshness.current, "while the helper is lost the last schedule is still held, and the client is not current (\(lost.gantt?.rows.count ?? -1) rows)")
        var state = await MainActor.run { snap(model) }
        check(state.rows == rows && state.selection == .work(identity) && state.focus == "gantt.row.TEST-B", "the drawn rows, the selection and the cursor are kept through the loss")
        _ = try await modelExpect(model, "the helper to be replaced and the view to be current", 60) { $0.connection == .connected && $0.freshness.current && $0.gantt != nil && $0.counters.reconnects >= 1 }
        state = await MainActor.run { snap(model) }
        check(state.rows == rows && state.selection == .work(identity), "after recovery the marks clear and the rows and selection are as they were")
        try await worker(database, ["claim", "TEST-A"])
        let changed = try await modelExpect(model, "the claimed task to appear in the schedule", 30) { $0.gantt?.rows.first { $0.key == "TEST-A" }?.status == "Claimed" && $0.freshness.current }
        check(changed.gantt?.revision == changed.revision, "an external commit is followed by a schedule read at the same revision")
        await closeGantt(model)
    }

    // MARK: The filter field's focus (model path only)

    /// F20-UI-01. MODEL-PATH test: it drives `filterFieldFocus` and `handleKey` on the model. It is NOT real keyboard
    /// coverage: no hosted SwiftUI view, key window, event monitor or `FocusState` transition is involved; that
    /// real-path regression is NOT ASSESSED here.
    func ganttFilterFocusLossDoesNotRequestTheRows() async throws {
        print("gantt: [model path] leaving the filter field without Escape names no row; Escape and a commit still do")
        let database = try await newStore("gantt-filter-focus", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        let (typing, left, afterLoss) = await MainActor.run { () -> (String?, String?, String?) in
            _ = self.press(model, "/")
            let typing = model.focusTarget
            model.filterFieldFocus(false)   // Tab or Shift-Tab: focus lost, not Escape, not a commit
            return (typing, model.focusTarget, model.focusTarget)
        }
        check(typing == "gantt.filter" && left == nil && afterLoss == nil && !(left ?? "").hasPrefix("gantt.row."), "focus lost from the filter by Tab names no row: \(typing ?? "none") then \(left ?? "none")")
        let escaped = await MainActor.run { () -> String? in
            _ = self.press(model, "/")
            _ = self.press(model, "\u{1B}")
            return model.focusTarget
        }
        check(escaped?.hasPrefix("gantt.row.") == true, "Escape from the filter returns the keyboard to the cursor row: \(escaped ?? "none")")
        await closeGantt(model)
    }
}
