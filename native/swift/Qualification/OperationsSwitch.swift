// A request's answer settles only the workspace it was sent to. Opening another workspace while a request is on the
// wire leaves the other one clean; coming back restores the request as unknown, to be reconciled, and reconciling
// records it once. A refusal is released by a read of the work even when the helper was replaced in between.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func operationsSettleOnlyTheirWorkspace() async throws {
        print("operations: an answer settles only its own workspace; a request left on the wire is restored and reconciled once")
        let first = try await newStore("operations-first")
        let second = try await newStore("operations-second")
        let armed = LockedBox(false), sent = Event()
        let model = try await openGanttModel(.database(first)) { settings in
            let audited = settings.onBlockingStep
            settings.onBlockingStep = { step in
                audited?(step)
                if step == "exchange", armed.withLock({ $0 }) { sent.signal() }
            }
        }
        _ = try await modelExpect(model, "the first workspace to be read", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let found = await MainActor.run { model.snapshot.inventory.items.first { $0.key == "TEST-A" }?.identity }
        guard let a = found else { check(false, "the fixture is readable"); return }
        await MainActor.run { model.setEditing(true); model.setEditActor("agent:pilot") }

        // A refusal, then a replaced helper: a read after the reconnect still releases the hold.
        await MainActor.run { model.operate(.start, on: a) }
        try await waitOperation(model, "starting unclaimed work to be refused") { if case .refused = $0.phase { return true } else { return false } }
        guard let pid = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "no helper process") }
        kill(pid, SIGKILL)
        _ = try await modelExpect(model, "the helper to be replaced and the view current", 60) { $0.counters.reconnects >= 1 && $0.connection == .connected && $0.freshness.current }
        let released = await MainActor.run { () -> String? in model.acknowledgeChanges(); return model.operationBlocker }
        check(released == nil, "after a reconnect the refused work was read again and the hold released: \(released ?? "none")")

        // A request left on the wire when another workspace is opened.
        let before = try await historyCount(first)
        guard let helper = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "no helper process") }
        check(await freeze(helper), "the first workspace's helper is stopped (its stop was awaited as an event)")
        armed.withLock { $0 = true }
        await MainActor.run { model.operate(.claim, on: a) }
        check(await sent.wait(), "the claim is on the wire, unanswered")
        armed.withLock { $0 = false }
        await MainActor.run { model.open(.database(second)) }
        _ = try await modelExpect(model, "the second workspace to be read", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let clean = await MainActor.run { (model.operations.phase == .idle, model.operationBlocker, model.applyBlocker) }
        check(clean.0 && clean.1 == nil, "the second workspace has nothing pending from the first: \(clean)")
        kill(helper, SIGKILL)

        await MainActor.run { model.open(.database(first)) }
        _ = try await modelExpect(model, "the first workspace to be read again", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let restored = await MainActor.run { (model.operations.unresolved != nil, model.operationBlocker) }
        check(restored.0 && restored.1?.contains("Reconcile") == true, "coming back restores the claim as unknown, to be reconciled: \(restored)")
        await MainActor.run { model.reconcileOperation() }
        try await waitOperation(model, "the reconciled claim to settle") { if case .accepted = $0.phase { return true } else { return false } }
        let after = try await historyCount(first)
        let owner = try await cliJSON(first, ["show", "TEST-A"])["data"]["execution"]["owner"]["name"]
        check(after == before + 1 && owner == .string("pilot"), "the claim is recorded once, by its own workspace: \(after - before) operation(s)")
        let untouched = try await historyCount(second)
        check(untouched == 0, "the second workspace recorded nothing")
        await closeGantt(model)
    }
}
