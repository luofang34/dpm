// Gantt: the plan as a hierarchy on a timeline; it only reads unless editing is on. Rows follow the plan's work packages; a bar
// is the elapsed-hour span the shared schedule projection gives, a milestone is a diamond with its own
// text, and float, criticality, priority, relations and the finish forecast are the query's own values.
//
// Everything is reachable without a pointer. The page holds the keyboard as one native region, a real
// AppKit first responder behind the rows and the timeline, whose active row is the model's cursor; keys go
// through `GanttKeyboard` to `ObserverModel.handleKey`, the same path the suite drives. The rows are real
// accessibility elements carrying their full text; the timeline beside them is a drawing and is hidden
// from assistive technology, its content being the rows.
// Stale or lost data is marked on the page and in the window subtitle or the status bar, and the last reading stays drawn.

import AppKit
import DPMObserverCore
import SwiftUI

struct GanttView: View {
    @ObservedObject var model: ObserverModel
    /// Whether the region holds the keyboard: its window's actual first responder, as the region reports it.
    @State private var holdsKeyboard = false
    /// Counts the model's requests for a row to have the keyboard; the region takes it on each, if it lacks it.
    @State private var keyboardRequest = 0
    @State private var rowNamed = 0
    /// Whether the person opened the inspector. It is also shown while editing is on, as the edit panel always was.
    @State private var inspectorOpen = false
    /// The inspector's tab as last chosen; nil follows editing (Edit while editing is on, Info otherwise), so turning
    /// editing on opens the inspector on Edit.
    @State private var inspectorChoice: GanttInspectorTab?

