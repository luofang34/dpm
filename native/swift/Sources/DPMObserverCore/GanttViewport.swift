// The Gantt's viewport and filter: fitting the timeline, locating the selected row, panning, the status,
// text and critical-only filters, and what the view reports back (its measured viewport, scroll extent and
// focus). Each one changes only what is drawn; none of them reaches the project or changes an hour.

import Foundation

extension ObserverModel {
    /// Fit timeline: the scale at which the whole plan, from its earliest recorded start (or hour 0) to its last finish
    /// or p95 with the trailing room, is exactly the timeline's measured width, scrolled to its start. A view operation
    /// only; no hour changes.
    public func fitTimeline() {
        applyFit(chosen: true)
    }

    /// The fit of the valid schedule on entry, kept fitted as the measured width or the schedule changes, until the
    /// person chooses a viewport (a zoom, a pan, an explicit fit): their zoom, pan and selection then stay through
    /// refreshes and re-entry.
    public func fitTimelineInitially() {
        guard !gantt.viewportChosen, let schedule = snapshot.gantt, let scale = GanttLayout.fitScale(schedule, width: gantt.viewportWidth) else { return }
        if let current = gantt.fitScale, abs(current - scale) < 1e-9, abs(gantt.panX - GanttLayout.firstX(schedule, scale: scale)) < 1e-9 { return }
        applyFit(chosen: false)
    }

    private func applyFit(chosen: Bool) {
        guard let schedule = snapshot.gantt, let scale = GanttLayout.fitScale(schedule, width: gantt.viewportWidth) else { return }
        viewOperation("zoom") { state in
            state.fitScale = scale
            // The zoom step shown beside the fitted scale: the largest one not above it.
            state.zoomLevel = GanttLayout.zoomSteps.lastIndex { $0 <= scale } ?? 0
            state.panX = GanttLayout.firstX(schedule, scale: scale)
            if chosen { state.viewportChosen = true }
        }
    }

    /// Locate selected: the selected row under the cursor, scrolled into view along both axes. A row hidden only by
    /// collapsed packages is revealed by expanding exactly its collapsed ancestors (no other package). A row the filter
    /// hides is not panned to: the note says it is filtered out and how to show it, and the selection, the filter, the
    /// cursor and the view stay as they are. Changes only the view.
    public func locateSelected() {
        guard case .work(let id)? = selection, let schedule = snapshot.gantt, let row = schedule.row(id) else { return }
        var ancestors: [String] = []
        var cursor = row.parent
        while let up = cursor, ancestors.count < schedule.rows.count {
            ancestors.append(up)
            cursor = schedule.row(up)?.parent
        }
        let hiding = Set(ancestors).intersection(gantt.collapsed)
        let revealed = gantt.collapsed.subtracting(hiding)
        guard GanttLayout.outline(schedule, collapsed: revealed, filter: gantt.filter).contains(where: { $0.id == id }) else {
            // Only the note changes: no cursor, pan, zoom, collapse or filter change, and no sequence of a drawn operation.
            gantt.locateNote = "\(row.key) is selected but the filter (\(gantt.filter.words)) hides its row, so it cannot be shown: clear the filter (x) to locate it. The selection is kept."
            return
        }
        if !hiding.isEmpty { viewOperation("expand") { $0.collapsed.subtract(hiding) } }
        focusRow(id, selecting: false)
        guard let start = row.actual?.hours?.from ?? row.span?.start else { return }
        let x = max(GanttLayout.firstX(schedule, scale: gantt.scale), start * gantt.scale - 40)
        viewOperation("pan") { $0.panX = x }
    }

    /// The displayed row at a timeline point (the y from the top of the rows, below the axis), at the current scroll offset.
    public func ganttRow(atY y: Double) -> GanttOutlineRow? {
        guard y >= 0 else { return nil }
        let position = Int((y + gantt.panY) / GanttLayout.rowHeight)
        return position < outline.count ? outline[position] : nil
    }

    public func pan(dx: Double, dy: Double) {
        viewOperation("pan") { state in
            state.panX += dx
            state.panY += dy
            state.viewportChosen = true
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

    func cycleStatus() {
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
