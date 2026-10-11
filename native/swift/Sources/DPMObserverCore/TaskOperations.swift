// Task operations from the native app: claim, start, progress, block, unblock and submit by the work's owner, and
// verify or reject as a separate, independent review. Each is one application command sent at once with the acting
// actor, a request identity and the observed revision and lineage; the application decides whether it is allowed.
//
// An outcome is shown as accepted (with its revision), refused (with the application's reason) or unknown. An unknown
// outcome holds every further command, and every plan apply, until the same request is resent to learn what became of
// it. A refusal keeps what the person typed and asks for the work to be read again and its changes looked at before
// anything is sent again, so nothing is retried blindly.

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

/// An operation that was sent: enough to resend exactly the same request, and the work as it was read then.
public struct SentOperation: Equatable, Sendable {
    public let work: String
    public let operation: TaskOperation
    public let request: CommandRequest
    public let observed: ObservedWork

    public static func == (left: SentOperation, right: SentOperation) -> Bool {
        left.work == right.work && left.operation == right.operation && left.request.operationId == right.request.operationId
    }
}

/// The work as it was read when an operation was sent, to compare with the plan read again after a refusal.
public struct ObservedWork: Equatable, Sendable {
    public let revision: UInt64
    public let lineage: String?
    public let status: String
    public let owner: String?
    public let progress: Int
    /// How many status reads the model had made; a later one is a read of the work since.
    let reads: Int
}

public struct OperationState: Equatable, @unchecked Sendable {
    public enum Phase: Equatable, Sendable {
        case idle
        case sending(SentOperation)
        case accepted(work: String, operation: TaskOperation, revision: UInt64?)
        case refused(work: String, operation: TaskOperation, refusal: EditRefusal, observed: ObservedWork)
        /// Sent, outcome unknown: reconcile before any further command.
        case unknown(SentOperation, cause: String)
    }

    public var phase = Phase.idle
    /// What the person typed for a block or reject reason and a submit or verify note; a refusal keeps it.
    public var reason = ""
    public var note = ""
    /// After a refusal, whether the work was read again and its changes looked at.
    public var reviewedChanges = false
    /// A progress share set by dragging a bar, shown in the inspector until it is reported or dropped; a drag never
    /// sends anything by itself.
    public var stagedProgress: (work: String, percent: Int)?
    /// The revision an accepted operation produced; nothing more is sent until the plan is read at least that far.
    public var awaitedRevision: UInt64?

    public var unresolved: SentOperation? { if case .unknown(let sent, _) = phase { return sent } else { return nil } }
    public var inFlight: Bool { if case .sending = phase { return true } else { return unresolved != nil } }
    /// The work as it was when an operation was refused, until the person has looked at its changes.
    public var staleObservation: ObservedWork? {
        if case .refused(_, _, _, let observed) = phase, !reviewedChanges { return observed }
        return nil
    }

    public static func == (left: OperationState, right: OperationState) -> Bool {
        left.phase == right.phase && left.reason == right.reason && left.note == right.note && left.reviewedChanges == right.reviewedChanges
            && left.awaitedRevision == right.awaitedRevision && left.stagedProgress?.work == right.stagedProgress?.work
            && left.stagedProgress?.percent == right.stagedProgress?.percent
    }
}

extension ObserverModel {
    /// Why an operation cannot be sent now, or nil when it can.
    public var operationBlocker: String? {
        if let unavailable = editUnavailable { return unavailable }
        if operations.unresolved != nil || editing.unresolved != nil { return "Reconcile the last request first: its outcome is unknown." }
        if case .sending = operations.phase { return "The last operation is still being sent." }
        if editing.phase == .applying { return "A plan change is being applied." }
        if operations.staleObservation != nil { return "The last operation was refused; read the work again and look at its changes first." }
        if let awaited = operations.awaitedRevision, (snapshot.inventory.basis?.revision ?? 0) < awaited { return "Reading the plan again…" }
        if EditCommands.actor(editing.actor) == nil { return "Give the actor as kind:name, such as human:ada." }
        return nil
    }

    public func setOperationReason(_ text: String) { operations.reason = text }
    public func setOperationNote(_ text: String) { operations.note = text }

    /// Whether the work was read again since it was refused: a later status read, or another revision or lineage.
    public func rereadSince(_ observed: ObservedWork) -> Bool {
        guard snapshot.freshness.current, let basis = snapshot.inventory.basis else { return false }
        return statusReads > observed.reads || basis.revision != observed.revision || basis.lineage != observed.lineage
    }

    var statusReads: Int { snapshot.counters.projectReads &+ snapshot.counters.timeReads }

    /// The person has read the work again and looked at what changed; the next operation may be sent.
    public func acknowledgeChanges() {
        guard let observed = operations.staleObservation, rereadSince(observed) else { return }
        operations.reviewedChanges = true
    }

    /// Hold a progress share set by dragging a bar until the person reports it from the inspector.
    public func stageProgress(work: String, percent: Int) {
        operations.stagedProgress = (work, min(100, max(0, percent)))
    }

    public func dropStagedProgress() { operations.stagedProgress = nil }

    /// Forget every sent request and draft of another workspace, so nothing of it is reconciled or applied here.
    func resetWrites() {
        let keep = (editing.enabled, editing.actor)
        editing = EditState()
        editing.enabled = keep.0
        editing.actor = keep.1
        operations = OperationState()
    }

    /// Send one operation on `work` as the acting actor, against the plan now shown.
    public func operate(_ operation: TaskOperation, on work: String) {
        if let blocker = operationBlocker { editing.notice = blocker; return }
        guard let engine, let actor = EditCommands.actor(editing.actor), let basis = snapshot.inventory.basis else { return }
        let request = CommandRequest(actor: actor, baseRevision: basis.revision, baseLineage: basis.lineage, operationId: UUIDv7.make(),
                                     command: operation.command(work: work))
        let item = snapshot.inventory.byIdentity[work]
        let observed = ObservedWork(revision: basis.revision, lineage: basis.lineage, status: item?.status ?? "unknown", owner: item?.owner,
                                    progress: item?.reportedProgress ?? 0, reads: statusReads)
        let sent = SentOperation(work: work, operation: operation, request: request, observed: observed)
        operations.phase = .sending(sent)
        operations.reviewedChanges = false
        if case .progress = operation { operations.stagedProgress = nil }
        Task { [weak self] in
            let outcome = await engine.apply(request)
            await MainActor.run { self?.settleOperation(outcome, sent: sent) }
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
            await MainActor.run { self?.settleOperation(outcome, sent: sent) }
        }
    }

    func settleOperation(_ outcome: ApplyOutcome, sent: SentOperation) {
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
            operations.phase = .refused(work: sent.work, operation: sent.operation, refusal: refusal, observed: sent.observed)
            reload()
        case .unknown(_, let cause):
            operations.phase = .unknown(sent, cause: cause)
            reload()
        }
    }
}
