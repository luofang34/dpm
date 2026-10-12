// How the shared model carries the Gantt: the rows it draws, the keyboard cursor, the view operations
// and the one place a key press is applied. The app's window and the qualification suite both end in
// `handleKey`, so there is one path from a key to the model and to the focus it asks for.
//
// Every operation here changes what is drawn and nothing else. None of them can reach the project:
// the only call that leaves this file is `select`, the same shared selection every page uses, which
// makes a read. Collapse, filter, zoom and pan never touch the selection, so it persists through them,
// through a page change and through Detail.

import Foundation

extension ObserverModel {
    /// Points one pan key moves along each axis.
    static let panStepX = 160.0
    static let panStepRows = 4.0

    // MARK: The rows

    /// The rows drawn now, in plan hierarchy order, after collapse and filter.
    public var outline: [GanttOutlineRow] { outlined().rows }

    /// Where each drawn row is, by identity; for drawing a relation between two rows.
    public var outlinePositions: [String: Int] { outlined().positions }

    private func outlined() -> (rows: [GanttOutlineRow], positions: [String: Int]) {
        guard let schedule = snapshot.gantt else { return ([], [:]) }
        if let cached = outlineCache, cached.token == schedule.token, cached.collapsed == gantt.collapsed, cached.filter == gantt.filter { return (cached.rows, cached.positions) }
        let rows = GanttLayout.outline(schedule, collapsed: gantt.collapsed, filter: gantt.filter)
        let positions = Dictionary(rows.map { ($0.id, $0.position) }, uniquingKeysWith: { first, _ in first })
        outlineCache = (schedule.token, gantt.collapsed, gantt.filter, rows, positions)
        return (rows, positions)
    }

    public var focusedRow: GanttOutlineRow? {
        guard let id = gantt.focus else { return nil }
        return outline.first { $0.id == id }
    }

    /// The identifier of a row as an element: its key reads the same to a person and to a script.
    public static func element(forRow key: String) -> String { "gantt.row.\(key)" }

    /// A new schedule reading, or any other snapshot, was installed.
    func installed(_ new: ObserverSnapshot) {
        if let log = MeasureLog.shared {
            if let revision = new.revision, revision != measuredRevision {
                measuredRevision = revision
                // The task selected before the commit is the task its draw must show as selected.
                log.expect("commit:\(revision)", facts: ["revision": String(revision), "selected": Self.measuredSelection(selection, in: new)])
                log.emit("commit_seen", gen: revision, detail: ["revision": revision, "selected": Self.measuredSelection(selection, in: new)])
            }
            if let detail = new.detail, !detail.loading, selectionCount > detailAssignedFor, detail.subject == selection {
                detailAssignedFor = selectionCount
                log.emit("state_set", gen: selectionCount, detail: ["kind": "selection", "sections": detail.sections.count])
            }
            if let schedule = new.gantt, schedule.token != measuredSchedule {
                measuredSchedule = schedule.token
                log.emit("state_set", gen: log.id, detail: ["kind": "schedule", "rows": schedule.rows.count, "revision": schedule.revision.map { Int($0) } ?? -1])
            }
        }
        guard new.gantt != nil else { return }
        reconcileFocus()
        if page == .gantt, focusTarget == nil, !focusLeftFilter { updateFocusTarget() }
    }

    /// The task a Gantt draw shows as selected, by its key, as a measurement names it; `none` when the
    /// selection is not a task (the Gantt marks a task only).
    public static func measuredSelection(_ selection: Subject?, in snapshot: ObserverSnapshot) -> String {
        guard case .work(let identity)? = selection else { return "none" }
        return snapshot.inventory.key(of: identity) ?? identity
    }

