// Live: managed and reported-only runs, their public activity, input requests, lifecycle and how
// fresh each is. Run completion is the executor's report; nothing here verifies or releases work.
// The list holds only the newest runs the application returned, and says so; a run is inspected by
// its own identity, so one that has left the list is still reachable from its task.

import DPMObserverCore
import SwiftUI

struct LiveView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        HSplitView {
            List(selection: model.selectionBinding) {
                let unfinished = snapshot.runs.filter { !$0.finished }
                let finished = snapshot.runs.filter { $0.finished }
                Section("Unfinished (\(unfinished.count))") {
                    if unfinished.isEmpty { Text("No unfinished run among \(snapshot.runsCoverage.words).").foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
                    ForEach(unfinished) { run in RunRow(run: run).tag(Subject.run(run.identity)) }
                }
                Section("Ended — the executor's own report (\(finished.count))") {
                    ForEach(finished) { run in RunRow(run: run).tag(Subject.run(run.identity)) }
                }
                if snapshot.runsCoverage.truncated {
                    Text("This list holds only the newest \(snapshot.runsCoverage.read) runs. An older run is opened from its task: find the task in Detail, then choose the run under “Runs of this task”.")
                        .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                        .accessibilityIdentifier("live.coverage")
                }
            }
            .frame(minWidth: 340, idealWidth: 440)
            .accessibilityIdentifier("list.live")
            RunPane(model: model).frame(minWidth: 360)
        }
    }
}

struct RunPane: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        if case .run(let id)? = model.selection {
            if let run = snapshot.runDetail, run.identity == id {
                ScrollView {
                    VStack(alignment: .leading, spacing: Layout.margin) {
                        header(run)
                        Unavailable(controls: snapshot.unsupported)
                        if let window = snapshot.window, window.run == id {
                            inputRequests(window)
                            activity(window)
                            lifecycle(window)
                        } else {
                            ProgressView("Reading this run's activity…")
                        }
                    }
                    .padding(Layout.margin)
                }
                .accessibilityIdentifier("run.scroll")
            } else if let error = snapshot.detail?.error {
                EmptyState(symbol: "exclamationmark.triangle", title: "This run could not be read", message: error)
            } else {
                ProgressView("Reading this run…").frame(maxWidth: .infinity, maxHeight: .infinity)
            }
        } else {
            EmptyState(symbol: "waveform.path.ecg", title: "Select a run", message: "Runs report what they do in public activity. A run that goes quiet is stale, never idle or finished.")
        }
    }

    private func header(_ run: RunSummary) -> some View {
        let state = "Executor \(run.executor). Last report: \(run.state)\(run.stateDetail.map { " — \($0)" } ?? "") since \(displayTime(run.stateSince)). Last heard from \(displayTime(run.lastReceiptAt)).\(run.staleAt.map { " Turns stale at \(displayTime($0)) if nothing more arrives." } ?? "")"
        let observation = run.observation.text == "reported_only" ? (model.snapshot.reportedOnlyNote.isEmpty ? "A reported-only run reports itself; silence proves nothing." : model.snapshot.reportedOnlyNote) : ""
        let orphan = run.orphan.map { "This run no longer matches its task: \($0)." } ?? ""
        let meaning = "A run's completion is the executor's report. It never submits, verifies or releases the task. The contract it observed, its sources and its provider are in Detail."
        return VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            // Everything that only reads is one element that says all of it; the button is not inside it.
            VStack(alignment: .leading, spacing: Layout.rowSpacing) {
                Text("Run of \(run.workKey)").font(.title2.weight(.semibold))
                HStack(spacing: 6) {
                    Badge(text: run.observation.text == "managed" ? "Managed" : "Reported only", symbol: run.observation.text == "managed" ? "gearshape.2" : "text.bubble")
                    RunStatusBadge(run: run)
                    if run.foreignLineage { Badge(text: "Another history", symbol: "arrow.triangle.branch", tint: .orange) }
                }
                Prose(text: state)
                if !observation.isEmpty { Prose(text: observation, font: .callout) }
                if !orphan.isEmpty { Prose(text: orphan, font: .callout) }
                Prose(text: meaning, font: .callout)
            }
            .spoken("Run of \(run.workKey)", value: ([runSpeech(run), state, observation, orphan, meaning]).filter { !$0.isEmpty }.joined(separator: " "))
            .accessibilityIdentifier("run.header")
            Button("Open in Detail") { model.show(.detail) }.keyboardShortcut("d", modifiers: .command)
        }
    }

    private func inputRequests(_ window: RunWindow) -> some View {
        let requests = window.entries.filter { $0.kind == .known(.inputRequested) }
        return VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Text("Input requests (\(requests.count))").font(.headline).accessibilityAddTraits(.isHeader)
            Prose(text: "This app cannot answer or approve a request; the text below is what the run recorded about it.", font: .callout)
            if requests.isEmpty { Text("None recorded in the part of the activity shown.").foregroundStyle(.secondary) }
            ForEach(requests.reversed()) { entry in
                ActivityRow(entry: entry)
            }
        }
        .accessibilityIdentifier("run.input")
    }

    private func activity(_ window: RunWindow) -> some View {
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Text("Activity, newest first").font(.headline).accessibilityAddTraits(.isHeader)
            if window.droppedFromView > 0 {
                Prose(text: "Showing the newest \(window.entries.count) of \(window.recorded) records; \(window.droppedFromView) older are not shown here.", font: .callout)
            }
            if let gap = window.gap { Badge(text: gap, symbol: "exclamationmark.triangle", tint: .orange).accessibilityIdentifier("run.gap") }
            if let incomplete = window.incomplete { Badge(text: incomplete, symbol: "exclamationmark.triangle", tint: .orange).accessibilityIdentifier("run.incomplete") }
            if window.entries.isEmpty { Text("No activity recorded yet.").foregroundStyle(.secondary) }
            ForEach(window.entries.reversed()) { entry in
                ActivityRow(entry: entry)
            }
        }
        .accessibilityIdentifier("run.activity")
    }

    private func lifecycle(_ window: RunWindow) -> some View {
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Text("Lifecycle").font(.headline).accessibilityAddTraits(.isHeader)
            if window.lifecycleDropped > 0 { Prose(text: "\(window.lifecycleDropped) older lifecycle entries are not shown here.", font: .callout) }
            if let incomplete = window.lifecycleIncomplete { Badge(text: incomplete, symbol: "exclamationmark.triangle", tint: .orange).accessibilityIdentifier("run.lifecycle.incomplete") }
            ForEach(window.lifecycle.reversed()) { entry in
                Prose(text: "\(displayTime(entry.recordedAt)) — \(entry.state)\(entry.detail.map { ": \($0)" } ?? "") (\(entry.recordedBy))")
                    .spoken("Lifecycle \(entry.state)", value: "\(displayTime(entry.recordedAt)). \(entry.detail ?? "") Recorded by \(entry.recordedBy).")
            }
        }
    }
}