    var body: some View {
        let frame = GanttFrame(model: model, keyboard: holdsKeyboard)
        VStack(spacing: 0) {
            GanttToolbar(model: model, frame: frame)
            Divider()
            if let mark = frame.marked {
                Badge(text: mark, symbol: "exclamationmark.triangle", tint: .orange)
                    .padding(.horizontal, Layout.margin)
                    .padding(.vertical, 4)
                    .accessibilityIdentifier("gantt.mark")
            }
            // The cursor row in one line (identity, status, a short schedule line); its complete text is the row's
            // accessibility text, the inspector's Info and Detail.
            GanttCursorLine(frame: frame) { showInfo() }
            Divider()
            GanttBody(model: model, frame: frame, keyboardRequest: keyboardRequest, rowNamed: rowNamed) { holds in
                holdsKeyboard = holds
                report(holds)
            }
            // Containers of their own, as the rows are: an identifier set on a container that is not an element is
            // stamped on the elements inside it, and would hide the region's own `gantt.region`.
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("gantt.body")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .toolbar { windowTools(frame) }
        .inspector(isPresented: inspectorShown) {
            GanttInspector(model: model, frame: frame, tab: Binding(get: { tab }, set: { inspectorChoice = $0 }))
        }
        .focusedSceneValue(\.ganttActions, GanttActions(toggleInspector: { toggleInspector() }))
        .sheet(isPresented: Binding(get: { model.editing.reviewing }, set: { if !$0 { model.closeReview() } }), onDismiss: { model.reviewDismissed() }) {
            ReviewSheet(model: model)
        }
        // Closing the review, after an apply or a cancel, gives the keyboard back to the rows it was opened from.
        .onChange(of: model.editing.focusReturns) { keyboardRequest &+= 1 }
        .onChange(of: model.rowRequests) { keyboardRequest &+= 1 }
        .onChange(of: model.editing.enabled) { inspectorChoice = nil }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("gantt")
        .onAppear { report() }
        .onChange(of: model.focusTarget) {
            // The shared handler put the cursor on a row: the region takes the keyboard, unless other text is being edited
            // in its window (a row named again when a newer plan is read must not take it from an inspector field).
            if let target = model.focusTarget, target.hasPrefix("gantt.row.") { rowNamed &+= 1 }
            report()
        }
        .onChange(of: model.gantt.editingFilter) { report() }
    }

    private var tab: GanttInspectorTab { inspectorChoice ?? (model.editing.enabled ? .edit : .info) }

    /// Shown while the person has it open or while editing is on. Closing it while editing ends editing, as closing the
    /// edit inspector always did; opening it never turns editing on.
    private var inspectorShown: Binding<Bool> {
        Binding(get: { inspectorOpen || model.editing.enabled }, set: { shown in
            inspectorOpen = shown
            if !shown, model.editing.enabled { model.setEditing(false) }
        })
    }

    private func toggleInspector() {
        let shown = inspectorShown
        shown.wrappedValue = !shown.wrappedValue
    }

    private func showInfo() {
        inspectorChoice = .info
        inspectorOpen = true
    }

    /// The view controls, in the window's toolbar while the Gantt is shown, as four groups a person reads at a glance:
    /// zoom (out, the scale, in), where to look (fit, locate), the outline (collapse and expand all), and help with the
    /// inspector at the trailing end. A gap between groups keeps a system that draws adjacent items as one piece from
    /// merging them. Each control acts through the same model call as its key.
    @ToolbarContentBuilder
    private func windowTools(_ frame: GanttFrame) -> some ToolbarContent {
        let state = frame.state
        GanttToolbarGap()
        ToolbarItemGroup {
            GanttTool(label: "Zoom out", symbol: "minus.magnifyingglass", id: "gantt.zoom-out", help: "Zoom out (− or ⌘−)",
                      disabled: state.zoomLevel == 0 && state.fitScale == nil) { model.perform(.zoomOut) }
            Text("\(GanttToolbar.scaleWords(state.scale)) pt/h")
                .font(.caption)
                .monospacedDigit()
                .accessibilityLabel("Zoom: \(GanttToolbar.scaleWords(state.scale)) points per elapsed hour")
            GanttTool(label: "Zoom in", symbol: "plus.magnifyingglass", id: "gantt.zoom-in", help: "Zoom in (+ or ⌘=)",
                      disabled: state.zoomLevel == GanttLayout.zoomSteps.count - 1 && state.fitScale == nil) { model.perform(.zoomIn) }
        }
        GanttToolbarGap()
        ToolbarItemGroup {
            GanttTool(label: "Fit timeline", symbol: "arrow.left.and.right.square", id: "gantt.fit",
                      help: "Fit timeline (⌘0): zoom so the whole plan, from its earliest recorded start to its last finish or p95, fills the timeline. Changes only the view.") { model.fitTimeline() }
            GanttTool(label: "Locate selected", symbol: "scope", id: "gantt.locate",
                      help: "Locate selected: put the cursor on the selected task and scroll its bar into view") { model.locateSelected() }
        }
        GanttToolbarGap()
        ToolbarItemGroup {
            GanttTool(label: "Collapse all", symbol: "rectangle.compress.vertical", id: "gantt.collapse-all", help: "Collapse all work packages (,)") { model.perform(.collapseAll) }
            GanttTool(label: "Expand all", symbol: "rectangle.expand.vertical", id: "gantt.expand-all", help: "Expand all work packages (.)") { model.perform(.expandAll) }
        }
        GanttToolbarGap()
        ToolbarItemGroup {
            GanttKeysButton()
            GanttTool(label: "Inspector", symbol: "sidebar.trailing", id: "gantt.inspector-toggle",
                      help: "Show or hide the inspector: the cursor row in full, or the editor (⌥⌘I)") { toggleInspector() }
        }
    }

    /// The view says what holds the keyboard, as `search_focused` does for the search field: the filter field,
    /// the cursor row while the region is the actual first responder, or nothing.
    private func report(_ region: Bool? = nil) {
        guard model.page == .gantt else { return }
        if model.gantt.editingFilter {
            model.reportFocus("gantt.filter")
        } else if region ?? holdsKeyboard {
            model.reportFocus(model.focusTarget)
        } else {
            model.reportFocus(nil)
        }
        // A change of the focus the view reports is written to a reported state; with no reporter there is no host.
        Launch.shared.host?.noteChange()
    }
}

/// The cursor row in one line under the controls: identity, status and a short schedule line, cut for width only here,
/// with the row's complete text as its accessibility text and Info opening it in full in the inspector.
struct GanttCursorLine: View {
    let frame: GanttFrame
    let showInfo: () -> Void

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            summary
            Spacer(minLength: 4)
            // Beside the spoken text, not inside it: a spoken group ignores its children, so a button inside would not be a control.
            Button("Info", action: showInfo)
                .controlSize(.small)
                .help("Show the complete text of the cursor row and the schedule basis in the inspector (⌥⌘I)")
                .accessibilityIdentifier("gantt.details-toggle")
        }
        .padding(.horizontal, Layout.margin)
        .padding(.vertical, 6)
        .frame(maxWidth: .infinity, minHeight: 32, alignment: .leading)
        .reportFrame("strip")
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("gantt.strip")
    }

    @ViewBuilder
    private var summary: some View {
        if let schedule = frame.schedule {
            if let item = frame.cursor {
                let row = item.row
                let said = speech(item, frame: frame)
                let date = { (hours: Double) in GanttCalendarAxis.date(origin: schedule.evaluatedAt, hours: hours) }
                // Finished work is told by its recorded dates and the as-built path; its remaining span is all zero.
                let finished = GanttLayout.isFinished(row)
                let recorded = row.actual.map { actual in [actual.start, actual.finish].compactMap { $0 }.map(date).joined(separator: " to ") }
                let when = finished ? "recorded \(recorded ?? "")" : row.span.map { row.isMilestone ? "at \(hoursText($0.start))" : "\(hoursText($0.start)) to \(hoursText($0.finish))" } ?? "not scheduled: \(row.applicability)"
                let float = finished ? "" : row.times.map { " · total float \(hoursText($0.totalFloat))" } ?? ""
                let path = GanttLayout.isCritical(row) ? (finished ? " · on the as-built critical path" : " · on the critical path") : ""
                let end = GanttLayout.hasRemainingWork(schedule) ? "project finish \(hoursText(schedule.projectFinishHours))"
                    : GanttLayout.recordedFinishHours(schedule).map { "project finished \(date($0))" } ?? "nothing recorded"
                // The strip follows the cursor, which keys move without selecting; it says which it is.
                let facts = "\(frame.selected == row.id ? "Selected" : "Cursor, not selected") · \(kindWords(row)) · \(row.status)\(row.priority.isEmpty ? "" : " · priority \(row.priority)")\(path) · \(when)\(float) · \(end)"
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(row.key).font(.system(.callout, design: .monospaced).weight(.semibold))
                    Text(row.title).font(.callout.weight(.semibold)).lineLimit(1).layoutPriority(1)
                    Text(facts).font(.caption).monospacedDigit().foregroundStyle(.secondary).lineLimit(1)
                }
                .help("\(row.title) · \(facts)")
                .spoken(said.label, value: said.value)
                .accessibilityIdentifier("gantt.cursor")
            } else if frame.outline.isEmpty {
                let words = "No row matches the filter (\(frame.state.filter.words)). The plan has \(schedule.rows.count) rows; clearing the filter shows them again. The selection is kept."
                Text(words).font(.callout).lineLimit(1).help(words).accessibilityLabel(words)
            }
        } else {
            // Inline and small, so the strip keeps its one-line height when the schedule arrives and the chart below
            // it does not move.
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text("Reading the schedule from the application…").font(.caption).foregroundStyle(.secondary)
            }
        }
    }
}

