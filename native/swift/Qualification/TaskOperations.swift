// Task operations from the native app go through the application's commands with the acting actor, a request
// identity and the observed revision and lineage, and end visibly: accepted, refused with the reason, or unknown and
// then reconciled. 100% progress unlocks nothing, self-verification is refused, a same-identity resend records
// nothing new, a stale send keeps what was typed and asks for the changes to be read, and an unknown outcome holds
// every further command until the same request is reconciled.

import Combine
import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func taskOperationsGoThroughTheApplication() async throws {
        print("operations: claim, start, progress, block, unblock, submit, an independent verify; refusals, retries, stale sends and unknown outcomes")
        let database = try await newStore("task-operations")
        let armed = LockedBox(false), sent = Event()
        // No background reads: the stale send below is made against a revision the model has not read yet.
        let model = try await openGanttModel(.database(database)) { settings in
            settings.pollInterval = 3600
            let audited = settings.onBlockingStep
            settings.onBlockingStep = { step in
                audited?(step)
                if step == "exchange", armed.withLock({ $0 }) { sent.signal() }
            }
        }
        _ = try await modelExpect(model, "the plan to be read", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let found = await MainActor.run { model.snapshot.inventory.items.first { $0.key == "TEST-A" }?.identity }
        guard let a = found else { check(false, "the fixture is readable"); return }

        /// Send one operation as `actor` and wait for its answer.
        func operate(_ operation: TaskOperation, as actor: String) async throws -> OperationState.Phase {
            let blocker = await MainActor.run { () -> String? in model.setEditActor(actor); return model.operationBlocker }
            if let blocker { throw SuiteError(description: "\(operation.words) could not be sent: \(blocker)") }
            await MainActor.run { model.operate(operation, on: a) }
            try await waitOperation(model, "\(operation.words) to be answered") {
                switch $0.phase {
                case .accepted, .refused, .unknown: return true
                default: return false
                }
            }
            let phase = await MainActor.run { model.operations.phase }
            // An accepted operation holds the next one until the plan is read at its revision.
            if case .accepted(_, _, let revision?) = phase {
                _ = try await modelExpect(model, "the plan to be read at revision \(revision)", 30) { ($0.inventory.basis?.revision ?? 0) >= revision && $0.freshness.current }
            }
            return phase
        }
        func accepted(_ phase: OperationState.Phase) -> Bool { if case .accepted = phase { return true } else { return false } }

        await MainActor.run { model.setEditing(true) }
        for operation in [TaskOperation.claim, .start, .progress(percent: 100)] {
            let phase = try await operate(operation, as: "agent:pilot")
            check(accepted(phase), "\(operation.words) by the owner is accepted with its revision: \(phase)")
        }
        let successor = try await cliJSON(database, ["explain", "TEST-B"])["data"]["ready"]
        check(successor == .bool(false), "100% progress unlocks nothing: TEST-B is not ready")
        for operation in [TaskOperation.block(reason: "Fixture missing"), .unblock] {
            let phase = try await operate(operation, as: "agent:pilot")
            check(accepted(phase), "\(operation.words) by the owner is accepted: \(phase)")
        }
        // The same request again is answered with the operation already recorded, and records nothing.
        let history = try await historyCount(database)
        let (engine, basis) = await MainActor.run { (model.engine, model.snapshot.inventory.basis) }
        if let engine, let basis, let actor = EditCommands.actor("agent:pilot") {
            let request = CommandRequest(actor: actor, baseRevision: basis.revision, baseLineage: basis.lineage, operationId: UUIDv7.make(),
                                         command: .object(["ReportProgress": .object(["work": .string(a), "percent": .integer(100), "note": .null])]))
            let first = await engine.apply(request)
            let again = await engine.apply(request)
            let recorded = try await historyCount(database)
            check(first == again && recorded == history + 1, "a same-identity resend returns the recorded operation: \(first) \(again)")
        }

        // The resend went around the model, so the model reads the plan again before the next operation.
        await MainActor.run { model.reload() }
        _ = try await modelExpect(model, "the plan to be read after the resend", 30) { ($0.inventory.basis?.revision ?? 0) >= 6 && $0.freshness.current }
        let submitted = try await operate(.submit(note: "Evidence attached"), as: "agent:pilot")
        check(accepted(submitted), "submit by the owner is accepted: \(submitted)")
        await MainActor.run { model.setOperationNote("Looks done") }
        let selfReview = try await operate(.verify(note: "Looks done"), as: "agent:pilot")
        if case .refused(_, _, let refusal, _) = selfReview {
            check(refusal.fromApplication && !refusal.message.isEmpty, "the submitting actor's verify is refused with the application's reason: \(refusal.message)")
        } else {
            check(false, "self-verification is refused: \(selfReview)")
        }
        let typed = await MainActor.run { model.operations.note }
        check(typed == "Looks done", "a refusal keeps what was typed")

        // A send against a revision the model has not read is refused as stale; nothing more is sent until the work is
        // read again and its changes are looked at.
        let decided = try await cliJSON(database, ["decide", "TEST-GATE", "Proceed", "--actor", "human:lead"])["revision"].uint ?? 0
        await MainActor.run { model.setOperationReason("Report incomplete") }
        let stale = try await operate(.reject(reason: "Report incomplete"), as: "human:reviewer")
        if case .refused(_, _, let refusal, _) = stale { check(refusal.stale, "a send from an older revision is refused as stale: \(refusal.code)") }
        let held = await MainActor.run { (model.operationBlocker, model.operations.reason) }
        check(held.0?.contains("look at its changes") == true && held.1 == "Report incomplete", "the stale refusal keeps the reason and asks for the changes first: \(held)")
        _ = try await modelExpect(model, "the plan to be read again", 30) { ($0.inventory.basis?.revision ?? 0) >= decided && $0.freshness.current }
        let cleared = await MainActor.run { () -> String? in model.acknowledgeChanges(); return model.operationBlocker }
        check(cleared == nil, "once the changes were looked at, operations may be sent: \(cleared ?? "none")")

        // An unknown outcome holds every further command until the same request is reconciled, which records it once.
        guard let pid = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "no helper process") }
        let before = try await historyCount(database)
        check(await freeze(pid), "the helper is stopped (its stop was awaited as an event)")
        armed.withLock { $0 = true }
        await MainActor.run { model.setEditActor("human:reviewer"); model.operate(.verify(note: "Checked"), on: a) }
        check(await sent.wait(), "the verify is on the wire, unanswered")
        kill(pid, SIGKILL)
        try await waitOperation(model, "the lost answer to leave the outcome unknown") { $0.unresolved != nil }
        let blocked = await MainActor.run { () -> (String?, String?) in
            model.operate(.reject(reason: "Second thoughts"), on: a)
            return (model.operationBlocker, model.applyBlocker)
        }
        check(blocked.0?.contains("Reconcile") == true && blocked.1?.contains("Reconcile") == true,
              "while the outcome is unknown no operation and no plan apply may be sent: \(blocked)")
        await MainActor.run { model.reload() }
        _ = try await modelExpect(model, "the lost helper to be replaced", 60) { $0.connection == .connected && $0.counters.reconnects >= 1 }
        await MainActor.run { model.reconcileOperation() }
        try await waitOperation(model, "the reconciled verify to settle") { if case .accepted = $0.phase { return true } else { return false } }
        let status = try await cliJSON(database, ["show", "TEST-A"])["data"]["execution"]["status"]
        let after = try await historyCount(database)
        check(status == .string("Verified") && after == before + 1, "the reconciled verify is recorded once: \(status)")
        await closeGantt(model)
    }

    /// Wait on the main actor for a published operation state that satisfies `predicate`; driven by the model's
    /// own publications, never by polling.
    @MainActor
    func waitOperation(_ model: ObserverModel, _ what: String, _ seconds: TimeInterval = 30, _ predicate: @escaping (OperationState) -> Bool) async throws {
        if predicate(model.operations) { return }
        let found: Bool = await withCheckedContinuation { continuation in
            var cancellable: AnyCancellable?
            var done = false
            cancellable = model.$operations.sink { state in
                guard !done, predicate(state) else { return }
                done = true
                continuation.resume(returning: true)
                cancellable?.cancel()
            }
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds) {
                guard !done else { return }
                done = true
                continuation.resume(returning: false)
                cancellable?.cancel()
            }
        }
        if !found { throw SuiteError(description: "timed out after \(Int(seconds)) s waiting for \(what); operation phase \(model.operations.phase)") }
    }
}
