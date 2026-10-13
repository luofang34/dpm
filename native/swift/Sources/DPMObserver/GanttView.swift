// Gantt: the plan as a hierarchy on a timeline; it only reads unless editing is on. Rows follow the plan's work packages; a bar
// is the elapsed-hour span the shared schedule projection gives, a milestone is a diamond with its own
// text, and float, criticality, priority, relations and the finish forecast are the query's own values.
//
// Everything is reachable without a pointer. The page holds the keyboard as one native region, a real
// AppKit first responder behind the rows and the timeline, whose active row is the model's cursor; keys go
// through `GanttKeyboard` to `ObserverModel.handleKey`, the same path the suite drives. The rows are real
// accessibility elements carrying their full text; the timeline beside them is a drawing and is hidden
// from assistive technology, its content being the rows.
// Stale or lost data is marked on the page and in the status bar, and the last reading stays drawn.

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
    /// Whether the cursor row's complete text is expanded in the strip; compact by default, so the chart is the primary area.
    @State private var detailsShown = false

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
            // The cursor row in a compact summary (identity, status, a short schedule line); its complete text scrolls
            // in the expanded strip, is the row's accessibility text and is in Detail.
            GanttStrip(frame: frame, scrolling: true, expanded: $detailsShown)
                .frame(maxHeight: detailsShown ? 170 : 62)
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
        .inspector(isPresented: Binding(get: { model.editing.enabled }, set: { model.setEditing($0) })) { EditPanel(model: model) }
        .sheet(isPresented: Binding(get: { model.editing.reviewing }, set: { if !$0 { model.closeReview() } }), onDismiss: { model.reviewDismissed() }) {
            ReviewSheet(model: model)
        }
        // Closing the review, after an apply or a cancel, gives the keyboard back to the rows it was opened from.
        .onChange(of: model.editing.focusReturns) { keyboardRequest &+= 1 }
        .onChange(of: model.rowRequests) { keyboardRequest &+= 1 }
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

// MARK: - Controls

struct GanttToolbar: View {
    @ObservedObject var model: ObserverModel
    let frame: GanttFrame
    @FocusState private var filtering: Bool
    @State private var keysShown = false

    static let keys = "Keys: ↑ ↓ move · ← collapse or go to parent · → expand or go in · Space select · Return or ⌘D open Detail · Esc or ⌘[ back from Detail · + − zoom · ⇧ arrows pan · / filter · c critical only · s status · x clear · , . collapse or expand all. Pointer: a click on a bar or row selects it, a double click opens Detail, a drag or a scroll pans."

    /// A compact icon control with its full action as its accessibility label and its tooltip: no clipped meaning.
    private func tool(_ label: String, _ symbol: String, _ id: String, help: String, disabled: Bool = false, action: @escaping () -> Void) -> some View {
        Button(action: action) { Image(systemName: symbol).frame(minWidth: 18) }
            .disabled(disabled)
            .help(help)
            .accessibilityLabel(label)
            .accessibilityIdentifier(id)
    }