/// The cursor row in full, and the project's forecast: complete text, never cut for width. The inspector's Info shows it,
/// and so does the still picture.
struct GanttStrip: View {
    let frame: GanttFrame
    /// The cursor's identifier here; the one-line strip on the page is `gantt.cursor`.
    var cursorIdentifier = "gantt.cursor"

    var body: some View {
        VStack(alignment: .leading, spacing: 4) { details }
            .padding(.horizontal, Layout.margin)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
    }

    @ViewBuilder
    private var details: some View {
        if let schedule = frame.schedule {
            let spread = schedule.uncertainty.map { "forecast p50 \(hoursText($0.p50)), p80 \(hoursText($0.p80)), p95 \(hoursText($0.p95)) over \($0.iterations) simulations (seed \($0.seed))" } ?? "no finish forecast: the query supplied none (open choices, no estimates, or not requested)"
            Prose(text: "Elapsed hours are counted from \(displayTime(schedule.evaluatedAt)), when the schedule was evaluated; the query supplies no calendar dates, so none is shown. Project finishes in \(hoursText(schedule.projectFinishHours)); \(spread). \(schedule.milestones) milestones.", font: .caption)
                .spoken("Schedule basis", value: "Elapsed hours are counted from \(displayTime(schedule.evaluatedAt)). No calendar dates are supplied. Project finish \(hoursText(schedule.projectFinishHours)); \(spread).")
            if let item = frame.cursor {
                rowDetail(item, schedule: schedule)
            } else if frame.outline.isEmpty {
                Prose(text: "No row matches the filter (\(frame.state.filter.words)). The plan has \(schedule.rows.count) rows; clearing the filter shows them again. The selection is kept.", font: .callout)
            }
        } else {
            ProgressView("Reading the schedule from the application…").controlSize(.small)
        }
    }

