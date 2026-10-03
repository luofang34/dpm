// Review: submitted work, its acceptance criteria and its evidence, with verification kept apart
// from anything a run reported. Only an independent verifier's project operation accepts work.

import DPMObserverCore
import SwiftUI

struct ReviewView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        HSplitView {
            List(selection: model.selectionBinding) {
                let waiting = snapshot.inventory.submitted
                Section("Awaiting independent verification (\(waiting.count))") {
                    if waiting.isEmpty {
                        Text("Nothing is submitted and waiting. Work becomes reviewable when its owner submits it.").foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                    }
                    ForEach(waiting) { item in
                        ReviewRow(item: item).tag(Subject.work(item.identity))
                    }
                }
                let verified = snapshot.inventory.items.filter { $0.status == "Verified" }
                if !verified.isEmpty {
                    Section("Accepted (\(verified.count))") {
                        ForEach(verified.prefix(25)) { item in
                            ReviewRow(item: item).tag(Subject.work(item.identity))
                        }
                        if verified.count > 25 {
                            Text("\(verified.count - 25) more accepted tasks are found with the search in Detail.").font(.caption).foregroundStyle(.secondary)
                        }
                    }
                }
                if let status = snapshot.status, status.awaitingVerification != snapshot.inventory.submitted.count {
                    Text("The application counts \(status.awaitingVerification) awaiting verification but \(snapshot.inventory.submitted.count) are listed; the list is refreshing.")
                        .font(.caption).foregroundStyle(.orange).fixedSize(horizontal: false, vertical: true)
                }
            }
            .frame(minWidth: 340, idealWidth: 440)
            .accessibilityIdentifier("list.review")
            ReviewPane(model: model).frame(minWidth: 360)
        }
    }
}

struct ReviewRow: View {
    let item: WorkItem

    var body: some View {
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            HStack(spacing: 6) {
                Text(item.key).font(.system(.callout, design: .monospaced))
                Badge(text: item.status == "Verified" ? "Verified" : "Awaiting verification", symbol: item.status == "Verified" ? "checkmark.seal.fill" : "hourglass", tint: item.status == "Verified" ? .green : .orange)
                Spacer()
            }
            Text(item.title).font(.headline).fixedSize(horizontal: false, vertical: true)
            Text("Owner \(item.owner ?? "none") · submitted \(displayTime(item.submittedAt)) · \(item.acceptance.count) acceptance criteria")
                .font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }
        .spoken("\(item.status == "Verified" ? "Accepted" : "Awaiting independent verification") work \(item.key): \(item.title)", value: "Owner \(item.owner ?? "none"). \(item.acceptance.count) acceptance criteria.")
        .accessibilityIdentifier("row.review.\(item.key)")
    }
}