    var body: some View {
        let state = frame.state
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                TextField(Self.prompt, text: Binding(get: { model.gantt.filter.text }, set: { model.setFilterText($0) }))
                    .textFieldStyle(.roundedBorder)
                    .frame(minWidth: 220, maxWidth: 320)
                    .reportFrame("filter")
                    .focused($filtering)
                    .accessibilityLabel("Filter rows")
                    .accessibilityHint("Matches key, title, status and kind. Escape returns to the rows.")
                    .accessibilityIdentifier("gantt.filter")
                Picker("Status", selection: Binding(get: { state.filter.status ?? "Any status" }, set: { model.setFilterStatus($0 == "Any status" ? nil : $0) })) {
                    ForEach(["Any status"] + (frame.schedule?.statuses ?? []), id: \.self) { Text($0).tag($0) }
                }
                .frame(maxWidth: 190)
                .accessibilityIdentifier("gantt.status")
                Toggle("Critical only", isOn: Binding(get: { state.filter.criticalOnly }, set: { model.setCriticalOnly($0) }))
                    .accessibilityHint("Only work the query puts on the critical path, with the packages that contain it")
                    .accessibilityIdentifier("gantt.critical")
                Button("Clear filter") { model.perform(.clearFilter) }
                    .disabled(!state.filter.isActive)
                    .accessibilityIdentifier("gantt.clear")
                Spacer(minLength: 0)
                EditToggle(model: model)
            }
            HStack(spacing: 6) {
                tool("Collapse all", "rectangle.compress.vertical", "gantt.collapse-all", help: "Collapse all work packages (,)") { model.perform(.collapseAll) }
                tool("Expand all", "rectangle.expand.vertical", "gantt.expand-all", help: "Expand all work packages (.)") { model.perform(.expandAll) }
                Divider().frame(height: 16)
                tool("Zoom out", "minus.magnifyingglass", "gantt.zoom-out", help: "Zoom out (−)", disabled: state.zoomLevel == 0 && state.fitScale == nil) { model.perform(.zoomOut) }
                Text("\(Self.scaleWords(state.scale)) pt/h")
                    .font(.caption).monospacedDigit()
                    .accessibilityLabel("Zoom: \(Self.scaleWords(state.scale)) points per elapsed hour")
                tool("Zoom in", "plus.magnifyingglass", "gantt.zoom-in", help: "Zoom in (+)", disabled: state.zoomLevel == GanttLayout.zoomSteps.count - 1 && state.fitScale == nil) { model.perform(.zoomIn) }
                tool("Fit timeline", "arrow.left.and.right.square", "gantt.fit", help: "Fit timeline: zoom so the whole plan, from its earliest recorded start to its last finish or p95, fills the timeline. Changes only the view.") { model.fitTimeline() }
                tool("Locate selected", "scope", "gantt.locate", help: "Locate selected: put the cursor on the selected task and scroll its bar into view") { model.locateSelected() }
                Divider().frame(height: 16)
                Text("\(frame.outline.count) of \(frame.schedule?.rows.count ?? 0) rows · \(state.filter.words)")
                    .font(.caption)
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .help("\(frame.outline.count) of \(frame.schedule?.rows.count ?? 0) rows · \(state.filter.words)")
                    .accessibilityIdentifier("gantt.count")
                Spacer(minLength: 0)
                Button { keysShown.toggle() } label: { Label("Keys", systemImage: keysShown ? "keyboard.chevron.compact.down" : "keyboard") }
                    .help(Self.keys)
                    .accessibilityLabel("Keys and pointer help")
                    .accessibilityValue(keysShown ? "shown" : "hidden")
                    .accessibilityHint(Self.keys)
                    .accessibilityIdentifier("gantt.keys-toggle")
            }
            if let note = state.locateNote {
                // Why Locate selected did not move: the selected row is filtered out (the selection is kept).
                Label(note, systemImage: "eye.slash")
                    .font(.caption)
                    .fixedSize(horizontal: false, vertical: true)
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(note)
                    .accessibilityIdentifier("gantt.locate-note")
            }
            if keysShown {
                // The legend, shown on request; its complete text is also the toggle's hint and tooltip.
                Text(Self.keys)
                    .font(.caption2)
                    .foregroundStyle(.secondary)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(maxHeight: 54, alignment: .topLeading)
                    .accessibilityIdentifier("gantt.keys")
            }
        }
        .padding(.horizontal, Layout.margin)
        .padding(.vertical, 8)
        .reportFrame("toolbar")
        .onChange(of: filtering) { model.filterFieldFocus(filtering) }
        .onChange(of: model.focusTarget) {
            // A row named while the field has the keyboard (Escape) is the region's to take: its one native take ends the
            // field's editing and gives it the keyboard, and `filtering` follows that, as it does on Tab. Released here as
            // well, the field would be released a second time, after the region took the keyboard.
            let row = model.focusTarget?.hasPrefix("gantt.row.") ?? false
            if model.focusTarget == "gantt.filter" { filtering = true } else if filtering, !row { filtering = false }
        }
    }

    /// The field's prompt, by which the region also knows the field's editor (see `GanttRegionView.filterField`).
    static let prompt = "Filter rows by key, title, status or kind"

    static func scaleWords(_ scale: Double) -> String {
        scale >= 10 ? String(format: "%.0f", scale) : scale >= 1 ? String(format: "%.1f", scale) : String(format: "%.2f", scale)
    }
}

/// The cursor row in full, and the project's forecast: complete text, never cut for width.
struct GanttStrip: View {
    let frame: GanttFrame
    let scrolling: Bool
    /// Whether the complete text is shown; the compact summary otherwise. The still picture shows it complete.
    var expanded: Binding<Bool> = .constant(true)

