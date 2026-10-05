// Gantt: the plan as a hierarchy on a timeline, read-only. Rows follow the plan's work packages; a bar
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
            GanttBody(model: model, frame: frame, keyboardRequest: keyboardRequest) { holds in
                holdsKeyboard = holds
                report(holds)
            }
            // Containers of their own, as the rows are: an identifier set on a container that is not an element is
            // stamped on the elements inside it, and would hide the region's own `gantt.region`.
            .accessibilityElement(children: .contain)
            .accessibilityIdentifier("gantt.body")
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .top)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("gantt")
        .onAppear { report() }
        .onChange(of: model.focusTarget) {
            // The shared handler (or the pointer) put the cursor on a row: the region takes the keyboard.
            if let target = model.focusTarget, target.hasPrefix("gantt.row.") { keyboardRequest &+= 1 }
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
                tool("Fit timeline", "arrow.left.and.right.square", "gantt.fit", help: "Fit timeline: zoom so the whole plan, hour 0 to its last finish or p95, fills the timeline. Changes only the view.") { model.fitTimeline() }
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
                    if frame.selected == row.id { Badge(text: "Selected", symbol: "checkmark.circle", tint: .blue) }
                }
                Text("\(kindWords(row)) · \(row.status)\(row.priority.isEmpty ? "" : " · priority \(row.priority)")\(row.span?.critical == true ? " · on the critical path" : "") · \(when)\(float) · \(finish)")
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
                    if row.span?.critical == true { Badge(text: "On the critical path", symbol: "flame", tint: .orange) }
                    Text(row.key).font(.system(.callout, design: .monospaced))
                    if frame.selected == row.id { Badge(text: "Selected", symbol: "checkmark.circle", tint: .blue) }
                }
                Prose(text: facts(row), font: .caption)
                ForEach(Array(frame.inventory.relations(of: row.id).prefix(8)), id: \.id) { edge in
                    let names = { (identity: String) in schedule.row(identity)?.key ?? frame.inventory.key(of: identity) ?? String(identity.prefix(8)) }
                    Prose(text: "• " + edge.words(names: names), font: .caption)
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
        var parts: [String] = []
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
    /// Told whether the region holds the keyboard each time it gains or loses it.
    let onKeyboard: (Bool) -> Void
    /// The receipts of the pointer events this body's timeline received: one bounded log per concrete body, registered
    /// by its region for its own window.
    @State private var pointer = GanttPointerLog()

    var body: some View {
        // The height is the one the window offers, not one derived from the row count: the reader has no
        // ideal size of its own, so the page compresses to the space left and the row count follows it.
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
            .background(GanttRegion(request: keyboardRequest, cursor: frame.cursor.map { speech($0, frame: frame).label }, pointer: pointer, onKeyboard: onKeyboard) { probed in
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

struct GanttLabels: View {
    let frame: GanttFrame
    let geometry: GanttGeometry
    var model: ObserverModel?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Work (\(frame.outline.count) rows)").font(.caption.weight(.semibold)).accessibilityAddTraits(.isHeader)
                Spacer()
            }
            .padding(.horizontal, 8)
            .frame(height: GanttGeometry.axis)
            .reportFrame("axis")
            ZStack(alignment: .topLeading) {
                ForEach(Array(frame.outline[geometry.firstRow..<(geometry.firstRow + geometry.rowsDrawn)])) { item in
                    GanttLabel(item: item, frame: frame, model: model)
                        .frame(height: GanttLayout.rowHeight)
                        .offset(y: Double(item.position) * GanttLayout.rowHeight - geometry.offsetY)
                        .background { if item.position == geometry.firstRow { ObservedProbe(name: "first_row") } }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .clipped()
            .reportFrame("rows")
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Gantt rows")
        .accessibilityIdentifier("gantt.rows")
    }
}

struct GanttLabel: View {
    let item: GanttOutlineRow
    let frame: GanttFrame
    var model: ObserverModel?

    var body: some View {
        let row = item.row
        let said = speech(item, frame: frame)
        HStack(spacing: 4) {
            Color.clear.frame(width: Double(item.depth) * 14)
            if item.children > 0 {
                Image(systemName: item.expanded ? "chevron.down" : "chevron.right").font(.system(size: 9, weight: .bold)).frame(width: 12)
                    .accessibilityHidden(true)
                    .onTapGesture { model?.toggleDisclosure(item.id) }
            } else {
                Color.clear.frame(width: 12)
            }
            Image(systemName: row.isMilestone ? "diamond.fill" : (row.isPackage ? "folder" : "circle")).font(.system(size: 10)).accessibilityHidden(true)
            Text(row.key).font(.system(size: 11, design: .monospaced))
            Text(row.title).font(.system(size: 12, weight: row.isPackage ? .semibold : .regular)).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 2)
            Text(row.priority).font(.system(size: 10)).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 6)
        .contentShape(Rectangle())
        .help("\(row.key): \(row.title)")
        .onTapGesture(count: 2) {
            model?.focusRow(item.id)
            model?.perform(.openDetail)
        }
        .onTapGesture { model?.focusRow(item.id) }
        .spoken(said.label, value: said.value)
        .accessibilityAddTraits(frame.selected == row.id ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction(named: "Open in Detail") {
            model?.focusRow(item.id)
            model?.perform(.openDetail)
        }
        .accessibilityIdentifier(ObserverModel.element(forRow: row.key))
    }
}

// MARK: - The keyboard region, and the viewport as the window reports it

/// The native view behind the rows and the timeline: the Gantt's one keyboard stop, a real first responder of
/// its window, and the measure of the viewport. Whether the Gantt holds the keyboard is whether this view is its
/// window's actual first responder; nothing else says so.
struct GanttRegion: NSViewRepresentable {
    @Environment(\.hostObservation) private var host
    /// The model's requests for a row to have the keyboard, counted; a new one makes the region take it.
    let request: Int
    /// The cursor row in words, the region's accessibility value.
    let cursor: String?
    let pointer: GanttPointerLog
    let onKeyboard: (Bool) -> Void
    let onResize: (CGSize) -> Void

    func makeNSView(context: Context) -> GanttRegionView {
        let view = GanttRegionView()
        view.request = request
        update(view)
        return view
    }

    func updateNSView(_ view: GanttRegionView, context: Context) {
        update(view)
        guard view.request != request else { return }
        view.request = request
        view.take("row_requested")
    }

    private func update(_ view: GanttRegionView) {
        view.host = host
        if view.pointer !== pointer {
            view.pointer = pointer
            view.registerPointer()
        }
        view.onKeyboard = onKeyboard
        view.onResize = onResize
        view.cursor = cursor
    }
}

final class GanttRegionView: NSView {
    weak var host: HostObservation?
    var onKeyboard: ((Bool) -> Void)?
    var onResize: ((CGSize) -> Void)?
    var request = 0
    var cursor: String? {
        didSet { if cursor != oldValue, holdsKeyboard { NSAccessibility.post(element: self, notification: .valueChanged) } }
    }

    /// Whether this view is now the actual first responder of the window it is in.
    var holdsKeyboard: Bool { window?.firstResponder === self }

    override var acceptsFirstResponder: Bool { true }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { NSBezierPath(rect: bounds).fill() }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Gantt" }
    override func accessibilityValue() -> Any? { cursor ?? "" }
    override func accessibilityHelp() -> String? { "Arrow keys move the cursor row, Space selects it, Return opens it in Detail. The rows beside it carry their full text." }
    override func accessibilityIdentifier() -> String { "gantt.region" }

    override func layout() {
        super.layout()
        onResize?(bounds.size)
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        note("become", ["accepted": accepted])
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        note("resign", ["accepted": accepted])
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    /// Once this turn is done, the page is told whether the region holds the keyboard then, as its window says.
    private func tell() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.note("settled", ["holds_keyboard": self.holdsKeyboard])
            self.onKeyboard?(self.holdsKeyboard)
        }
    }

    /// One request that this region become its window's first responder, unless it already is. From the filter's editor
    /// this one call is the whole handoff: the window ends the editor's editing (the field releases the keyboard,
    /// `filter.end`) and then gives the keyboard to this region, and the field's focus state follows.
    func take(_ reason: String) {
        guard let window, window.firstResponder !== self else { return }
        let fromFilter = host != nil && (window.firstResponder as? NSTextView).flatMap { Self.filterField(of: $0) } != nil
        let accepted = window.makeFirstResponder(self)
        note("take." + reason, ["accepted": accepted, "from_filter_editor": fromFilter])
    }

    override func mouseDown(with event: NSEvent) {
        take("pointer")
        super.mouseDown(with: event)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        // Leaving with the keyboard (the page closed): the window takes it back, as it held it before this view existed.
        if newWindow == nil, let window, window.firstResponder === self { window.makeFirstResponder(nil) }
        super.viewWillMove(toWindow: newWindow)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stopObserving()
        guard let window else {
            MainActor.assumeIsolated { GanttKeyboard.shared.detach(self) }
            return
        }
        MainActor.assumeIsolated { GanttKeyboard.shared.attach(self) }
        // The window rebuilds its key view loop for the views it has now, so Tab and Shift-Tab reach this one on every visit.
        window.recalculateKeyViewLoop()
        startTrace()
        note("attached")
        // Arriving on the page while no control holds the keyboard (the window or its root view does), the region takes
        // it, and the cursor row is the element in it. Back from Detail, the Detail content may still hold it here: it hands
        // it to this region as it leaves the window.
        let responder = window.firstResponder
        if responder == nil || responder === window || responder === window.contentView { take("arrival") }
    }

    // MARK: What the real host reports

    /// The region's own focus events, each with its own window's actual first responder; bounded, and kept only while the
    /// window's state is reported.
    private var events: [[String: Any]] = []
    private var sequence = 0
    private static let limit = 48
    /// The filter's editor in this window, as last seen editing the filter, and its readings: bounded, and kept only while
    /// the window's state is reported.
    private weak var editor: NSTextView?
    private var readings: [[String: Any]] = []
    private var readingSequence = 0
    private var observers: [NSObjectProtocol] = []

    /// This body's pointer log, owned by the body and reported for this region's window.
    var pointer: GanttPointerLog?

    /// The pointer log reports through this region's host for this region's window, from the moment both exist; the
    /// launch creates its one host once, so a new window (not a replaced host) is what registers again. A log whose region
    /// left its window reports nothing (its sampler is removed). Host replacement is not a supported lifecycle here.
    func registerPointer() {
        guard let pointer, let host, window != nil else { return }
        MainActor.assumeIsolated {
            pointer.attach(host: host, region: self)
            host.register("gantt_pointer") { [weak pointer] in pointer?.traced() }
        }
    }

    private func startTrace() {
        guard let host, let window else { return }
        MainActor.assumeIsolated {
            host.register("gantt_focus") { [weak self] in self?.traced() }
            host.register("gantt_filter_editor") { [weak self] in self?.editorTraced() }
        }
        registerPointer()
        // The filter's own editor in this window: when the field begins and ends editing with it (the end is the field
        // releasing the keyboard), and each change of its text or selection (typing, an input method's marked text and its
        // commit). Delivered on the posting turn, so each is in order with the region's own events.
        for name in [NSControl.textDidBeginEditingNotification, NSControl.textDidEndEditingNotification, NSText.didChangeNotification, NSTextView.didChangeSelectionNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { [weak self, weak window] notification in
                let field = notification.object as? NSTextField
                guard let editor = field == nil ? notification.object as? NSTextView : notification.userInfo?["NSFieldEditor"] as? NSTextView,
                      let window, (field?.window ?? editor.window) === window else { return }
                MainActor.assumeIsolated { self?.editorChanged(name, editor, field) }
            })
        }
    }

    private func stopObserving() {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers = []
    }

    deinit {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }

    private func note(_ event: String, _ facts: [String: Any] = [:]) {
        guard let host else { return }
        sequence &+= 1
        var entry = facts
        entry["seq"] = sequence
        entry["event"] = event
        entry["time"] = Date().timeIntervalSince1970
        entry["uptime"] = ProcessInfo.processInfo.systemUptime
        entry["window"] = window.map { ["number": $0.windowNumber, "key": $0.isKeyWindow] as [String: Any] } ?? NSNull()
        entry["first_responder"] = window.map { Self.responder(in: $0, region: self) } ?? NSNull()
        events.append(entry)
        if events.count > Self.limit { events.removeFirst(events.count - Self.limit) }
        MainActor.assumeIsolated { host.noteChange() }
    }

    private static func responder(in window: NSWindow, region: GanttRegionView) -> [String: Any] {
        guard let responder = window.firstResponder else { return ["class": NSNull()] }
        let identifier = (responder as? NSView)?.accessibilityIdentifier() ?? ""
        return ["class": String(describing: type(of: responder)), "identifier": identifier.isEmpty ? NSNull() : identifier,
                "is_region": responder === region, "is_window": responder === window, "is_content_view": responder === window.contentView]
    }

    private func traced() -> [String: Any]? {
        guard let window else { return nil }
        return ["window": window.windowNumber, "holds_keyboard": holdsKeyboard, "events": events, "sequence": sequence, "limit": Self.limit]
    }

    /// How `field` is known to be this page's filter (its accessibility identifier, else its prompt); nil if it is not.
    static func filterIdentity(_ field: NSTextField) -> String? {
        if field.accessibilityIdentifier() == "gantt.filter" || (field.accessibilityAttributeValue(.identifier) as? String) == "gantt.filter" { return "identifier" }
        return field.placeholderString == GanttToolbar.prompt ? "prompt" : nil
    }

    /// How the field `editor` is editing is known to be this page's filter; nil if it is editing no field, or another.
    static func filterField(of editor: NSTextView) -> String? {
        guard editor.isFieldEditor, let field = editor.delegate as? NSTextField, field.window === editor.window else { return nil }
        return filterIdentity(field)
    }

    /// One notification of a text editor of this window (with the field, for a field's begin and end): kept only if the
    /// editor is the filter's.
    private func editorChanged(_ name: Notification.Name, _ editor: NSTextView, _ field: NSTextField?) {
        guard host != nil, let window, let by = field.map({ Self.filterIdentity($0) }) ?? Self.filterField(of: editor) else { return }
        self.editor = editor
        if name == NSControl.textDidBeginEditingNotification { note("filter.begin") }
        if name == NSControl.textDidEndEditingNotification { note("filter.end") }
        readingSequence &+= 1
        var entry = Self.reading(editor, by, in: window)
        entry["seq"] = readingSequence
        entry["cause"] = name.rawValue
        readings.append(entry)
        if readings.count > Self.limit { readings.removeFirst(readings.count - Self.limit) }
        MainActor.assumeIsolated { host?.noteChange() }
    }

    /// What the filter's editor itself holds now, read from the real text view: never from the model's filter text.
    private static func reading(_ editor: NSTextView, _ by: String, in window: NSWindow) -> [String: Any] {
        func range(_ value: NSRange) -> Any { value.location == NSNotFound ? NSNull() : [value.location, value.length] }
        let text = editor.string
        return ["time": Date().timeIntervalSince1970, "uptime": ProcessInfo.processInfo.systemUptime, "window": window.windowNumber, "key_window": window.isKeyWindow,
                "identified_by": by, "first_responder": window.firstResponder === editor, "has_marked_text": editor.hasMarkedText(),
                "marked_range": range(editor.markedRange()), "selected_range": range(editor.selectedRange()),
                "string": String(text.prefix(256)), "length": (text as NSString).length]
    }

    private func editorTraced() -> [String: Any]? {
        guard let window else { return nil }
        // The window's actual first responder if it is the filter's editor (a field given the keyboard posts nothing until it
        // is edited), else the editor last seen editing the filter; null when neither edits the filter in this window now.
        var current: Any = NSNull()
        if let editor = (window.firstResponder as? NSTextView) ?? editor, editor.window === window, let by = Self.filterField(of: editor) {
            current = Self.reading(editor, by, in: window)
        }
        return ["window": window.windowNumber, "current": current, "readings": readings, "sequence": readingSequence, "limit": Self.limit]
    }
}
