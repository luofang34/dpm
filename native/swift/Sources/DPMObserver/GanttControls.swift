// The Gantt's controls. The page keeps one row of its own: the filter field (which the keyboard reaches with `/`
// and leaves with Escape, so it stays a field of the page, above the rows), a Filter menu with the status choice,
// critical-only and Clear, the row count, and the Edit toggle. The view controls (zoom, fit, locate, collapse and
// expand all, the keys help and the inspector) are items of the window's toolbar while the Gantt is shown. Every
// control acts through the same model calls as the keys; none of them changes the plan.

import DPMObserverCore
import SwiftUI

struct GanttToolbar: View {
    @ObservedObject var model: ObserverModel
    let frame: GanttFrame
    @FocusState private var filtering: Bool

    static let keys = "Keys: ↑ ↓ move · ← collapse or go to parent · → expand or go in · Space select · Return or ⌘D open Detail · Esc or ⌘[ back from Detail · + − zoom (⌘= ⌘− from the menu, ⌘0 fit) · ⇧ arrows pan · / filter (⌥⌘F from the menu) · c critical only · s status · x clear · , . collapse or expand all · ⌥⌘I inspector. Pointer: a click on a bar or row selects it, a double click opens Detail, a drag or a scroll pans."

    var body: some View {
        let state = frame.state
        let count = "\(frame.outline.count) of \(frame.schedule?.rows.count ?? 0) rows · \(state.filter.words)"
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                TextField(Self.prompt, text: Binding(get: { model.gantt.filter.text }, set: { model.setFilterText($0) }))
                    .textFieldStyle(.roundedBorder)
                    .frame(minWidth: 180, maxWidth: 320)
                    .reportFrame("filter")
                    .focused($filtering)
                    .accessibilityLabel("Filter rows")
                    .accessibilityHint("Matches key, title, status and kind. Escape returns to the rows.")
                    .accessibilityIdentifier("gantt.filter")
                filterMenu(state)
                Text(count)
                    .font(.caption)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .help(count)
                    .accessibilityIdentifier("gantt.count")
                Spacer(minLength: 0)
                EditToggle(model: model)
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

    /// The status choice, critical-only and Clear in one menu; its title says when a filter is on, in words as well as
    /// by its symbol. The keys s, c and x do the same from the rows.
    private func filterMenu(_ state: GanttViewState) -> some View {
        Menu {
            Picker("Status", selection: Binding(get: { state.filter.status ?? "Any status" }, set: { model.setFilterStatus($0 == "Any status" ? nil : $0) })) {
                ForEach(["Any status"] + (frame.schedule?.statuses ?? []), id: \.self) { Text($0).tag($0) }
            }
            .accessibilityIdentifier("gantt.status")
            Toggle("Critical only", isOn: Binding(get: { state.filter.criticalOnly }, set: { model.setCriticalOnly($0) }))
                .accessibilityHint("Only work the query puts on the critical path, with the packages that contain it")
                .accessibilityIdentifier("gantt.critical")
            Divider()
            Button("Clear filter") { model.perform(.clearFilter) }
                .disabled(!state.filter.isActive)
                .accessibilityIdentifier("gantt.clear")
        } label: {
            Label(state.filter.isActive ? "Filter on" : "Filter",
                  systemImage: state.filter.isActive ? "line.3.horizontal.decrease.circle.fill" : "line.3.horizontal.decrease.circle")
        }
        .fixedSize()
        .help("Filter by status, or show only critical work (s, c; x clears)")
        .accessibilityIdentifier("gantt.filter-menu")
    }

    /// The field's prompt, by which the region also knows the field's editor (see `GanttRegionView.filterField`).
    static let prompt = "Filter rows by key, title, status or kind"

    static func scaleWords(_ scale: Double) -> String {
        scale >= 10 ? String(format: "%.0f", scale) : scale >= 1 ? String(format: "%.1f", scale) : String(format: "%.2f", scale)
    }
}

/// A compact icon control of the window toolbar, with its full action as its accessibility label and its tooltip.
struct GanttTool: View {
    let label: String
    let symbol: String
    let id: String
    let help: String
    var disabled = false
    let action: () -> Void

    var body: some View {
        Button(action: action) { Label(label, systemImage: symbol) }
            .disabled(disabled)
            .help(help)
            .accessibilityLabel(label)
            .accessibilityIdentifier(id)
    }
}

/// The keys and pointer help, shown on request from the toolbar; its complete text is also the button's hint and tooltip.
struct GanttKeysButton: View {
    @State private var shown = false

    var body: some View {
        Button { shown.toggle() } label: { Label("Keys", systemImage: shown ? "keyboard.chevron.compact.down" : "keyboard") }
            .help(GanttToolbar.keys)
            .accessibilityLabel("Keys and pointer help")
            .accessibilityValue(shown ? "shown" : "hidden")
            .accessibilityHint(GanttToolbar.keys)
            .accessibilityIdentifier("gantt.keys-toggle")
            .popover(isPresented: $shown, arrowEdge: .bottom) {
                Text(GanttToolbar.keys)
                    .font(.callout)
                    .fixedSize(horizontal: false, vertical: true)
                    .frame(width: 360, alignment: .leading)
                    .padding(Layout.margin)
                    .textSelection(.enabled)
                    .accessibilityIdentifier("gantt.keys")
            }
    }
}

/// What the inspector beside the Gantt shows: the cursor row in full, or the editor.
enum GanttInspectorTab: String, Hashable {
    case info = "Info", edit = "Edit"
}

/// The inspector: a choice of Info or Edit, then the cursor row's complete text and the schedule basis, or the edit
/// panel. Edit without editing on says the window only reads and how to turn editing on; nothing here edits by itself.
struct GanttInspector: View {
    @ObservedObject var model: ObserverModel
    let frame: GanttFrame
    @Binding var tab: GanttInspectorTab

    var body: some View {
        VStack(spacing: 0) {
            Picker("Inspector", selection: $tab) {
                Text(GanttInspectorTab.info.rawValue).tag(GanttInspectorTab.info)
                Text(GanttInspectorTab.edit.rawValue).tag(GanttInspectorTab.edit)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .padding(8)
            .accessibilityLabel("Inspector content")
            .accessibilityIdentifier("gantt.inspector.tab")
            Divider()
            switch tab {
            case .info:
                ScrollView {
                    GanttStrip(frame: frame, cursorIdentifier: "gantt.inspector.cursor")
                }
                .accessibilityIdentifier("gantt.inspector.info")
            case .edit:
                if model.editing.enabled {
                    EditPanel(model: model)
                } else {
                    readOnly
                }
            }
        }
    }

    private var readOnly: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("Editing is off", systemImage: "lock").font(.headline)
            Prose(text: "This window only reads the plan. Turn on Edit (⌘E) to change it: edits gather in a draft and apply only after review, with a reason.", font: .callout)
            Button("Turn On Editing") { model.setEditing(true) }
                .accessibilityIdentifier("gantt.inspector.edit-on")
            Spacer(minLength: 0)
        }
        .padding(Layout.margin)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("gantt.inspector.read-only")
    }
}
