// How the shared model carries the Gantt: the rows it draws, the keyboard cursor, the view operations
// and the one place a key press is applied. The app's window and the qualification suite both end in
// `handleKey`, so there is one path from a key to the model and to the focus it asks for.
//
// Every operation here changes what is drawn and nothing else. None of them can reach the project:
// the only call that leaves this file is `select`, the same shared selection every page uses, which
// makes a read. Collapse, filter, zoom and pan never touch the selection, so it persists through them,
// through a page change and through Detail.

import Foundation

/// A key press as the keyboard delivers it, reduced to what decides its meaning. It is built from a
/// real `NSEvent` (see `KeyEvents.swift`), and the suite builds the same from events it constructs.
public struct KeyInput: Equatable, Sendable {
    /// The characters the key makes without modifiers; an arrow or paging key is its function-key character.
    public var characters: String
    public var shift: Bool
    public var option: Bool
    public var command: Bool
    public var control: Bool

    public init(characters: String, shift: Bool = false, option: Bool = false, command: Bool = false, control: Bool = false) {
        self.characters = characters
        self.shift = shift
        self.option = option
        self.command = command
        self.control = control
    }
}

public enum GanttIntent: String, CaseIterable, Sendable {
    case moveUp, moveDown, pageUp, pageDown, first, last
    case expand, collapse, expandAll, collapseAll
    case select, openDetail, closeDetail
    case zoomIn, zoomOut
    case panLeft, panRight, panUp, panDown
    case toggleCritical, cycleStatus, clearFilter, focusFilter
}

public enum KeyOutcome: Equatable, Sendable {
    case ignored
    case handled(GanttIntent)
}

/// The pure part: which intent a key press means, if any. It applies nothing.
public enum KeyDecoder {
    public static let up = "\u{F700}", down = "\u{F701}", left = "\u{F702}", right = "\u{F703}"
    public static let home = "\u{F729}", end = "\u{F72B}", pageUp = "\u{F72C}", pageDown = "\u{F72D}"

    public static func intent(for input: KeyInput) -> GanttIntent? {
        if input.control { return nil }
        if input.command {
            switch input.characters {
            case "d": return .openDetail
            case "[": return .closeDetail
            default: return nil
            }
        }
        if input.option { return nil }
        switch input.characters {
        case up: return input.shift ? .panUp : .moveUp
        case down: return input.shift ? .panDown : .moveDown
        case left: return input.shift ? .panLeft : .collapse
        case right: return input.shift ? .panRight : .expand
        case pageUp: return .pageUp
        case pageDown: return .pageDown
        case home: return .first
        case end: return .last
        case " ": return .select
        case "\r", "\u{3}": return .openDetail
        case "\u{1B}": return .closeDetail
        case "=", "+": return .zoomIn
        case "-", "_": return .zoomOut
        case "/": return .focusFilter
        case "c": return .toggleCritical
        case "s": return .cycleStatus
        case "x": return .clearFilter
        case ",": return .collapseAll
        case ".": return .expandAll
        default: return nil
        }
    }
}

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
    private func viewOperation(_ kind: String, _ change: (inout GanttViewState) -> Void) {
        let sequence = gantt.sequence &+ 1
        MeasureLog.shared?.emit("view_op_call", gen: sequence, detail: ["op": kind])
        var state = gantt
        change(&state)
        state.sequence = sequence
        gantt = state
        clampPan()
        // A pure geometry operation (pan, zoom) only clamps: the cursor and focus stay where they are and
        // the pan survives even when the cursor is scrolled out of view.
        if kind != "pan", kind != "zoom" {
            reconcileFocus()
            // A change of row membership can leave the cursor outside the viewport: bring it back, as a key does.
            if let id = gantt.focus, let at = outline.firstIndex(where: { $0.id == id }) {
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

    private func clampPan() {
        guard let schedule = snapshot.gantt else { return }
        let state = gantt
        let maxX = max(0, GanttLayout.timelineWidth(schedule, level: state.zoomLevel) - state.viewportWidth)
        let maxY = max(0, Double(outline.count - state.viewportRows) * GanttLayout.rowHeight)
        let x = max(0, min(maxX, state.panX)), y = max(0, min(maxY, state.panY))
        if x != state.panX || y != state.panY {
            gantt.panX = x
            gantt.panY = y
        }
    }

    private func zoom(by delta: Int) {
        viewOperation("zoom") { state in
            let before = GanttLayout.pointsPerHour(state.zoomLevel)
            state.zoomLevel = max(0, min(GanttLayout.zoomSteps.count - 1, state.zoomLevel + delta))
            // The hour at the left edge stays there, so zooming does not lose the place.
            state.panX = state.panX / before * GanttLayout.pointsPerHour(state.zoomLevel)
        }
    }

    public func pan(dx: Double, dy: Double) {
        viewOperation("pan") { state in
            state.panX += dx
            state.panY += dy
        }
    }

    /// Filter by one lifecycle word, or by none.
    public func setFilterStatus(_ status: String?) {
        guard status != gantt.filter.status else { return }
        viewOperation("filter") { $0.filter.status = status }
    }

    public func setCriticalOnly(_ on: Bool) {
        guard on != gantt.filter.criticalOnly else { return }
        viewOperation("filter") { $0.filter.criticalOnly = on }
    }

    private func cycleStatus() {
        let words = snapshot.gantt?.statuses ?? []
        viewOperation("filter") { state in
            guard !words.isEmpty else { return }
            if let current = state.filter.status, let at = words.firstIndex(of: current) {
                state.filter.status = at + 1 < words.count ? words[at + 1] : nil
            } else {
                state.filter.status = words[0]
            }
        }
    }

    /// The text of the filter field.
    public func setFilterText(_ text: String) {
        guard text != gantt.filter.text else { return }
        viewOperation("filter") { $0.filter.text = text }
    }

    /// The filter field says whether it has the keyboard, as the view reports it.
    public func filterFieldFocus(_ focused: Bool) {
        guard focused != gantt.editingFilter else { return }
        gantt.editingFilter = focused
        if focused {
            updateFocusTarget()
        } else {
            // Focus left the field without Escape or a commit (Tab, Shift-Tab, a click elsewhere): the
            // keyboard is where the system put it, so no row is named and the rows are not asked to take it.
            focusLeftFilter = true
            if focusTarget == "gantt.filter" { focusTarget = nil }
        }
    }

    /// The window says how many rows and how much width it shows.
    public func setViewport(rows: Int, width: Double) {
        guard rows > 0, width > 0, rows != gantt.viewportRows || abs(width - gantt.viewportWidth) > 1 else { return }
        gantt.viewportRows = rows
        gantt.viewportWidth = width
    }

    /// A scripted scroll, for measurement: the request that a draw must show to count as a frame.
    public func driveScroll(_ drive: GanttDrive?) {
        gantt.drive = drive
    }

    /// The content extent and viewport the Gantt would scroll through along an axis now.
    public func scrollMetrics(_ axis: GanttDrive.Axis) -> (extent: Double, viewport: Double) {
        GanttLayout.scrollExtent(axis, schedule: snapshot.gantt, rows: outline.count, state: gantt)
    }

    /// What the focused element says about itself, as the view reports it.
    public func reportFocus(_ element: String?) {
        if focusReported != element { focusReported = element }
    }
}
