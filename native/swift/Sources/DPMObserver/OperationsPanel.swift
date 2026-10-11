// The selected work's operations in the inspector: the owner's claim, start, block, unblock and submit, and an
// independent verify or reject, each sent at once as the acting actor. Nothing here decides whether an operation is
// allowed; the application answers, and its answer is shown as accepted, refused or unknown.

import DPMObserverCore
import SwiftUI

struct OperationsPanel: View {
    @ObservedObject var model: ObserverModel
    let item: WorkItem

    var body: some View {
        let blocker = model.operationBlocker
        VStack(alignment: .leading, spacing: 8) {
            Text("Operations").font(.subheadline.weight(.semibold))
            Text("\(item.status)\(item.owner.map { " · owner \($0)" } ?? "") · as \(model.editing.actor)").font(.caption).foregroundStyle(.secondary)
            HStack {
                action("Claim", .claim, id: "ops.claim")
                action("Start", .start, id: "ops.start")
                action("Unblock", .unblock, id: "ops.unblock")
            }
            .disabled(blocker != nil)
            TextField("Reason (block or reject)", text: Binding(get: { model.operations.reason }, set: { model.setOperationReason($0) }))
                .textFieldStyle(.roundedBorder).accessibilityLabel("Reason for a block or a reject").accessibilityIdentifier("ops.reason")
            TextField("Note (submit or verify)", text: Binding(get: { model.operations.note }, set: { model.setOperationNote($0) }))
                .textFieldStyle(.roundedBorder).accessibilityLabel("Note for a submit or a verify").accessibilityIdentifier("ops.note")
            HStack {
                action("Block", .block(reason: model.operations.reason), id: "ops.block")
                    .disabled(blocker != nil || model.operations.reason.trimmingCharacters(in: .whitespaces).isEmpty)
                action("Submit", .submit(note: model.operations.note), id: "ops.submit").disabled(blocker != nil)
            }
            Text("Independent review").font(.caption.weight(.semibold)).padding(.top, 2)
            HStack {
                action("Verify", .verify(note: model.operations.note), id: "ops.verify").disabled(blocker != nil)
                action("Reject", .reject(reason: model.operations.reason), id: "ops.reject")
                    .disabled(blocker != nil || model.operations.reason.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            .help("A reviewer other than the work's holders and evidence authors; the application refuses anyone else")
            outcome
            if let blocker { Label(blocker, systemImage: "lock").font(.caption).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true) }
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("ops.panel")
    }

    private func action(_ title: String, _ operation: TaskOperation, id: String) -> some View {
        Button(title) { model.operate(operation, on: item.identity) }
            .accessibilityLabel("\(title) \(item.key)")
            .accessibilityIdentifier(id)
    }

    @ViewBuilder private var outcome: some View {
        switch model.operations.phase {
        case .idle:
            EmptyView()
        case .sending(let sent):
            Label("Sending \(sent.operation.words)…", systemImage: "arrow.up.circle").font(.caption)
        case .accepted(let work, let operation, let revision):
            Label("\(operation.words.capitalized) \(model.editName(work)) accepted at revision \(revision.map(String.init) ?? "?").", systemImage: "checkmark.circle")
                .font(.caption).accessibilityIdentifier("ops.accepted")
        case .refused(let work, let operation, let refusal, _):
            VStack(alignment: .leading, spacing: 4) {
                Label("\(operation.words.capitalized) \(model.editName(work)) refused: \(refusal.message)", systemImage: "xmark.octagon")
                    .font(.caption).fixedSize(horizontal: false, vertical: true).accessibilityIdentifier("ops.refused")
                if let observed = model.operations.staleObservation { changes(since: observed, work: work) }
            }
        case .unknown(let sent, let cause):
            VStack(alignment: .leading, spacing: 4) {
                Label("Whether \(sent.operation.words) was recorded is unknown: \(cause)", systemImage: "questionmark.circle")
                    .font(.caption).fixedSize(horizontal: false, vertical: true)
                Button("Reconcile") { model.reconcileOperation() }
                    .accessibilityIdentifier("ops.reconcile")
                    .help("Resend request \(sent.request.operationId) to learn what became of it")
            }
        }
    }

    /// The work as it was refused beside the work read again, before anything is sent again.
    @ViewBuilder private func changes(since observed: ObservedWork, work: String) -> some View {
        let now = model.snapshot.inventory.byIdentity[work]
        let reread = (model.snapshot.inventory.basis?.revision ?? observed.revision) != observed.revision
        VStack(alignment: .leading, spacing: 2) {
            Text("Changed since revision \(observed.revision):").font(.caption.weight(.semibold))
            Text("status \(observed.status) → \(now?.status ?? "?")").font(.caption)
            Text("owner \(observed.owner ?? "none") → \(now?.owner ?? "none")").font(.caption)
            Text("progress \(observed.progress)% → \(now?.reportedProgress ?? 0)%").font(.caption)
            Button("I have read the changes") { model.acknowledgeChanges() }
                .disabled(!reread)
                .accessibilityIdentifier("ops.acknowledge")
                .help(reread ? "Allow the next operation against the plan as read now" : "Waiting for the plan to be read again")
        }
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("ops.changes")
    }
}