    @ViewBuilder
    private func rowDetail(_ item: GanttOutlineRow, schedule: GanttSchedule) -> some View {
        let row = item.row
        let said = speech(item, frame: frame)
        VStack(alignment: .leading, spacing: 3) {
            Text(row.title).font(.headline).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            HStack(spacing: 6) {
                Badge(text: kindWords(row), symbol: row.isMilestone ? "diamond.fill" : (row.isPackage ? "folder" : "circle"))
                Badge(text: row.status, symbol: "flag.checkered")
                if !row.priority.isEmpty { Badge(text: "Priority \(row.priority)", symbol: "person.fill.questionmark") }
                if row.span?.critical == true { Badge(text: "On the critical path", symbol: "flame", tint: Palette.critical) }
                Text(row.key).font(.system(.callout, design: .monospaced))
            }
            Prose(text: facts(row), font: .caption)
            ForEach(Array(frame.inventory.relations(of: row.id).prefix(8)), id: \.id) { edge in
                let names = { (identity: String) in schedule.row(identity)?.key ?? frame.inventory.key(of: identity) ?? String(identity.prefix(8)) }
                Prose(text: "• " + schedule.words(edge, names: names), font: .caption)
            }
            if frame.inventory.relations(of: row.id).count > 8 {
                Text("and \(frame.inventory.relations(of: row.id).count - 8) more relations are listed in Detail").font(.caption).foregroundStyle(.secondary)
            }
        }
        .spoken(said.label, value: said.value)
        .accessibilityIdentifier(cursorIdentifier)
    }

    /// The row's own numbers in one line each way: nothing here is computed from other values.
    private func facts(_ row: GanttRow) -> String {
        var parts: [String] = [frame.selected == row.id ? "Selected" : "Cursor, not selected"]
        if let span = row.span {
            parts.append(row.isMilestone ? "Milestone at \(hoursText(span.start)) elapsed" : "Elapsed hours \(hoursText(span.start)) to \(hoursText(span.finish)) (\(hoursText(span.hours)))")
        } else {
            parts.append("Not scheduled: \(row.applicability)")
        }
        if let times = row.times {
            parts.append("total float \(hoursText(times.totalFloat)), free float \(hoursText(times.freeFloat)), latest finish \(hoursText(times.latestFinish))")
        }
        parts.append(frame.inventory.byIdentity[row.id].map { GanttWords.estimate($0.estimate) } ?? "own estimate not available: the plan snapshot does not list this item")
        parts.append(row.criticality.map { "criticality \(percentText($0)) of simulated schedules (not the human priority \(row.priority))" } ?? "criticality not supplied by the query")
        if let calendar = row.calendar { parts.append(calendar) }
        return parts.joined(separator: " · ")
    }
}

// MARK: - Rows and timeline

struct GanttBody: View {
    @ObservedObject var model: ObserverModel
    @Environment(\.layoutRegistry) private var layout
    let frame: GanttFrame
    let keyboardRequest: Int
    let rowNamed: Int
    /// Told whether the region holds the keyboard each time it gains or loses it.
    let onKeyboard: (Bool) -> Void
    /// The receipts of the pointer events this body's timeline received: one bounded log per concrete body, registered
    /// by its region for its own window.
    @State private var pointer = GanttPointerLog()
    /// A fold or unfold being animated; nil at rest.
    @State private var motion: GanttMotion?

    var body: some View {
        // The height is the one the window offers, not one derived from the row count: the reader has no
        // ideal size of its own, so the page compresses to the space left and the row count follows it.
        TimelineView(.animation(paused: motion == nil)) { timeline in
            content(frame.moving(motion, at: timeline.date))
        }
        .onChange(of: frame.state.collapsed) { old, _ in animateFold(from: old) }
    }