struct ActivityRow: View {
    let entry: ActivityEntry

    var body: some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol).frame(width: 18).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text("\(label) · \(displayTime(entry.recordedAt)) · record \(entry.sourceSequence)").font(.caption).foregroundStyle(.secondary)
                if !entry.text.isEmpty { Prose(text: entry.text, font: .callout) }
            }
        }
        .spoken("\(label), record \(entry.sourceSequence), \(displayTime(entry.recordedAt))", value: entry.text)
        .accessibilityIdentifier("activity.\(entry.sequence)")
    }

    private var label: String {
        switch entry.kind {
        case .known(.toolStarted): return "Tool started"
        case .known(.toolResult): return "Tool result"
        case .known(.progress): return "Progress"
        case .known(.inputRequested): return "Input requested"
        case .known(.heartbeat): return "Heartbeat"
        case .unknown(let word): return word
        }
    }

    private var symbol: String {
        switch entry.kind {
        case .known(.toolStarted): return "wrench.and.screwdriver"
        case .known(.toolResult): return "checkmark.square"
        case .known(.progress): return "text.alignleft"
        case .known(.inputRequested): return "questionmark.bubble"
        case .known(.heartbeat): return "heart"
        case .unknown: return "questionmark.circle"
        }
    }
}

/// What this app does not do, in plain words, from the application's own list of missing controls.
struct Unavailable: View {
    let controls: [Unsupported]

    var body: some View {
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Label("What this app does not do", systemImage: "lock").font(.headline)
            Prose(text: "This app only observes. It never changes a project.", font: .callout)
            ForEach(controls, id: \.control) { control in
                Prose(text: control.plain, font: .callout)
            }
        }
        .padding(8)
        .background(Color.secondary.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
        .spoken("What this app does not do", value: "This app only observes. " + controls.map(\.plain).joined(separator: " "))
        .accessibilityIdentifier("run.unavailable")
    }
}
