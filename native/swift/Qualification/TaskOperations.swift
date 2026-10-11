// Task operations from the native app go through the application's commands with the acting actor, a request
// identity and the observed revision and lineage, and end visibly: accepted, refused with the reason, or unknown and
// then reconciled. 100% progress unlocks nothing and only the accepted verify does; self-verification is refused; a
// same-identity resend records nothing new; every refusal keeps what was typed and holds further operations until
// the work was read again and its changes looked at; an unknown outcome holds every further command until the same
// request is reconciled; a bar drag only stages progress; and another workspace starts with nothing pending.

import Combine
import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func taskOperationsGoThroughTheApplication() async throws {
        print("operations: the owner's operations, an independent reject and verify; refusals, retries, stale sends and unknown outcomes")
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

        /// Send one operation as `actor` and wait for its answer; an accepted one is read before the next is sent.
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
            if case .accepted(_, _, let revision?) = phase {
                _ = try await modelExpect(model, "the plan to be read at revision \(revision)", 30) { ($0.inventory.basis?.revision ?? 0) >= revision && $0.freshness.current }
            }
            return phase
        }
        /// After a refusal: held until the work is read again, then released by looking at its changes.
        func readChanges(_ what: String) async throws {
            // Background reads are off here, so the plan is read on request until it is current.
            await MainActor.run { model.reload() }
            _ = try await modelExpect(model, "the view to be current after \(what)", 30) { $0.freshness.current }
            let held = await MainActor.run { model.operationBlocker }
            check(held?.contains("look at its changes") == true, "\(what): operations are held until the changes are read: \(held ?? "not held")")
            let observed = await MainActor.run { model.operations.staleObservation }
            guard let observed else { check(false, "\(what): the refusal records the work as it was"); return }
            _ = try await modelExpect(model, "the work to be read again after \(what)", 30) { _ in MainActor.assumeIsolated { model.rereadSince(observed) } }
            let cleared = await MainActor.run { () -> String? in model.acknowledgeChanges(); return model.operationBlocker }
            check(cleared == nil, "\(what): once the changes were read, operations may be sent: \(cleared ?? "none")")
        }
        func accepted(_ phase: OperationState.Phase) -> Bool { if case .accepted = phase { return true } else { return false } }
        func ready(_ key: String) async throws -> JSON { try await cliJSON(database, ["explain", key])["data"]["ready"] }

        await MainActor.run { model.setEditing(true) }
        for operation in [TaskOperation.claim, .start, .progress(percent: 100), .block(reason: "Fixture missing"), .unblock] {
            let phase = try await operate(operation, as: "agent:pilot")
            check(accepted(phase), "\(operation.words) by the owner is accepted with its revision: \(phase)")
        }
        check(try await ready("TEST-B") == .bool(false), "100% progress unlocks nothing: TEST-B is not ready")

        // A drag on the bar only stages a share; nothing is recorded until Report is pressed.
        let staged = try await historyCount(database)
        await MainActor.run { model.stageProgress(work: a, percent: 55) }
        let stagedState = await MainActor.run { model.operations.stagedProgress.map { "\($0.percent)" } }
        let stagedHistory = try await historyCount(database)
        check(stagedState == "55" && stagedHistory == staged, "a staged share records nothing until it is reported")
        await MainActor.run { model.dropStagedProgress() }

        // The same request again is answered with the operation already recorded, and records nothing.
        let history = try await historyCount(database)
        let (engine, basis) = await MainActor.run { (model.engine, model.snapshot.inventory.basis) }
        if let engine, let basis, let actor = EditCommands.actor("agent:pilot") {
            let request = CommandRequest(actor: actor, baseRevision: basis.revision, baseLineage: basis.lineage, operationId: UUIDv7.make(),
                                         command: .object(["ReportProgress": .object(["work": .string(a), "percent": .integer(90), "note": .null])]))
            let first = await engine.apply(request)
            let again = await engine.apply(request)
            let recorded = try await historyCount(database)
            check(first == again && recorded == history + 1, "a same-identity resend returns the recorded operation: \(first) \(again)")
            // The resend went around the model, which reads the plan before the next operation.
            await MainActor.run { model.reload() }
            _ = try await modelExpect(model, "the plan to be read after the resend", 30) { ($0.inventory.basis?.revision ?? 0) > basis.revision && $0.freshness.current }
        }

        check(accepted(try await operate(.submit(note: "Evidence attached"), as: "agent:pilot")), "submit by the owner is accepted")
        await MainActor.run { model.setOperationNote("Looks done") }
        let selfReview = try await operate(.verify(note: "Looks done"), as: "agent:pilot")
        if case .refused(_, _, let refusal, _) = selfReview {
            check(refusal.fromApplication && refusal.message.contains("cannot verify"), "the submitting actor's verify is refused with the application's reason: \(refusal.message)")
        } else {
            check(false, "self-verification is refused: \(selfReview)")
        }
        let note = await MainActor.run { model.operations.note }
        check(note == "Looks done", "a refusal keeps what was typed")
        try await readChanges("the refused self-verification")

        // A send against a revision the model has not read is refused as stale and held the same way.
        let decided = try await cliJSON(database, ["decide", "TEST-GATE", "Proceed", "--actor", "human:lead"])["revision"].uint ?? 0
        await MainActor.run { model.setOperationReason("Report incomplete") }
        let stale = try await operate(.reject(reason: "Report incomplete"), as: "human:reviewer")
        if case .refused(_, _, let refusal, _) = stale {
            check(refusal.stale, "a send from an older revision is refused as stale: \(refusal.code)")
        } else {
            check(false, "a send from an older revision is refused: \(stale)")
        }
        let reason = await MainActor.run { model.operations.reason }
        check(reason == "Report incomplete", "the stale refusal keeps the reason")
        _ = try await modelExpect(model, "the decision to be read", 30) { ($0.inventory.basis?.revision ?? 0) >= decided }
        try await readChanges("the stale reject")

        // An independent reject is accepted and returns the work to its owner, who submits again.
        check(accepted(try await operate(.reject(reason: "Report incomplete"), as: "human:reviewer")), "an independent reject is accepted")
        check(accepted(try await operate(.submit(note: "Report completed"), as: "agent:pilot")), "the owner submits again")
        check(try await ready("TEST-B") == .bool(false), "a submission unlocks nothing: TEST-B is still not ready")

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
        check(try await ready("TEST-B") == .bool(true), "only the accepted verify unlocks TEST-B")

        // Another workspace starts with nothing pending: no request of this one is reconciled or awaited there.
        let other = try await newStore("task-operations-other")
        await MainActor.run { model.stageProgress(work: a, percent: 20); model.open(.database(other)) }
        let reset = await MainActor.run { model.operations.phase == .idle && model.operations.stagedProgress == nil && model.operations.awaitedRevision == nil }
        check(reset, "opening another workspace forgets the operations of this one")
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