    /// Fold or unfold rows over a short animation: the rows below the package close up over the folded rows, or open
    /// out to reveal them, while nothing above the package moves. A person who asked for reduced motion, and a
    /// scripted measurement, see the result at once.
    private func animateFold(from old: Set<String>) {
        // One package folding or unfolding is animated; collapse-all, expand-all and a reveal of several are not.
        let toggled = old.symmetricDifference(frame.state.collapsed)
        guard toggled.count == 1, let package = toggled.first, MeasureLog.shared == nil,
              !NSWorkspace.shared.accessibilityDisplayShouldReduceMotion, let schedule = frame.schedule else { return }
        let before = GanttLayout.outline(schedule, collapsed: old, filter: frame.state.filter)
        guard let started = GanttMotion(from: before, to: model.outline, toggled: package, at: Date()) else { return }
        motion = started
        DispatchQueue.main.asyncAfter(deadline: .now() + GanttMotion.duration) {
            if motion?.start == started.start { motion = nil }
        }
    }

    private func content(_ frame: GanttFrame) -> some View {
        GeometryReader { proxy in
            let size = proxy.size
            let geometry = GanttGeometry(frame, height: size.height)
            // The label column adapts to the body's width, so the timeline keeps most of it; the timeline's own
            // coordinates are what the pointer hits, so hit testing follows the actual split.
            let labels = GanttLayout.labelWidth(body: size.width)
            HStack(spacing: 0) {
                GanttLabels(frame: frame, geometry: geometry, model: model)
                    .frame(width: labels, height: size.height)
                Divider()
                Canvas { context, size in GanttDrawing.draw(&context, size: size, frame: frame) }
                    .frame(maxWidth: .infinity, maxHeight: size.height)
                    .modifier(GanttTimelinePointer(model: model, log: pointer))
                    .reportFrame("timeline")
                    .accessibilityHidden(true)
                    .accessibilityIdentifier("gantt.timeline")
            }
            .frame(width: size.width, height: size.height, alignment: .topLeading)
            .clipped()
            .reportFrame("body")
            .background(GanttRegion(request: keyboardRequest, renamed: rowNamed, cursor: frame.cursor.map { speech($0, frame: frame).label }, pointer: pointer, onKeyboard: onKeyboard) { probed in
                let rows = Int((probed.height - GanttGeometry.axis) / GanttLayout.rowHeight)
                DispatchQueue.main.async {
                    let before = model.gantt.viewportRows
                    model.setViewport(rows: rows, width: probed.width - GanttLayout.labelWidth(body: probed.width) - 1)
                    if model.gantt.viewportRows != before { layout?.noteChange() }
                    fitInitially()
                }
            })
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .onChange(of: frame.schedule?.token) { fitInitially() }
    }

    /// The first valid schedule is fitted to the measured timeline once, unless the person already chose a viewport.
    /// A scripted measurement keeps the default zoom its record names.
    private func fitInitially() {
        guard MeasureLog.shared == nil else { return }
        model.fitTimelineInitially()
    }
}

/// A row's symbol: its kind by shape and, for a task or milestone, its lifecycle state by fill, so the outline says
/// what is done or under way without colour.
enum GanttGlyph {
    static func symbol(_ row: GanttRow) -> String {
        if row.isPackage { return "folder" }
        if row.isMilestone { return row.status == "Verified" ? "diamond.fill" : "diamond" }
        switch row.status {
        case "Verified": return "checkmark.circle.fill"
        case "Submitted": return "hourglass.circle"
        case "InProgress": return "circle.lefthalf.filled"
        case "Claimed": return "circle.dotted"
        case "Blocked": return "exclamationmark.circle"
        case "Proposed": return "circle.dashed"
        default: return "circle"
        }
    }
}

/// The fixed gap between two Gantt toolbar groups where the system provides one; elsewhere groups already stand apart.
struct GanttToolbarGap: ToolbarContent {
    var body: some ToolbarContent {
        if #available(macOS 26.0, *) {
            ToolbarSpacer(.fixed)
        }
    }
}
