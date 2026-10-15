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
        // Each send arms a fresh event with the helper to stop. The helper is stopped at the first exchange after the send,
        // before the request is written: a read already on the wire is answered, and the send is the one left unanswered.
        let armed = LockedBox<Int32>(0), stopped = LockedBox(false), sent = LockedBox(Event())
        let model = try await openGanttModel(.database(first)) { settings in
            // No background reads: with a helper stopped, a read in flight would make the view stale between the check
            // that a send is possible and the send. Each workspace is read when it is opened or reconnected.
            settings.pollInterval = 3600
            settings.runsRefreshInterval = 3600
            // A replaced helper waits long enough for the lost state to be seen.
            settings.reconnectDelays = [0.5, 1]
            let audited = settings.onBlockingStep
            settings.onBlockingStep = { step in
                audited?(step)
                guard step == "exchange" else { return }
                let pid = armed.withLock { pid -> Int32 in defer { pid = 0 }; return pid }
                guard pid != 0 else { return }
                stopped.withLock { $0 = stopNow(pid) }
                sent.withLock { $0 }.signal()
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
        // While the helper is lost, applies and operations are both disabled with the reason shown.
        _ = try await modelExpect(model, "the lost helper to be shown", 30) { if case .reconnecting = $0.connection { return true } else { return false } }
        let lost = await MainActor.run { (model.applyBlocker, model.operationBlocker) }
        check(lost.0?.contains("not connected") == true && lost.1?.contains("not connected") == true,
              "while the helper is lost, applies and operations are disabled with the reason shown: \(lost)")
        // With no background reads, the replaced helper is read on request.
        await MainActor.run { model.reload() }
        _ = try await modelExpect(model, "the helper to be replaced and the view current", 60) { $0.counters.reconnects >= 1 && $0.connection == .connected && $0.freshness.current }
        let released = await MainActor.run { () -> String? in model.acknowledgeChanges(); return model.operationBlocker }
        check(released == nil, "after a reconnect the refused work was read again and the hold released: \(released ?? "none")")

        // A request left on the wire when another workspace is opened.
        let before = try await historyCount(first)
        guard let helper = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "no helper process") }
        _ = try await modelExpect(model, "operations to be possible before the claim", 30) { _ in MainActor.assumeIsolated { model.operationBlocker == nil } }
        let claimSent = Event()
        sent.withLock { $0 = claimSent }
        armed.withLock { $0 = helper }
        let held = await MainActor.run { () -> String? in
            if let blocker = model.operationBlocker { return blocker }
            model.operate(.claim, on: a)
            return nil
        }
        if let held { throw SuiteError(description: "the claim was held before it was sent: \(held)") }
        check(await claimSent.wait() && stopped.withLock({ $0 }), "the claim is on the wire to a stopped helper, unanswered")
        await MainActor.run { model.open(.database(second)) }
        // Operations are held as read-only until the workspace's identity says it is live, which can arrive just after its
        // first current read; the check below judges the live second workspace.
        _ = try await modelExpect(model, "the second workspace to be read as live", 60) { $0.inventory.basis != nil && $0.freshness.current && $0.identity?.source == "live" }
        let clean = await MainActor.run { (model.operations.phase == .idle, model.operationBlocker, model.applyBlocker) }
        check(clean.0 && clean.1 == nil, "the second workspace has nothing pending from the first: \(clean)")
        kill(helper, SIGKILL)

        await MainActor.run { model.open(.database(first)) }
        // Requests left in a workspace are restored once it is shown as that live workspace.
        _ = try await modelExpect(model, "the first workspace to be read again", 60) {
            $0.inventory.basis != nil && $0.freshness.current && $0.identity?.source == "live"
        }
        // The left request is restored when the workspace is adopted as the one shown, which may be a later snapshot.
        try await waitOperation(model, "the claim left on the wire to be restored") { $0.unresolved != nil }
        let restored = await MainActor.run { (model.operations.unresolved != nil, model.operationBlocker) }
        check(restored.0 && restored.1?.contains("Reconcile") == true, "coming back restores the claim as unknown, to be reconciled: \(restored)")
        await MainActor.run { model.reconcileOperation() }
        try await waitOperation(model, "the reconciled claim to settle") { if case .accepted = $0.phase { return true } else { return false } }
        let after = try await historyCount(first)
        let owner = try await cliJSON(first, ["show", "TEST-A"])["data"]["execution"]["owner"]["name"]
        check(after == before + 1 && owner == .string("pilot"), "the claim is recorded once, by its own workspace: \(after - before) operation(s)")
        let untouched = try await historyCount(second)
        check(untouched == 0, "the second workspace recorded nothing")

        // Close Workspace while an operation is on the wire, then open it again: the request is still to be reconciled.
        try await leaveWithRequestOnTheWire(model, first, armed: armed, stopped: stopped, sent: sent, what: "a start", leave: { model.close() }) {
            model.operate(.start, on: a)
        }
        await MainActor.run { model.reconcileOperation() }
        try await waitOperation(model, "the reconciled start to settle") { if case .accepted = $0.phase { return true } else { return false } }
        let started = try await cliJSON(first, ["show", "TEST-A"])["data"]["execution"]["status"]
        check(started == .string("InProgress"), "the start sent before Close Workspace is recorded once it is reconciled: \(started)")

        // A plan apply on the wire when another workspace is opened comes back with its draft, to be reconciled.
        let applied = try await historyCount(first)
        let c = await MainActor.run { model.snapshot.inventory.items.first { $0.key == "TEST-C" }?.identity }
        guard let c else { check(false, "TEST-C is readable"); return }
        await MainActor.run {
            model.setEditActor("human:planner")
            model.edit(.title(work: c, text: "Contract C, renamed while away"))
            model.reviewDraft()
        }
        try await waitEdit(model, "the draft to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        await MainActor.run { model.setEditReason("Rename C") }
        _ = try await modelExpect(model, "the view to be current before the apply", 30) { $0.freshness.current }
        try await leaveWithRequestOnTheWire(model, first, armed: armed, stopped: stopped, sent: sent, what: "an apply", leave: { model.open(.database(second)) }) {
            model.applyDraft()
        }
        let draft = await MainActor.run { (model.editing.unresolved != nil, model.editing.draft?.edits.count) }
        check(draft.0 && draft.1 == 1, "coming back restores the apply as unknown with its draft: \(draft)")
        await MainActor.run { model.reconcileApply() }
        try await waitEdit(model, "the reconciled apply to settle") { if case .applied = $0.phase { return true } else { return false } }
        let renamed = try await historyCount(first)
        check(renamed == applied + 1, "the apply sent before leaving is recorded once: \(renamed - applied) operation(s)")
        await closeGantt(model)
    }

    /// Send a request to a helper stopped as it goes on the wire, leave the workspace while it is unanswered, end that
    /// helper, and open the workspace again until it is current.
    private func leaveWithRequestOnTheWire(_ model: ObserverModel, _ database: URL, armed: LockedBox<Int32>, stopped: LockedBox<Bool>, sent: LockedBox<Event>, what: String,
                                           leave: @escaping @MainActor () -> Void, send: @escaping @MainActor () -> Void) async throws {
        // The send must be possible before the helper is stopped, or the guard would rightly hold it.
        _ = try await modelExpect(model, "\(what) to be possible", 30) { _ in
            MainActor.assumeIsolated { model.editing.reviewed != nil ? model.applyBlocker == nil : model.operationBlocker == nil }
        }
        guard let helper = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "no helper process") }
        let onWire = Event()
        sent.withLock { $0 = onWire }
        armed.withLock { $0 = helper }
        let held = await MainActor.run { () -> String? in
            let blocker = model.editing.reviewed != nil ? model.applyBlocker : model.operationBlocker
            if blocker == nil { send() }
            return blocker
        }
        if let held { throw SuiteError(description: "\(what) was held before it was sent: \(held)") }
        check(await onWire.wait() && stopped.withLock({ $0 }), "\(what) is on the wire to a stopped helper, unanswered")
        await MainActor.run { leave() }
        kill(helper, SIGKILL)
        await MainActor.run { model.open(.database(database)) }
        _ = try await modelExpect(model, "the workspace to be shown again after \(what)", 60) {
            $0.connection == .connected && $0.inventory.basis != nil && $0.freshness.current && $0.helperProcess != nil && $0.helperProcess != helper
                && $0.identity?.source == "live"
        }
    }
}