struct ReviewPane: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        if case .work(let identity)? = model.selection, let item = snapshot.inventory.byIdentity[identity] {
            ScrollView {
                VStack(alignment: .leading, spacing: Layout.margin) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(item.title).font(.title2.weight(.semibold)).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                        Text("\(item.key) · \(item.status) · owner \(item.owner ?? "none")").foregroundStyle(.secondary)
                    }
                    .spoken(item.title, value: "\(item.key), \(item.status), owner \(item.owner ?? "none")")
                    .accessibilityAddTraits(.isHeader)
                    verification(item)
                    VStack(alignment: .leading, spacing: Layout.rowSpacing) {
                        Text("Acceptance criteria").font(.headline).accessibilityAddTraits(.isHeader)
                        if item.acceptance.isEmpty { Text("None recorded.").foregroundStyle(.secondary) }
                        ForEach(Array(item.acceptance.enumerated()), id: \.offset) { index, text in
                            Prose(text: "\(index + 1). \(text)").spoken("Acceptance criterion \(index + 1)", value: text)
                        }
                    }
                    .accessibilityIdentifier("review.acceptance")
                    evidence(item, snapshot: snapshot)
                    RelatedView(model: model, identity: identity)
                    Button("Open in Detail") { model.show(.detail) }.keyboardShortcut("d", modifiers: .command)
                }
                .padding(Layout.margin)
            }
            .accessibilityIdentifier("review.scroll")
        } else {
            EmptyState(symbol: "checkmark.seal", title: "Select submitted work", message: "Review shows the acceptance criteria and the evidence. Verification is a separate, independent act that this app does not perform.")
        }
    }

    private func verification(_ item: WorkItem) -> some View {
        let headline = item.status == "Verified" ? "Accepted by an independent verifier" : (item.status == "Submitted" ? "Verification pending" : "Not submitted: \(item.status)")
        let detail = item.status == "Submitted" ? "Only a project operation by an independent verifier accepts this work. A run that completed and a 100% progress report are the executor's words, not acceptance." : ""
        let attempts = item.attempts.map { "Attempt \($0.number): submitted \(displayTime($0.submittedAt)); outcome \($0.outcome)." }
        return VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Label(headline, systemImage: item.status == "Verified" ? "checkmark.seal.fill" : (item.status == "Submitted" ? "hourglass" : "circle.dashed")).font(.headline)
            if !detail.isEmpty { Prose(text: detail, font: .callout) }
            ForEach(attempts, id: \.self) { Prose(text: $0, font: .callout) }
        }
        .padding(8)
        .background(Color.secondary.opacity(0.1), in: RoundedRectangle(cornerRadius: 6))
        .spoken(headline, value: ([detail] + attempts).filter { !$0.isEmpty }.joined(separator: " "))
        .accessibilityIdentifier("review.verification")
    }

    private func evidence(_ item: WorkItem, snapshot: ObserverSnapshot) -> some View {
        let artifacts = item.artifactIds.compactMap { snapshot.inventory.artifacts[$0] }
        let note = snapshot.operations.last(where: { $0.workIdentity == item.identity && $0.verb == "Submit" })?.note
        return VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Text("Evidence").font(.headline).accessibilityAddTraits(.isHeader)
            if artifacts.isEmpty { Text("No artifact is attached to this work.").foregroundStyle(.secondary) }
            ForEach(artifacts, id: \.identity) { artifact in
                let text = "\(artifact.kind) \(artifact.uri)\(artifact.role.map { " (\($0))" } ?? "")"
                Prose(text: "\(artifact.label) — \(text)").spoken(artifact.label, value: text)
            }
            if let note = note { Prose(text: "Submission note: \(note)").spoken("Submission note", value: note) }
        }
        .accessibilityIdentifier("review.evidence")
    }
}

/// The runs of one task, read for it with how much of them was read, and the recent operations
/// naming it, each kept to its own kind of fact. A run is opened by its own identity.
struct RelatedView: View {
    @ObservedObject var model: ObserverModel
    let identity: String

    var body: some View {
        let snapshot = model.snapshot
        let operations = snapshot.operations.filter { $0.workIdentity == identity }
        VStack(alignment: .leading, spacing: Layout.margin) {
            VStack(alignment: .leading, spacing: Layout.rowSpacing) {
                if let loaded = snapshot.workRuns, loaded.work == identity {
                    Text("Runs of this task (\(loaded.runs.count))").font(.headline).accessibilityAddTraits(.isHeader)
                    Prose(text: loaded.runs.isEmpty ? "The application holds no run for this task." : "Read for this task: \(loaded.coverage.words).", font: .callout)
                    ForEach(loaded.runs) { run in
                        Button {
                            model.select(.run(run.identity))
                            model.show(.live)
                        } label: {
                            Text("\(run.executor) · \(run.observation.text == "managed" ? "managed" : "reported only") · executor reports \(run.state), observed \(run.status) · \(run.recorded) records")
                                .fixedSize(horizontal: false, vertical: true).multilineTextAlignment(.leading)
                        }
                        .buttonStyle(.link)
                        .help("Open this run in Live")
                        .accessibilityLabel("Open run by \(run.executor), \(run.status)")
                        .accessibilityValue(runSpeech(run))
                    }
                } else {
                    Text("Runs of this task").font(.headline).accessibilityAddTraits(.isHeader)
                    ProgressView("Reading this task's runs…")
                }
            }
            .accessibilityIdentifier("related.runs")
            VStack(alignment: .leading, spacing: Layout.rowSpacing) {
                Text("Recent project operations naming it (\(operations.count))").font(.headline).accessibilityAddTraits(.isHeader)
                if operations.isEmpty { Text("None among the \(snapshot.operations.count) most recent operations read.").foregroundStyle(.secondary) }
                ForEach(operations.reversed()) { operation in
                    let text = "\(displayTime(operation.timestamp)) — \(operation.actor) \(operation.verb)\(operation.note.map { ": \($0)" } ?? "") (revision \(operation.resultingRevision.map { String($0) } ?? "?"))"
                    Prose(text: text).spoken(text)
                }
            }
        }
    }
}