    /// Keep the cursor on a row that is drawn: its nearest drawn ancestor, else the selected row, else the first.
    func reconcileFocus() {
        guard let schedule = snapshot.gantt else { return }
        let rows = outline
        guard !rows.isEmpty else {
            if gantt.focus != nil { gantt.focus = nil }
            return
        }
        let visible = Set(rows.map(\.id))
        if let id = gantt.focus, visible.contains(id) { return }
        var found: String?
        var cursor = gantt.focus.flatMap { schedule.row($0)?.parent }
        var hops = 0
        while let up = cursor, hops < schedule.rows.count {
            if visible.contains(up) { found = up; break }
            cursor = schedule.row(up)?.parent
            hops += 1
        }
        if found == nil, case .work(let selected)? = selection, visible.contains(selected) { found = selected }
        gantt.focus = found ?? rows[0].id
    }

    /// The element the keyboard is on, from the cursor and from whether the filter field has the keyboard.
    func updateFocusTarget() {
        guard page == .gantt else { return }
        focusLeftFilter = false
        if gantt.editingFilter {
            focusTarget = "gantt.filter"
        } else if let row = focusedRow {
            focusTarget = Self.element(forRow: row.row.key)
        } else {
            focusTarget = nil
        }
    }

    // MARK: Keys

    /// The one place a key press is applied, from the window and from the suite alike. It decodes the
    /// press, applies its intent to the model, and leaves the element that should hold focus in `focusTarget`.
    @discardableResult
    public func handleKey(_ input: KeyInput) -> KeyOutcome {
        guard let intent = KeyDecoder.intent(for: input), accepts(intent) else { return .ignored }
        perform(intent)
        return .handled(intent)
    }

    private func accepts(_ intent: GanttIntent) -> Bool {
        switch page {
        case .gantt:
            if gantt.editingFilter { return intent == .closeDetail }
            if intent == .closeDetail { return false }
            return snapshot.gantt != nil || intent == .focusFilter
        case .detail:
            return intent == .closeDetail && canReturn
        default:
            return false
        }
    }

    public func perform(_ intent: GanttIntent) {
        switch intent {
        case .moveUp: moveFocus(by: -1)
        case .moveDown: moveFocus(by: 1)
        case .pageUp: moveFocus(by: -max(1, gantt.viewportRows - 1))
        case .pageDown: moveFocus(by: max(1, gantt.viewportRows - 1))
        case .first: moveFocus(to: 0)
        case .last: moveFocus(to: Int.max)
        case .expand: expandOrDescend()
        case .collapse: collapseOrAscend()
        case .expandAll: viewOperation("expand_all") { $0.collapsed = [] }
        case .collapseAll:
            // The containers are found inside the operation, so finding them is part of what is timed.
            viewOperation("collapse_all") { state in
                state.collapsed = Set(self.snapshot.gantt?.rows.compactMap(\.parent) ?? []).filter { self.snapshot.gantt?.index[$0] != nil }
            }
        case .select:
            if let id = gantt.focus { select(.work(id)) }
        case .openDetail:
            if let row = focusedRow { openDetail(for: .work(row.id), from: Self.element(forRow: row.row.key), row: row.id) }
        case .closeDetail:
            if gantt.editingFilter {
                gantt.editingFilter = false
                updateFocusTarget()
            } else {
                closeDetail()
            }
        case .zoomIn: zoom(by: 1)
        case .zoomOut: zoom(by: -1)
        case .panLeft: pan(dx: -Self.panStepX, dy: 0)
        case .panRight: pan(dx: Self.panStepX, dy: 0)
        case .panUp: pan(dx: 0, dy: -Self.panStepRows * GanttLayout.rowHeight)
        case .panDown: pan(dx: 0, dy: Self.panStepRows * GanttLayout.rowHeight)
        case .toggleCritical: viewOperation("filter") { $0.filter.criticalOnly.toggle() }
        case .clearFilter: viewOperation("filter") { $0.filter = GanttFilter() }
        case .cycleStatus: cycleStatus()
        case .focusFilter:
            gantt.editingFilter = true
            updateFocusTarget()
        }
    }

