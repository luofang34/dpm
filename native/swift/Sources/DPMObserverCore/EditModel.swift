// The editing session a person drives from the Gantt, the network or the edit inspector: a draft of edits
// against the export it was made from, the application's review of it, and one explicit apply.
//
// The order is fixed: edits gather in a draft; `review` asks the application to validate and diff the
// candidate (a read that records nothing); only a reviewed draft can be applied, with a reason and an actor;
// an apply whose outcome is unknown must be reconciled, by resending the same request, before anything else
// applies; a refusal for a stale basis keeps the draft and asks for a re-read and a new review.

import DPMNative
import Foundation

/// An apply that was sent: enough to resend exactly the same request.
public struct SentApply: Equatable, Sendable {
    /// The draft whose review was sent; an answer clears this draft and no later one.
    public let draft: PlanDraft
    public let preview: ProposalPreview
    public let reason: String
    public let actor: JSON
    public let operationId: String

    var request: CommandRequest { EditCommands.apply(preview, reason: reason, actor: actor, operationId: operationId) }
}

public struct EditState: Equatable, Sendable {
    public enum Phase: Equatable, Sendable {
        case drafting
        case validating
        case reviewed(ProposalPreview)
        case refused(EditRefusal)
        case applying
        case applied(operationId: String, revision: UInt64?)
        /// Sent, outcome unknown: reconcile before anything else applies.
        case unknown(SentApply, cause: String)
    }

    /// Whether the views offer editing gestures; editing never starts by itself.
    public var enabled = false
    /// Who applies, as `kind:name`; attribution, not authentication.
    public var actor = "human:" + NSUserName()
    public var reason = ""
    public var draft: PlanDraft?
    public var phase = Phase.drafting
    /// Whether the review sheet is shown.
    public var reviewing = false
    /// Why the last request to apply could not be sent, when it could not.
    public var notice: String?

    public var reviewed: ProposalPreview? { if case .reviewed(let preview) = phase { return preview } else { return nil } }
    public var unresolved: SentApply? { if case .unknown(let sent, _) = phase { return sent } else { return nil } }
    /// An apply is in flight or its outcome is unknown: nothing else may change the draft or apply.
    public var held: Bool { unresolved != nil || phase == .applying }
    /// A stale refusal asked for a re-read: the draft is rebased and reviewed once a newer plan is read.
    public var rebasePending = false
}

extension ObserverModel {
    /// The words the views use for a work item or decision identity.
    public func editName(_ identity: String) -> String {
        snapshot.inventory.key(of: identity) ?? snapshot.inventory.decisionByIdentity[identity]?.key ?? String(identity.prefix(8))
    }

    /// Why the workspace cannot be edited now, or nil when it can.
    public var editUnavailable: String? {
        guard snapshot.identity?.source == "live" else { return "Only a live workspace can be edited; this one is read-only." }
        guard case .connected = snapshot.connection else { return "The workspace is not connected." }
        guard snapshot.freshness.current else { return "The view is refreshing; edits apply only to the current plan." }
        return nil
    }

    public func setEditing(_ on: Bool) {
        editing.enabled = on
    }

    public func setEditActor(_ text: String) { editing.actor = text }

    /// Say why a gesture proposed nothing, or clear it.
    public func setEditNotice(_ text: String?) { if editing.notice != text { editing.notice = text } }

    /// The link being drawn on the network, or none.
    public func setNetworkLink(_ link: NetworkLinkDrag?) {
        if network.linking != link { network.linking = link }
    }
    public func setEditReason(_ text: String) { editing.reason = text }

    /// Close the review sheet; the draft and its review stay.
    public func closeReview() { editing.reviewing = false }

    /// The parent of a work item as the draft's basis records it.
    public func editParent(of identity: String) -> String? {
        editing.draft?.basis.plan["work_items"][identity]["parent"].string ?? snapshot.inventory.basis?.plan["work_items"][identity]["parent"].string
    }

    /// Add one edit to the draft, starting a draft against the export now shown if there is none.
    public func edit(_ change: PlanEdit) {
        guard !editing.held else {
            editing.notice = "The last apply is still being sent or must be reconciled; this edit was not added."
            return
        }
        if editing.draft == nil {
            guard let basis = snapshot.inventory.basis else { return }
            editing.draft = PlanDraft(basis: basis)
            editing.rebasePending = false
        }
        editing.draft?.add(change)
        editing.phase = .drafting
        editing.notice = nil
    }

    public func removeEdit(at index: Int) {
        guard !editing.held else { return }
        editing.draft?.remove(at: index)
        if editing.draft?.isEmpty == true {
            editing.draft = nil
            editing.rebasePending = false
        }
        editing.phase = .drafting
    }

    public func discardDraft() {
        guard !editing.held else { return }
        editing.draft = nil
        editing.phase = .drafting
        editing.reviewing = false
        editing.rebasePending = false
    }

