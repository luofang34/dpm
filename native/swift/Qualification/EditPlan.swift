// Editing the plan from the native app goes through the application's review and apply, in that order: a
// validation is a read that records nothing, an apply records one operation with its actor and reason, a resend of
// the same request records nothing more, cycles and protected execution are refused with the revision unchanged,
// and a stale apply keeps the draft until it is read again and reviewed.

import Combine
import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func editsGoThroughReviewAndApply() async throws {
        print("edit: drafts are validated without a write, applied once with actor and reason, and refused for cycles, protected work and stale bases")
        let database = try await newStore("edit-apply")
        let model = try await openGanttModel(.database(database))
        _ = try await modelExpect(model, "the plan to be read for editing", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let (engine, basis, ids) = await MainActor.run { () -> (ObserverEngine?, PlanBasis?, [String: String]) in
            (model.engine, model.snapshot.inventory.basis, Dictionary(model.snapshot.inventory.items.map { ($0.key, $0.identity) }, uniquingKeysWith: { a, _ in a }))
        }
        guard let engine, let basis, let a = ids["TEST-A"], let b = ids["TEST-B"], let c = ids["TEST-C"], let f = ids["TEST-F"] else {
            check(false, "the edit fixture is readable"); return
        }
        let historyBefore = try await cliJSON(database, ["history"])["data"]["entries"].array?.count ?? 0
        let revisionBefore = basis.revision

        // A validation is a read: twice over, nothing is recorded.
        var draft = PlanDraft(basis: basis)
        draft.add(.link(id: UUID().uuidString.lowercased(), from: a, fromEnd: .finish, to: c, toEnd: .start, lagHours: 2))
        draft.add(.estimate(work: b, optimistic: 12, likely: 22, pessimistic: 40))
        let first = await engine.propose(draft.candidate(), basis: draft.basis)
        let second = await engine.propose(draft.candidate(), basis: draft.basis)
        guard case .success(let preview) = first, case .success(let again) = second else {
            check(false, "a link and an estimate validate: \(first) \(second)"); return
        }
        check(preview == again && preview.revision == revisionBefore && preview.changeList.count == 2,
              "a repeated validation gives the same diff at the observed revision: \(preview.changeList.count) changes")
        let historyAfterValidation = try await cliJSON(database, ["history"])["data"]["entries"].array?.count ?? 0
        let revisionAfterValidation = try await cliJSON(database, ["revision"])["revision"].uint
        check(historyAfterValidation == historyBefore && revisionAfterValidation == revisionBefore, "validation recorded nothing and kept the revision")

        // The ends a drag joins name the relation, and every other kind of edit validates through the application.
        check(PlanEdit.kind(from: .finish, to: .start) == "FinishStart" && PlanEdit.kind(from: .start, to: .start) == "StartStart"
              && PlanEdit.kind(from: .finish, to: .finish) == "FinishFinish" && PlanEdit.kind(from: .start, to: .finish) == "StartFinish",
              "the joined ends name FS, SS, FF and SF")
        if let d = ids["TEST-D"], let gate = basis.plan["decisions"].objectKeys.first {
            var every = PlanDraft(basis: basis)
            let fresh = NewTask(key: "TEST-NEW", title: "A task added in the app", objective: "Prove a new task enters the plan Proposed.",
                                acceptance: ["The task exists and is Proposed."], parent: nil, estimate: (optimistic: 1, likely: 2, pessimistic: 4))
            every.add(.title(work: d, text: "Contract D, renamed"))
            every.add(.acceptance(work: d, criteria: ["Result D passes its independent check.", "Its evidence is attached."]))
            every.add(.newTask(fresh))
            every.add(.gate(decision: gate, work: c, blocks: true))
            every.add(.link(id: UUID().uuidString.lowercased(), from: fresh.id, fromEnd: .finish, to: f, toEnd: .finish, lagHours: -1))
            let all = await engine.propose(every.candidate(), basis: every.basis)
            if case .success(let review) = all {
                check(review.changeList.count >= 4, "title, acceptance, a new task, a gate and an FF lead validate together: \(review.changeList.count) changes")
            } else {
                check(false, "title, acceptance, a new task, a gate and an FF lead validate together: \(all)")
            }
        }

        // An apply records one operation with its actor and reason; the same request again records nothing more.
        let actor = EditCommands.actor("human:planner")!
        let request = EditCommands.apply(preview, reason: "Link A to C and widen B", actor: actor)
        let applied = await engine.apply(request)
        guard case .applied(let operation, let revision) = applied else { check(false, "the reviewed draft applies: \(applied)"); return }
        let resent = await engine.reconcile(request)
        check(resent == .applied(operationId: operation, revision: revision), "resending the same request returns the recorded operation: \(resent)")
        let entries = try await cliJSON(database, ["history"])["data"]["entries"].array ?? []
        let last = entries.last ?? .null
        check(entries.count == historyBefore + 1 && last["operation"]["actor"]["name"].string == "planner" && last["operation"]["command"]["ApplyChange"]["reason"].string == "Link A to C and widen B",
              "one operation was recorded, attributed to human:planner with its reason")

        // A cycle is refused with the application's reason, and the revision stays.
        let now = try await cliJSON(database, ["revision"])["revision"].uint ?? 0
        let current = PlanBasis(plan: try await cliJSON(database, ["export"])["data"], revision: now, lineage: preview.lineage)
        var cycle = PlanDraft(basis: current)
        cycle.add(.link(id: UUID().uuidString.lowercased(), from: f, fromEnd: .finish, to: a, toEnd: .start, lagHours: 0))
        let refused = await engine.propose(cycle.candidate(), basis: cycle.basis)
        if case .failure(let refusal) = refused {
            check(!refusal.message.isEmpty, "a cycle is refused with a reason: \(refusal.message)")
        } else {
            check(false, "a cycle is refused")
        }

        // Rewriting the contract of started work is refused; the revision stays.
        _ = try await cliJSON(database, ["claim", "TEST-A", "--actor", "agent:pilot"])
        _ = try await cliJSON(database, ["start", "TEST-A", "--actor", "agent:pilot"])
        let started = try await cliJSON(database, ["revision"])["revision"].uint ?? 0
        var protected = PlanDraft(basis: PlanBasis(plan: try await cliJSON(database, ["export"])["data"], revision: started, lineage: preview.lineage))
        protected.add(.objective(work: a, text: "A different objective for started work"))
        let guarded = await engine.propose(protected.candidate(), basis: protected.basis)
        if case .failure(let refusal) = guarded {
            check(!refusal.message.isEmpty, "rewriting started work is refused with a reason: \(refusal.message)")
        } else {
            check(false, "rewriting started work is refused")
        }
        check(try await cliJSON(database, ["revision"])["revision"].uint == started, "refusals left the revision unchanged")

        // A draft reviewed against an older revision is refused at apply and kept; read again, it reviews and applies.
        await MainActor.run {
            model.setEditing(true)
            model.edit(.estimate(work: c, optimistic: 30, likely: 45, pessimistic: 70))
        }
        _ = try await modelExpect(model, "the model to read the started work", 30) { $0.inventory.basis?.revision == started && $0.freshness.current }
        await MainActor.run { model.discardDraft(); model.edit(.estimate(work: c, optimistic: 30, likely: 45, pessimistic: 70)); model.reviewDraft() }
        try await waitEdit(model, "the draft to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        _ = try await cliJSON(database, ["progress", "TEST-A", "10", "--actor", "agent:pilot"])
        _ = try await modelExpect(model, "the model to read the newer plan", 30) { $0.inventory.basis?.revision == started &+ 1 && $0.freshness.current }
        await MainActor.run { model.setEditReason("Widen C"); model.setEditActor("human:planner"); model.applyDraft() }
        let blocker = await MainActor.run { model.editing.notice }
        check(blocker == nil, "the reviewed draft could be sent: \(blocker ?? "yes")")
        try await waitEdit(model, "the stale apply to be refused") { if case .refused(let refusal) = $0.phase { return refusal.stale } else { return false } }
        let kept = await MainActor.run { model.editing.draft?.edits.count }
        check(kept == 1, "the stale refusal kept the draft")
        await MainActor.run { model.rebaseDraft() }
        try await waitEdit(model, "the rebased draft to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        // While the apply is in flight nothing else may change the draft or apply: an edit made now is ignored.
        let heldEdit = await MainActor.run { () -> (Bool, Int?) in
            model.applyDraft()
            model.edit(.title(work: c, text: "Edited while applying"))
            return (model.editing.held, model.editing.draft?.edits.count)
        }
        check(heldEdit.0 && heldEdit.1 == 1, "an apply in flight holds the draft: an edit made meanwhile is ignored")
        try await waitEdit(model, "the rebased draft to apply") { if case .applied = $0.phase { return true } else { return false } }
        check(try await cliJSON(database, ["revision"])["revision"].uint == started &+ 2, "the reviewed draft applied after a re-read")
        await closeGantt(model)
    }

    /// An apply whose answer was lost holds the draft and every other apply until the same request is resent, and
    /// the resend records it exactly once.
    func editUnknownOutcomeIsReconciled() async throws {
        print("edit: an apply whose outcome is unknown holds the draft until the same request is reconciled, which records it once")
        let database = try await newStore("edit-unknown")
        let armed = LockedBox(false), sent = Event()
        // No background reads while the helper is stopped: the only exchange armed is the apply.
        let model = try await openGanttModel(.database(database)) { settings in
            settings.pollInterval = 3600
            let audited = settings.onBlockingStep
            settings.onBlockingStep = { step in
                audited?(step)
                if step == "exchange", armed.withLock({ $0 }) { sent.signal() }
            }
        }
        _ = try await modelExpect(model, "the plan to be read for editing", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let found = await MainActor.run { model.snapshot.inventory.items.first { $0.key == "TEST-C" }?.identity }
        guard let c = found else { check(false, "the edit fixture is readable"); return }
        let historyBefore = try await historyCount(database)
        await MainActor.run {
            model.setEditing(true)
            model.edit(.estimate(work: c, optimistic: 30, likely: 45, pessimistic: 70))
            model.reviewDraft()
        }
        try await waitEdit(model, "the draft to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        guard let pid = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "the snapshot names no helper process") }
        check(await freeze(pid), "the helper is stopped (its stop was awaited as an event)")
        armed.withLock { $0 = true }
        await MainActor.run { model.setEditReason("Widen C"); model.setEditActor("human:planner"); model.applyDraft() }
        check(await sent.wait(), "the apply is on the wire, unanswered")
        kill(pid, SIGKILL)
        try await waitEdit(model, "the lost answer to leave the outcome unknown") { $0.unresolved != nil }
        let held = await MainActor.run { () -> (String?, Int?) in
            model.edit(.title(work: c, text: "Edited while unknown"))
            model.discardDraft()
            return (model.applyBlocker, model.editing.draft?.edits.count)
        }
        check(held.0 == "Reconcile the last apply first." && held.1 == 1, "while unknown, no other apply is allowed and the draft is neither edited nor discarded: \(held)")
        await MainActor.run { model.reload() }
        _ = try await modelExpect(model, "the lost helper to be noticed and replaced", 60) { $0.connection == .connected && $0.counters.reconnects >= 1 }
        await MainActor.run { model.reconcileApply() }
        try await waitEdit(model, "the reconciled apply to settle") { if case .applied = $0.phase { return true } else { return false } }
        check(try await historyCount(database) == historyBefore + 1, "the reconciled request is recorded exactly once")
        await closeGantt(model)
    }

    /// A progress report from the app is the owner's execution report: refused for anyone else with the plan unchanged,
    /// recorded for the owner, and never a plan edit or an acceptance.
    func editProgressIsTheOwnersReport() async throws {
        print("edit: a progress report is sent as the acting actor, refused unless it owns the started work, recorded for the owner")
        let database = try await newStore("edit-progress")
        _ = try await cliJSON(database, ["claim", "TEST-A", "--actor", "agent:pilot"])
        _ = try await cliJSON(database, ["start", "TEST-A", "--actor", "agent:pilot"])
        let model = try await openGanttModel(.database(database))
        _ = try await modelExpect(model, "the started work to be read", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let found = await MainActor.run { model.snapshot.inventory.items.first { $0.key == "TEST-A" }?.identity }
        guard let a = found else { check(false, "the fixture is readable"); return }
        let before = try await historyCount(database)
        await MainActor.run { model.setEditing(true); model.setEditActor("human:planner"); model.reportProgress(work: a, percent: 40) }
        try await waitEdit(model, "the non-owner's report to be refused") { $0.notice?.hasPrefix("Progress for TEST-A was refused") == true }
        check(try await historyCount(database) == before, "a report by someone other than the owner recorded nothing")
        await MainActor.run { model.setEditActor("agent:pilot"); model.reportProgress(work: a, percent: 40) }
        try await waitEdit(model, "the owner's report to be recorded") { $0.notice?.hasPrefix("Reported 40% for TEST-A") == true }
        let shown = try await cliJSON(database, ["show", "TEST-A"])["data"]["execution"]
        check(shown["reported_progress_percent"] == .integer(40) && shown["status"].string == "InProgress",
              "the owner's report is recorded as 40% and the work stays in progress: \(shown["reported_progress_percent"]) \(shown["status"])")
        check(try await historyCount(database) == before + 1, "the report is one operation")
        await closeGantt(model)
    }

    /// Wait on the main actor for a published edit state that satisfies `predicate`, or fail; driven by the model's
    /// own publications, never by polling.
    @MainActor
    private func waitEdit(_ model: ObserverModel, _ what: String, _ seconds: TimeInterval = 30, _ predicate: @escaping (EditState) -> Bool) async throws {
        if predicate(model.editing) { return }
        let found: Bool = await withCheckedContinuation { continuation in
            var cancellable: AnyCancellable?
            var done = false
            cancellable = model.$editing.sink { state in
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
        if !found { throw SuiteError(description: "timed out after \(Int(seconds)) s waiting for \(what); edit phase \(model.editing.phase)") }
    }
}