    /// Close Detail and go back to where it was opened: the page and, from the Gantt, the row, kept by
    /// identity. The selection is not touched, so a link followed inside Detail keeps what it selected.
    public func closeDetail() {
        guard page == .detail, let origin = detailReturn else { return }
        detailReturn = nil
        if origin.page == .network {
            // Back on the network, the originating node is the cursor again and the element asked to focus.
            // Set after the page, so entering the page does not put the cursor on a link followed inside Detail.
            page = origin.page
            if let node = origin.row { network.focus = node }
            focusTarget = origin.element
            return
        }
        if let row = origin.row { gantt.focus = row }
        page = origin.page
        if origin.page == .gantt, snapshot.gantt != nil {
            reconcileFocus()
            updateFocusTarget()
        } else {
            focusTarget = origin.element
        }
    }

    // MARK: Moving the cursor

    private func moveFocus(by delta: Int) {
        let rows = outline
        guard !rows.isEmpty else { return }
        let current = gantt.focus.flatMap { id in rows.firstIndex { $0.id == id } }
        let target = current.map { $0 + delta } ?? (delta > 0 ? 0 : rows.count - 1)
        moveFocus(to: target)
    }

    private func moveFocus(to position: Int) {
        let rows = outline
        guard !rows.isEmpty else { return }
        let at = max(0, min(rows.count - 1, position))
        var state = gantt
        state.focus = rows[at].id
        Self.keepVisible(at, in: &state)
        gantt = state
        updateFocusTarget()
    }

    private static func keepVisible(_ position: Int, in state: inout GanttViewState) {
        let top = Double(position) * GanttLayout.rowHeight
        let bottom = top + GanttLayout.rowHeight
        let height = Double(state.viewportRows) * GanttLayout.rowHeight
        if top < state.panY { state.panY = top } else if bottom > state.panY + height { state.panY = bottom - height }
    }

    private func expandOrDescend() {
        guard let row = focusedRow, row.children > 0 else { return }
        if !row.expanded {
            viewOperation("expand") { $0.collapsed.remove(row.id) }
        } else if row.position + 1 < outline.count {
            moveFocus(to: row.position + 1)
        }
    }

    private func collapseOrAscend() {
        guard let row = focusedRow else { return }
        if row.children > 0, row.expanded {
            viewOperation("collapse") { $0.collapsed.insert(row.id) }
        } else if let parent = row.row.parent, let position = outline.first(where: { $0.id == parent })?.position {
            moveFocus(to: position)
        }
    }

    /// Select this row with the pointer: the cursor goes to it and the shared selection follows.
    public func focusRow(_ identity: String, selecting: Bool = true) {
        guard let position = outline.first(where: { $0.id == identity })?.position else { return }
        moveFocus(to: position)
        rowRequests &+= 1
        if selecting { select(.work(identity)) }
    }

    /// The disclosure triangle of a row, or the pointer.
    public func toggleDisclosure(_ identity: String) {
        guard let row = outline.first(where: { $0.id == identity }), row.children > 0 else { return }
        viewOperation(row.expanded ? "collapse" : "expand") { state in
            if row.expanded { state.collapsed.insert(identity) } else { state.collapsed.remove(identity) }
        }
    }

    // MARK: View operations

