// The owner's progress report in the inspector: a share set by slider, by a typed percent or by a bar drag, sent only
// by an explicit report, from the pointer or the keyboard alone.

import DPMObserverCore
import SwiftUI

/// The owner's progress report for started work: a slider in steps of 5% and an explicit report. It is sent at once as
/// the acting actor, never through the draft, and the application refuses it unless that actor owns the work.
struct ProgressEditor: View {
    @ObservedObject var model: ObserverModel
    let item: WorkItem
    @State private var percent = 0.0

    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            Text("Reported progress \(Int(percent))%\(item.owner.map { " · owner \($0)" } ?? "")").font(.caption).foregroundStyle(.secondary)
            if let staged = model.operations.stagedProgress, staged.work == item.identity {
                Label("Set to \(staged.percent)% on the bar; Report sends it", systemImage: "hand.draw").font(.caption)
                    .accessibilityIdentifier("edit.progress-staged")
            }
            HStack {
                Slider(value: $percent, in: 0...100, step: 5)
                    .accessibilityLabel("Reported progress of \(item.key)")
                    .accessibilityValue("\(Int(percent)) percent")
                    .accessibilityIdentifier("edit.progress")
                // The share typed, for the keyboard: Tab reaches this field whatever the system's keyboard navigation, and
                // Return reports it.
                TextField("Progress percent", text: Binding(get: { "\(Int(percent))" }, set: { text in
                    if let typed = Int(text.trimmingCharacters(in: .whitespaces)) { percent = Double(min(100, max(0, typed))) }
                }))
                .textFieldStyle(.roundedBorder).frame(width: 56).onSubmit(report)
                .modifier(InspectorFocus(model: model, name: "progress-percent"))
                .accessibilityLabel("Progress percent of \(item.key)").accessibilityIdentifier("edit.progress-percent")
                Button("Report", action: report)
                    .keyboardShortcut("p", modifiers: [.command, .option])
                    .disabled(Int(percent) == item.reportedProgress)
                    .accessibilityLabel("Report progress of \(item.key)")
                    .accessibilityIdentifier("edit.progress-report")
                    .help("Report this share as \(model.editing.actor) (⌥⌘P, or Return in the percent field); a report of 100% is not acceptance")
            }
        }
        .onAppear { percent = Double(staged ?? item.reportedProgress) }
        .onChange(of: item.reportedProgress) { _, now in percent = Double(now) }
        .onChange(of: model.operations.stagedProgress?.percent) { _, now in if let now, staged != nil { percent = Double(now) } }
    }

    private func report() {
        guard Int(percent) != item.reportedProgress else { return }
        model.reportProgress(work: item.identity, percent: Int(percent))
    }

    private var staged: Int? {
        guard let staged = model.operations.stagedProgress, staged.work == item.identity else { return nil }
        return staged.percent
    }
}
