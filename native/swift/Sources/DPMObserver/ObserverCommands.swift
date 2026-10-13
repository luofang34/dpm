// The View menu's Gantt items: the same view operations as the Gantt's keys, through the same model calls, so the
// menu bar lists what the keys do. Unmodified keys stay the rows' own (a menu item on c or x would also take the
// letter from the filter field), so the menu uses Command chords that no key of the page or the editor already holds.
// None of these changes the plan; they change only what is shown.

import DPMObserverCore
import SwiftUI

/// What the Gantt page offers the menu bar while it is shown: the actions that live in the view, not the model.
struct GanttActions {
    var toggleInspector: () -> Void
}

private struct GanttActionsKey: FocusedValueKey {
    typealias Value = GanttActions
}

extension FocusedValues {
    /// The shown Gantt page's actions, or nil while another page is shown.
    var ganttActions: GanttActions? {
        get { self[GanttActionsKey.self] }
        set { self[GanttActionsKey.self] = newValue }
    }
}

struct GanttCommands: Commands {
    @ObservedObject var model: ObserverModel
    @FocusedValue(\.ganttActions) private var actions

    var body: some Commands {
        CommandGroup(after: .toolbar) {
            Divider()
            Button("Zoom In") { model.perform(.zoomIn) }
                .keyboardShortcut("=", modifiers: .command)
                .disabled(!ready)
            Button("Zoom Out") { model.perform(.zoomOut) }
                .keyboardShortcut("-", modifiers: .command)
                .disabled(!ready)
            Button("Fit Timeline") { model.fitTimeline() }
                .keyboardShortcut("0", modifiers: .command)
                .disabled(!ready)
            Divider()
            Button("Collapse All Packages") { model.perform(.collapseAll) }
                .disabled(!ready)
            Button("Expand All Packages") { model.perform(.expandAll) }
                .disabled(!ready)
            Toggle("Critical Work Only", isOn: Binding(get: { model.gantt.filter.criticalOnly }, set: { model.setCriticalOnly($0) }))
                .disabled(!ready)
            // ⌘F stays Find Task or Decision; the Gantt's filter field is ⌥⌘F here and / on the rows.
            Button("Filter Gantt Rows") { model.perform(.focusFilter) }
                .keyboardShortcut("f", modifiers: [.command, .option])
                .disabled(model.page != .gantt)
            Button("Clear Gantt Filter") { model.perform(.clearFilter) }
                .disabled(!ready || !model.gantt.filter.isActive)
            Divider()
            Button("Show or Hide Inspector") { actions?.toggleInspector() }
                .keyboardShortcut("i", modifiers: [.command, .option])
                .disabled(actions == nil)
        }
    }

    /// The Gantt is the page shown and has a schedule to act on, as its keys require.
    private var ready: Bool { model.page == .gantt && model.snapshot.gantt != nil }
}