    /// Apply a change to how the Gantt is drawn, as one operation with its own sequence number: the
    /// generation a draw must carry, with the rows and the view state it must show, to complete it. The
    /// generation is allocated and its call is logged on entry, before anything is applied, so the call's
    /// time includes all the synchronous work of the operation; what the draw must show is set after it.
    func viewOperation(_ kind: String, _ change: (inout GanttViewState) -> Void) {
        let sequence = gantt.sequence &+ 1
        MeasureLog.shared?.emit("view_op_call", gen: sequence, detail: ["op": kind])
        let previousY = gantt.panY
        var state = gantt
        state.locateNote = nil
        change(&state)
        // A pan by key, pointer or Locate is the person's viewport: the automatic fit no longer applies.
        if kind == "pan" { state.viewportChosen = true }
        state.sequence = sequence
        gantt = state
        // Folding or unfolding one package changes only the rows below it, so nothing above it moves: the vertical
        // offset is kept even past the end of the shorter content, as long as a row stays in view.
        // A pan or zoom never jumps an offset kept past the end that way: it may only shrink it.
        clampPan(keepingRowsAbove: kind == "collapse" || kind == "expand", keptY: kind == "pan" || kind == "zoom" ? previousY : nil)
        // A pure geometry operation (pan, zoom) only clamps: the cursor and focus stay where they are and
        // the pan survives even when the cursor is scrolled out of view.
        if kind != "pan", kind != "zoom" {
            reconcileFocus()
            // A change of row membership can leave the cursor outside the viewport: bring it back, as a key does.
            // A fold or unfold does not: the person acted on a package in view, and the rows above it stay put
            // even when a wheel pan left the cursor off screen.
            if kind != "collapse", kind != "expand", let id = gantt.focus, let at = outline.firstIndex(where: { $0.id == id }) {
                var kept = gantt
                Self.keepVisible(at, in: &kept)
                if kept.panY != gantt.panY { gantt.panY = kept.panY }
            }
        }
        updateFocusTarget()
        guard let log = MeasureLog.shared else { return }
        let rows = outline.count
        let applied = gantt
        let fingerprint = GanttFingerprint.facts(zoom: applied.zoomLevel, offsetX: applied.panX, offsetY: applied.panY, filter: applied.filter, collapsed: applied.collapsed.count)
        log.expect("view_op:\(sequence)", facts: fingerprint.merging(["sequence": String(sequence), "rows": String(rows)]) { first, _ in first })
        var detail: [String: Any] = ["kind": "view_op", "op": kind, "rows": rows]
        for (name, value) in fingerprint { detail[name] = value }
        log.emit("state_set", gen: sequence, detail: detail)
    }

    private func clampPan(keepingRowsAbove: Bool = false, keptY: Double? = nil) {
        guard let schedule = snapshot.gantt else { return }
        let state = gantt
        let maxX = max(0, GanttLayout.timelineWidth(schedule, scale: state.scale) - state.viewportWidth)
        let fitted = max(0, Double(outline.count - state.viewportRows) * GanttLayout.rowHeight)
        let lastRowAtTop = max(0, Double(outline.count - 1) * GanttLayout.rowHeight)
        let maxY = keepingRowsAbove ? max(fitted, min(state.panY, lastRowAtTop)) : max(fitted, min(keptY ?? 0, lastRowAtTop))
        let x = max(0, min(maxX, state.panX)), y = max(0, min(maxY, state.panY))
        if x != state.panX || y != state.panY {
            gantt.panX = x
            gantt.panY = y
        }
    }

    private func zoom(by delta: Int) {
        viewOperation("zoom") { state in
            let before = state.scale
            let steps = GanttLayout.zoomSteps
            // A fitted scale beyond the last step in the direction asked stays: a zoom never goes the other way.
            if let fitted = state.fitScale, (delta > 0 && fitted >= steps[steps.count - 1] - 1e-9) || (delta < 0 && fitted <= steps[0] + 1e-9) { return }
            if let fitted = state.fitScale {
                // From a fitted scale, one step is the next zoom step beyond it in that direction.
                state.zoomLevel = delta > 0 ? (steps.firstIndex { $0 > fitted + 1e-9 } ?? steps.count - 1) : (steps.lastIndex { $0 < fitted - 1e-9 } ?? 0)
                state.fitScale = nil
            } else {
                state.zoomLevel = max(0, min(GanttLayout.zoomSteps.count - 1, state.zoomLevel + delta))
            }
            state.viewportChosen = true
            // The hour at the left edge stays there, so zooming does not lose the place.
            state.panX = state.panX / before * state.scale
        }
    }
}
