// Task operations from the native app: claim, start, progress, block, unblock and submit by the work's owner, and
// verify or reject as a separate, independent review. Each is one application command sent at once with the acting
// actor, a request identity and the observed revision and lineage; the application decides whether it is allowed.
//
// An outcome is shown as accepted (with its revision), refused (with the application's reason) or unknown. An unknown
// outcome holds every further command, and every plan apply, until the same request is resent to learn what became of
// it. A refusal keeps what the person typed; a refusal for a stale basis asks for the work to be read again and its
// changes looked at before anything is sent again.

import DPMNative
import Foundation

/// One operation on a work item, as the person asked for it.
public enum TaskOperation: Equatable, Sendable {
    case claim
    case start
    case progress(percent: Int)
    case block(reason: String)
    case unblock
    case submit(note: String)
    case verify(note: String)
    case reject(reason: String)

    /// The application command, in its own form.
    func command(work: String) -> JSON {
        let id = JSON.string(work)
        func text(_ value: String) -> JSON { value.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? .null : .string(value) }
        switch self {
        case .claim: return .object(["Claim": .object(["work": id])])
        case .start: return .object(["Start": .object(["work": id, "occurred_at": .null])])
        case .progress(let percent): return .object(["ReportProgress": .object(["work": id, "percent": .integer(Int64(min(100, max(0, percent)))), "note": .null])])
        case .block(let reason): return .object(["Block": .object(["work": id, "reason": .string(reason)])])
        case .unblock: return .object(["Unblock": .object(["work": id])])
        case .submit(let note): return .object(["Submit": .object(["work": id, "note": text(note), "occurred_at": .null])])
        case .verify(let note): return .object(["Verify": .object(["work": id, "note": text(note), "occurred_at": .null])])
        case .reject(let reason): return .object(["Reject": .object(["work": id, "reason": .string(reason)])])
        }
    }

    /// The operation in words.
    public var words: String {
        switch self {
        case .claim: return "claim"
        case .start: return "start"
        case .progress(let percent): return "report \(percent)%"
        case .block: return "block"
        case .unblock: return "unblock"
        case .submit: return "submit"
        case .verify: return "verify"
        case .reject: return "reject"
        }
    }
}

/// An operation that was sent: enough to resend exactly the same request.
public struct SentOperation: Equatable, Sendable {
    public let work: String
    public let operation: TaskOperation
    public let request: CommandRequest

    public static func == (left: SentOperation, right: SentOperation) -> Bool {
        left.work == right.work && left.operation == right.operation && left.request.operationId == right.request.operationId
    }
}

/// The work as it was when an operation was refused for a stale basis, to compare with the plan read again.
public struct ObservedWork: Equatable, Sendable {
    public let revision: UInt64
    public let status: String
    public let owner: String?
    public let progress: Int
}

public struct OperationState: Equatable, Sendable {
    public enum Phase: Equatable, Sendable {
        case idle
        case sending(SentOperation)
        case accepted(work: String, operation: TaskOperation, revision: UInt64?)
        case refused(work: String, operation: TaskOperation, refusal: EditRefusal, observed: ObservedWork?)
        /// Sent, outcome unknown: reconcile before any further command.
        case unknown(SentOperation, cause: String)
    }

    public var phase = Phase.idle
    /// What the person typed for a block or reject reason and a submit or verify note; a refusal keeps it.
    public var reason = ""
    public var note = ""
    /// After a stale refusal, whether the work was read again and its changes looked at.
    public var reviewedChanges = false
    /// The revision an accepted operation produced; nothing more is sent until the plan is read at least that far.
    public var awaitedRevision: UInt64?

    public var unresolved: SentOperation? { if case .unknown(let sent, _) = phase { return sent } else { return nil } }
    public var inFlight: Bool { if case .sending = phase { return true } else { return unresolved != nil } }
    /// The work as it was refused for a stale basis, until the person has looked at its changes.
    public var staleObservation: ObservedWork? {
        if case .refused(_, _, let refusal, let observed) = phase, refusal.stale, !reviewedChanges { return observed }
        return nil
    }
}

extension ObserverModel {
    /// Why an operation cannot be sent now, or nil when it can.
    public var operationBlocker: String? {
        if let unavailable = editUnavailable { return unavailable }
        if operations.unresolved != nil || editing.unresolved != nil { return "Reconcile the last request first: its outcome is unknown." }
        if case .sending = operations.phase { return "The last operation is still being sent." }
        if editing.phase == .applying { return "A plan change is being applied." }
        if operations.staleObservation != nil { return "The work changed since it was read; look at its changes first." }
        if let awaited = operations.awaitedRevision, (snapshot.inventory.basis?.revision ?? 0) < awaited { return "Reading the plan again…" }
        if EditCommands.actor(editing.actor) == nil { return "Give the actor as kind:name, such as human:ada." }
        return nil
    }

    public func setOperationReason(_ text: String) { operations.reason = text }
    public func setOperationNote(_ text: String) { operations.note = text }

    /// The person has read the work again and looked at what changed; the next operation may be sent.
    public func acknowledgeChanges() {
        guard let observed = operations.staleObservation, let basis = snapshot.inventory.basis, basis.revision != observed.revision else { return }
        operations.reviewedChanges = true
    }

    /// Send one operation on `work` as the acting actor, against the plan now shown.
    public func operate(_ operation: TaskOperation, on work: String) {
        if let blocker = operationBlocker { editing.notice = blocker; return }
        guard let engine, let actor = EditCommands.actor(editing.actor), let basis = snapshot.inventory.basis else { return }
        let request = CommandRequest(actor: actor, baseRevision: basis.revision, baseLineage: basis.lineage, operationId: UUIDv7.make(),
                                     command: operation.command(work: work))
        let sent = SentOperation(work: work, operation: operation, request: request)
        let observed = snapshot.inventory.byIdentity[work].map { ObservedWork(revision: basis.revision, status: $0.status, owner: $0.owner, progress: $0.reportedProgress) }
        operations.phase = .sending(sent)
        operations.reviewedChanges = false
        Task { [weak self] in
            let outcome = await engine.apply(request)
            await MainActor.run { self?.settleOperation(outcome, sent: sent, observed: observed) }
        }
    }

    /// Resend the operation whose outcome is unknown, to learn what became of it.
    public func reconcileOperation() {
        guard let sent = operations.unresolved, let engine else { return }
        operations.phase = .sending(sent)
        Task { [weak self] in
            var outcome = await engine.reconcile(sent.request)
            // Only the application's own answer settles it.
            if case .refused(let refusal) = outcome, !refusal.fromApplication {
                outcome = .unknown(operationId: sent.request.operationId, cause: refusal.message)
            }
            await MainActor.run { self?.settleOperation(outcome, sent: sent, observed: nil) }
        }
    }

    func settleOperation(_ outcome: ApplyOutcome, sent: SentOperation, observed: ObservedWork?) {
        switch outcome {
        case .applied(_, let revision):
            operations.phase = .accepted(work: sent.work, operation: sent.operation, revision: revision)
            operations.awaitedRevision = revision
            switch sent.operation {
            case .block, .reject: operations.reason = ""
            case .submit, .verify: operations.note = ""
            default: break
            }
            reload()
        case .refused(let refusal):
            operations.phase = .refused(work: sent.work, operation: sent.operation, refusal: refusal, observed: observed)
            if refusal.stale { reload() }
        case .unknown(_, let cause):
            operations.phase = .unknown(sent, cause: cause)
            reload()
        }
    }
}
