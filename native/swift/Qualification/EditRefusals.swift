// What the application refuses when the native app edits the plan: an agent may not apply, an apply needs a reason,
// an estimate out of order and a link to a work package are refused at review with the draft kept, and a request
// from another lineage is refused as stale by the application (the model's handling of a stale refusal, which keeps
// the draft and requires a re-read, is exercised with a revision conflict in edit-review-apply). Closing the review
// signals the page to take the keyboard back; the views act on that signal.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func editRefusalsKeepTheDraft() async throws {
        print("edit: agent applies, missing reasons, out-of-order estimates and package links are refused with the draft kept; another lineage is refused as stale")
        let database = try await newStore("edit-refusals", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database))
        _ = try await modelExpect(model, "the plan to be read for editing", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let ids = await MainActor.run { Dictionary(model.snapshot.inventory.items.map { ($0.key, $0.identity) }, uniquingKeysWith: { a, _ in a }) }
        guard let a = ids["TEST-A"], let c = ids["TEST-C"], let package = ids["TEST-PKG"] else { check(false, "the Gantt fixture is readable"); return }
        let history = try await historyCount(database)
        let revision = try await cliJSON(database, ["revision"])["revision"].uint

        // An agent may draft and review, never apply: the application refuses it and the draft stays.
        await MainActor.run {
            model.setEditing(true)
            model.setEditActor("agent:drafter")
            model.edit(.title(work: c, text: "Retitled by an agent"))
            model.reviewDraft()
        }
        try await waitEdit(model, "the agent's draft to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        let blocked = await MainActor.run { () -> String? in model.setEditReason(""); model.applyDraft(); return model.editing.notice }
        check(blocked?.contains("reason") == true, "an apply without a reason is not sent: \(blocked ?? "sent")")
        await MainActor.run { model.setEditReason("Agents may not approve scope"); model.applyDraft() }
        try await waitEdit(model, "the agent's apply to be refused") { if case .refused = $0.phase { return true } else { return false } }
        let agentRefusal = await MainActor.run { () -> (String, Int?) in
            if case .refused(let refusal) = model.editing.phase { return (refusal.message, model.editing.draft?.edits.count) }
            return ("", nil)
        }
        check(agentRefusal.0.contains("not permitted") && agentRefusal.1 == 1, "the application refused the agent's apply with its reason and the draft stayed: \(agentRefusal)")

        // An estimate out of order and a link from a work package are the application's refusals, at review.
        for (edit, words, what) in [
            (PlanEdit.estimate(work: c, optimistic: 9, likely: 5, pessimistic: 2), "optimistic <= likely <= pessimistic", "an estimate out of order"),
            (PlanEdit.link(id: UUID().uuidString.lowercased(), from: package, fromEnd: .finish, to: a, toEnd: .start, lagHours: 0), "work package", "a link from a work package"),
        ] {
            await MainActor.run { model.discardDraft(); model.edit(edit); model.reviewDraft() }
            try await waitEdit(model, "\(what) to be refused") { if case .refused = $0.phase { return true } else { return false } }
            let refused = await MainActor.run { () -> (String, Int?) in
                if case .refused(let refusal) = model.editing.phase { return (refusal.message, model.editing.draft?.edits.count) }
                return ("", nil)
            }
            check(refused.0.contains(words) && refused.1 == 1, "\(what) is refused with the application's reason and the draft stays: \(refused.0)")
        }

        // The application refuses a request carrying another lineage as stale.
        await MainActor.run { model.discardDraft(); model.setEditActor("human:planner"); model.edit(.title(work: c, text: "Contract C, renamed")); model.reviewDraft() }
        try await waitEdit(model, "the draft to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        let (engine, preview) = await MainActor.run { (model.engine, model.editing.reviewed) }
        if let engine, let preview, let actor = EditCommands.actor("human:planner") {
            var request = EditCommands.apply(preview, reason: "From another lineage", actor: actor)
            request.baseLineage = UUIDv7.make()
            let outcome = await engine.apply(request)
            if case .refused(let refusal) = outcome {
                check(refusal.stale && refusal.fromApplication, "another lineage is refused as stale by the application: \(refusal.code)")
            } else {
                check(false, "another lineage is refused: \(outcome)")
            }
        } else {
            check(false, "the reviewed draft is available")
        }
        let historyAfter = try await historyCount(database)
        let revisionAfter = try await cliJSON(database, ["revision"])["revision"].uint
        check(historyAfter == history && revisionAfter == revision, "every refusal left the history and the revision as they were")

        // Closing the review, after an apply or a cancel, signals the page to take the keyboard back.
        let returned = await MainActor.run { () -> Bool in
            let before = model.editing.focusReturns
            model.closeReview()
            // The sheet's dismissal, once it is gone, is what hands the keyboard back.
            model.reviewDismissed()
            return model.editing.focusReturns == before &+ 1 && !model.editing.reviewing
        }
        check(returned, "closing the review signals the page it was opened from to take the keyboard back")
        await closeGantt(model)
    }
}
