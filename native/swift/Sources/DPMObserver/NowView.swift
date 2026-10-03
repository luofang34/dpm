// Now: what to do, why, which runs are in progress and, when nothing is ready, what holds work back.
// Every reason is the application's own; nothing is ranked or judged ready here. Anything that
// holds work back can be opened, and any task can be found with the search in Detail.

import DPMObserverCore
import SwiftUI

extension ObserverModel {
    var selectionBinding: Binding<Subject?> {
        Binding(get: { self.selection }, set: { self.select($0) })
    }
}

struct NowView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        HSplitView {
            List(selection: model.selectionBinding) {
                Section("Do now") {
                    if snapshot.candidates.isEmpty {
                        NothingReady(snapshot: snapshot)
                    }
                    ForEach(snapshot.candidates) { candidate in
                        CandidateRow(candidate: candidate).tag(Subject.work(candidate.identity))
                    }
                    Button("Find any task or decision…") { model.findWork() }
                        .help("Search every task and decision (⌘F)")
                        .accessibilityIdentifier("now.find")
                }
                Section("Runs in progress") {
                    let active = snapshot.runs.filter { !$0.finished }
                    if active.isEmpty {
                        Text("None among \(snapshot.runsCoverage.words).\(snapshot.runsCoverage.truncated ? " Older runs may be in progress; they are found from their task in Detail." : "") A run's silence is never read as idle or finished.")
                            .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(active) { run in
                        RunRow(run: run).tag(Subject.run(run.identity))
                    }
                }
                if let status = snapshot.status, !status.blockers.isEmpty {
                    Section("What holds work back (\(status.blockers.count) things)") {
                        ForEach(status.blockers.prefix(25)) { group in
                            let target = self.target(group, snapshot: snapshot)
                            BlockerRow(group: group, opens: target != nil)
                                .tag(target ?? Subject.work("blocker:\(group.id)"))
                                .selectionDisabled(target == nil)
                        }
                        if status.blockers.count > 25 {
                            Text("\(status.blockers.count - 25) more are not listed here; find them with the search in Detail.").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
            }
            .frame(minWidth: 340, idealWidth: 440)
            .accessibilityIdentifier("list.now")
            SubjectPane(model: model, only: ["Summary", "Why now", "Objective", "Acceptance criteria", "Readiness", "Predecessors", "Run", "Decision", "Question", "Rationale"])
                .frame(minWidth: 320)
        }
    }

    /// What a held-back group opens: the decision that gates, or the predecessor that is awaited.
    private func target(_ group: BlockerGroup, snapshot: ObserverSnapshot) -> Subject? {
        switch group.kind {
        case .decision: return snapshot.inventory.decision(key: group.key).map { Subject.decision($0.identity) }
        case .dependency: return group.workIdentity.map { Subject.work($0) }
        default: return nil
        }
    }
}

struct CandidateRow: View {
    let candidate: Candidate

    var body: some View {
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            HStack(spacing: 6) {
                Text(candidate.key).font(.system(.callout, design: .monospaced))
                if !candidate.priority.isEmpty { Badge(text: "Priority \(candidate.priority)", symbol: "flag") }
                if candidate.critical { Badge(text: "On the critical path", symbol: "flame", tint: .orange) }
                Spacer()
                Text("rank \(candidate.rank)").font(.caption).foregroundStyle(.secondary)
            }
            Text(candidate.title).font(.headline).fixedSize(horizontal: false, vertical: true)
            ForEach(candidate.reasons.prefix(3), id: \.self) { reason in
                Text("• \(reason)").font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
        }
        .padding(.vertical, 2)
        .spoken("Ready work \(candidate.key): \(candidate.title)", value: "Priority \(candidate.priority). Rank \(candidate.rank). \(candidate.reasons.joined(separator: ". "))")
        .accessibilityIdentifier("row.now.\(candidate.key)")
    }
}

/// When `next` is empty: the counts and the reasons the application gave, in words.
struct NothingReady: View {
    let snapshot: ObserverSnapshot

    var body: some View {
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Label("No work is ready to start", systemImage: "pause.circle").font(.headline)
            if let status = snapshot.status {
                Text("\(status.totalWork) tasks: \(status.ready) ready, \(status.inFlight) in progress, \(status.blocked) blocked, \(status.awaitingVerification) awaiting verification, \(status.complete) complete. \(status.openDecisions) open decisions.")
                    .foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Text("The reasons are listed under “What holds work back”. An empty list is the application's answer, not a fault.").font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .spoken("No work is ready to start", value: snapshot.status.map { "\($0.totalWork) tasks: \($0.ready) ready, \($0.inFlight) in progress, \($0.blocked) blocked, \($0.awaitingVerification) awaiting verification, \($0.complete) complete. \($0.openDecisions) open decisions. The reasons are listed under what holds work back." })
        .accessibilityIdentifier("now.empty")
    }
}

struct BlockerRow: View {
    let group: BlockerGroup
    let opens: Bool

    var body: some View {
        HStack(alignment: .top) {
            Image(systemName: symbol).accessibilityHidden(true)
            VStack(alignment: .leading, spacing: 1) {
                Text(group.key).font(.system(.callout, design: .monospaced))
                Text("\(group.tasks) task\(group.tasks == 1 ? "" : "s") \(group.summary)").font(.callout).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                if opens { Text("Select to open it").font(.caption).foregroundStyle(.secondary) }
            }
        }
        .spoken("\(group.key): \(group.tasks) tasks \(group.summary)", value: opens ? "Select to open it" : nil)
        .accessibilityIdentifier("row.blocker.\(group.id)")
    }

    private var symbol: String {
        switch group.kind {
        case .decision: return "questionmark.diamond"
        case .dependency: return "link"
        case .lifecycle: return "clock"
        case .other: return "exclamationmark.circle"
        }
    }
}

struct RunRow: View {
    let run: RunSummary

    var body: some View {
        VStack(alignment: .leading, spacing: 2) {
            HStack(spacing: 6) {
                Text(run.workKey).font(.system(.callout, design: .monospaced))
                Badge(text: run.observation.text == "managed" ? "Managed" : "Reported only", symbol: run.observation.text == "managed" ? "gearshape.2" : "text.bubble")
                RunStatusBadge(run: run)
                Spacer()
            }
            Text("\(run.executor) · last heard \(displayTime(run.lastReceiptAt)) · \(run.recorded) records")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .spoken("Run of \(run.workKey)", value: runSpeech(run))
        .accessibilityIdentifier("row.run.\(run.identity)")
    }
}

func runSpeech(_ run: RunSummary) -> String {
    let how = run.observation.text == "managed" ? "managed" : "reported only"
    let freshness = run.status == "stale" ? "stale: nothing received recently, not known to be running or finished" : run.status
    return "By \(run.executor), \(how), \(freshness). Executor reports \(run.state). \(run.recorded) activity records."
}

struct RunStatusBadge: View {
    let run: RunSummary

    var body: some View {
        switch run.status {
        case "working": Badge(text: "Working", symbol: "play.circle", tint: .green)
        case "waiting": Badge(text: "Waiting for input", symbol: "hourglass", tint: .orange)
        case "stale": Badge(text: "Stale — not known to be running", symbol: "questionmark.circle", tint: .orange)
        case "unknown": Badge(text: "Unknown — another history", symbol: "questionmark.diamond", tint: .orange)
        case "completed": Badge(text: "Executor reports completed", symbol: "checkmark.circle", tint: .blue)
        case "failed": Badge(text: "Failed", symbol: "xmark.circle", tint: .red)
        case "interrupted": Badge(text: "Interrupted", symbol: "stop.circle", tint: .red)
        default: Badge(text: run.status, symbol: "circle")
        }
    }
}