    /// Ask the application to validate and diff the draft against the basis it was made from; nothing is recorded.
    public func reviewDraft() {
        guard let draft = editing.draft, !draft.isEmpty, !editing.held, let engine else { return }
        editing.phase = .validating
        editing.reviewing = true
        Task { [weak self] in
            let result = await engine.propose(draft.candidate(), basis: draft.basis)
            await MainActor.run {
                guard let self, self.editing.draft == draft else { return }
                switch result {
                case .success(let preview): self.editing.phase = .reviewed(preview)
                case .failure(let refusal): self.editing.phase = .refused(refusal)
                }
            }
        }
    }

    /// Keep the draft's edits, read the plan again, and review them against it: at once if a newer plan is
    /// already shown, otherwise when the re-read arrives.
    public func rebaseDraft() {
        guard let draft = editing.draft, !editing.held else { return }
        if let basis = snapshot.inventory.basis, basis.revision != draft.basis.revision || basis.lineage != draft.basis.lineage {
            editing.rebasePending = false
            editing.draft = draft.rebased(on: basis)
            reviewDraft()
        } else {
            editing.rebasePending = true
            editing.phase = .validating
            reload()
        }
    }

    /// Called with every installed snapshot: finish a rebase that was waiting for a newer plan.
    func continueRebase() {
        guard editing.rebasePending, let draft = editing.draft, let basis = snapshot.inventory.basis,
              basis.revision != draft.basis.revision || basis.lineage != draft.basis.lineage else { return }
        rebaseDraft()
    }

    /// Apply the reviewed draft, once, with the reason and actor given.
    public func applyDraft() {
        editing.notice = applyBlocker
        guard editing.notice == nil, let preview = editing.reviewed, let engine, let actor = EditCommands.actor(editing.actor) else { return }
        guard let draft = editing.draft else { return }
        let sent = SentApply(draft: draft, preview: preview, reason: editing.reason, actor: actor, operationId: UUIDv7.make())
        editing.phase = .applying
        Task { [weak self] in
            let outcome = await engine.apply(sent.request)
            await MainActor.run { self?.settle(outcome, sent: sent) }
        }
    }

    /// Why the reviewed draft cannot be applied now, or nil when it can.
    public var applyBlocker: String? {
        if let unavailable = editUnavailable { return unavailable }
        if editing.unresolved != nil { return "Reconcile the last apply first." }
        if editing.reviewed == nil { return "Review the draft first." }
        if EditCommands.actor(editing.actor) == nil { return "Give the actor as kind:name, such as human:ada." }
        if editing.reason.trimmingCharacters(in: .whitespaces).isEmpty { return "Give a reason; it is recorded with the change." }
        return nil
    }

    /// Report how far an owned, started task has come, as the acting actor. This is an execution report sent at once
    /// through the application's command, never a plan edit: the application refuses it unless the actor owns the
    /// started task, and a report of 100% is not acceptance.
    public func reportProgress(work: String, percent: Int) {
        if let unavailable = editUnavailable { editing.notice = unavailable; return }
        guard let engine, let actor = EditCommands.actor(editing.actor), let basis = snapshot.inventory.basis else {
            editing.notice = "Give the actor as kind:name, such as human:ada."
            return
        }
        let clamped = min(100, max(0, percent))
        let key = editName(work)
        let request = CommandRequest(actor: actor, baseRevision: basis.revision, baseLineage: basis.lineage, operationId: UUIDv7.make(),
                                     command: .object(["ReportProgress": .object(["work": .string(work), "percent": .integer(Int64(clamped)), "note": .null])]))
        editing.notice = "Reporting \(clamped)% for \(key)…"
        Task { [weak self] in
            let outcome = await engine.apply(request)
            await MainActor.run {
                guard let self else { return }
                switch outcome {
                case .applied(_, let revision):
                    self.editing.notice = "Reported \(clamped)% for \(key)\(revision.map { " at revision \($0)" } ?? ""); a report is not acceptance."
                    self.reload()
                case .refused(let refusal):
                    self.editing.notice = "Progress for \(key) was refused: \(refusal.message)"
                case .unknown(let id, let cause):
                    self.editing.notice = "Whether the \(clamped)% report for \(key) was recorded is unknown (\(cause)); request \(id). Reload to see it."
                    self.reload()
                }
            }
        }
    }

    /// Resend the apply whose outcome is unknown, to learn what became of it.
    public func reconcileApply() {
        guard let sent = editing.unresolved, let engine else { return }
        editing.phase = .applying
        Task { [weak self] in
            var outcome = await engine.reconcile(sent.request)
            // Only the application's own answer settles it: a resend that never reached the application leaves
            // the outcome as unknown as before.
            if case .refused(let refusal) = outcome, !refusal.fromApplication {
                outcome = .unknown(operationId: sent.operationId, cause: refusal.message)
            }
            await MainActor.run { self?.settle(outcome, sent: sent) }
        }
    }

    func settle(_ outcome: ApplyOutcome, sent: SentApply) {
        switch outcome {
        case .applied(let id, let revision):
            editing.phase = .applied(operationId: id, revision: revision)
            if editing.draft == sent.draft {
                editing.draft = nil
                editing.rebasePending = false
            }
            editing.reason = ""
            reload()
        case .refused(let refusal):
            editing.phase = .refused(refusal)
        case .unknown(_, let cause):
            editing.phase = .unknown(sent, cause: cause)
        }
    }
}