    var body: some View {
        let content = VStack(alignment: .leading, spacing: 4) { details }
            .padding(.horizontal, Layout.margin)
            .padding(.vertical, 6)
            .frame(maxWidth: .infinity, alignment: .leading)
        if scrolling {
            ScrollView { content }.reportFrame("strip").accessibilityIdentifier("gantt.strip")
        } else {
            content
        }
    }

    @ViewBuilder
    private var details: some View {
        if let schedule = frame.schedule {
            let spread = schedule.uncertainty.map { "forecast p50 \(hoursText($0.p50)), p80 \(hoursText($0.p80)), p95 \(hoursText($0.p95)) over \($0.iterations) simulations (seed \($0.seed))" } ?? "no finish forecast: the query supplied none (open choices, no estimates, or not requested)"
            if expanded.wrappedValue {
                Prose(text: "Elapsed hours are counted from \(displayTime(schedule.evaluatedAt)), when the schedule was evaluated; the query supplies no calendar dates, so none is shown. Project finishes in \(hoursText(schedule.projectFinishHours)); \(spread). \(schedule.milestones) milestones.", font: .caption)
                    .spoken("Schedule basis", value: "Elapsed hours are counted from \(displayTime(schedule.evaluatedAt)). No calendar dates are supplied. Project finish \(hoursText(schedule.projectFinishHours)); \(spread).")
            }
            if let item = frame.cursor {
                if expanded.wrappedValue { rowDetail(item, schedule: schedule) } else { compact(item, schedule: schedule, spread: spread) }
            } else if frame.outline.isEmpty {
                Prose(text: "No row matches the filter (\(frame.state.filter.words)). The plan has \(schedule.rows.count) rows; clearing the filter shows them again. The selection is kept.", font: .callout)
            }
        } else {
            ProgressView("Reading the schedule from the application…").controlSize(.small)
        }
    }

    /// The cursor row in two lines: its identity and status, and a short schedule line. Its accessibility text is the
    /// row's complete text, and `Details` expands the complete wrapped text here. The toggle stands beside the spoken
    /// text, not inside it: a spoken group ignores its children, so a button inside it would not be a control.
    @ViewBuilder
    private func compact(_ item: GanttOutlineRow, schedule: GanttSchedule, spread: String) -> some View {
        let row = item.row
        let said = speech(item, frame: frame)
        let when = row.span.map { row.isMilestone ? "at \(hoursText($0.start))" : "\(hoursText($0.start)) to \(hoursText($0.finish))" } ?? "not scheduled: \(row.applicability)"
        let float = row.times.map { " · total float \(hoursText($0.totalFloat))" } ?? ""
        let finish = "project finish \(hoursText(schedule.projectFinishHours))" + (schedule.uncertainty.map { ", p50 \(hoursText($0.p50)), p80 \(hoursText($0.p80)), p95 \(hoursText($0.p95))" } ?? "")
        HStack(alignment: .firstTextBaseline, spacing: 6) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(spacing: 6) {
                    Text(row.key).font(.system(.callout, design: .monospaced).weight(.semibold))
                    Text(row.title).font(.callout.weight(.semibold)).lineLimit(1).help(row.title)
                    Spacer(minLength: 4)
                }
                // The strip follows the cursor, which keys move without selecting; it says which it is.
                Text("\(frame.selected == row.id ? "Selected" : "Cursor, not selected") · \(kindWords(row)) · \(row.status)\(row.priority.isEmpty ? "" : " · priority \(row.priority)")\(row.span?.critical == true ? " · on the critical path" : "") · \(when)\(float) · \(finish)")
                    .font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    .help(spread)
            }
            .spoken(said.label, value: said.value)
            .accessibilityIdentifier("gantt.cursor")
            if scrolling { detailsToggle }
        }
    }

    private var detailsToggle: some View {
        Button(expanded.wrappedValue ? "Less" : "Details") { expanded.wrappedValue.toggle() }
            .controlSize(.small)
            .help(expanded.wrappedValue ? "Show the compact summary" : "Show the complete text of the cursor row and the schedule basis")
            .accessibilityIdentifier("gantt.details-toggle")
    }

    @ViewBuilder
    private func rowDetail(_ item: GanttOutlineRow, schedule: GanttSchedule) -> some View {
        let row = item.row
        let said = speech(item, frame: frame)
        // The toggle beside the spoken text, as in the compact summary, so it stays a control of its own.
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(row.title).font(.headline).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                    Spacer(minLength: 4)
                }
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
            .accessibilityIdentifier("gantt.cursor")
            if scrolling { detailsToggle }
        }
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
