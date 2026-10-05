// The engine's two editing calls, kept apart from its reads: a read-only proposal that the application
// validates and diffs against the basis it observed, and the one mutation, an explicit apply of the
// changes that proposal returned. Nothing here judges a change; refusals are the application's.

import DPMNative
import Foundation

/// The application's answer to a candidate: the revision and lineage it was computed against, the changes
/// an apply would record, the work to reassess, and work entering or leaving the active graph.
public struct ProposalPreview: Equatable, Sendable {
    public let revision: UInt64
    public let lineage: String?
    /// The entity changes exactly as `ApplyChange` takes them.
    public let changes: JSON
    public let affectedWork: [String]
    public let applicability: [String]

    public var isEmpty: Bool { changes.items.isEmpty }
    public var changeList: [JSON] { changes.items }
}

/// A proposal the application refused, with its reason as written.
public struct EditRefusal: Error, Equatable, Sendable {
    public let code: String
    public let message: String
    /// The application refused it; otherwise the request may never have reached the application.
    public var fromApplication = true
    /// The draft was made against an older revision or another lineage: re-read and review again.
    public var stale: Bool { code == "revision_conflict" || code == "lineage_mismatch" }
}

/// What became of an apply.
public enum ApplyOutcome: Equatable, Sendable {
    /// Recorded as this operation, giving this revision.
    case applied(operationId: String, revision: UInt64?)
    /// Refused, with the plan unchanged.
    case refused(EditRefusal)
    /// Sent, but whether it committed is unknown: the same request must be reconciled before anything else.
    case unknown(operationId: String, cause: String)
}

extension ObserverEngine {
    /// Validate a candidate plan without changing anything: the application diffs it against its current
    /// state, which must still be the revision and lineage the draft was made from.
    public func propose(_ candidate: JSON, basis: PlanBasis) async -> Result<ProposalPreview, EditRefusal> {
        guard let connection else { return .failure(EditRefusal(code: "not_connected", message: "The workspace is not open.")) }
        do {
            let view = try await connection.query(.object(["query": .string("propose_change"), "plan": candidate]))
            let revision = view.envelope.revision ?? 0
            let lineage = view.envelope.lineageId
            guard revision == basis.revision, lineage == basis.lineage else {
                return .failure(EditRefusal(code: "revision_conflict",
                                            message: "The plan changed since the draft was made (revision \(basis.revision) → \(revision)); review the draft against the current plan."))
            }
            let data = view.envelope.data
            return .success(ProposalPreview(
                revision: revision, lineage: lineage, changes: data["changes"],
                affectedWork: data["affected_work"].items.compactMap { $0["key"].string ?? $0["work"].string },
                applicability: data["applicability_changes"].items.compactMap { $0["key"].string ?? $0["work"].string }))
        } catch {
            return .failure(Self.refusal(error))
        }
    }

    /// Apply the changes a proposal returned, once, as `actor`, with the request's identity and the basis it
    /// was reviewed against. An outcome that cannot be known is reported, never retried here.
    public func apply(_ request: CommandRequest) async -> ApplyOutcome {
        guard let connection else { return .refused(EditRefusal(code: "not_connected", message: "The workspace is not open.", fromApplication: false)) }
        do {
            let committed = try await connection.execute(request)
            let envelope = committed.envelope
            return .applied(operationId: envelope.data["id"].string ?? request.operationId, revision: envelope.revision)
        } catch BridgeError.commandOutcomeUnknown(let id, let cause) {
            return .unknown(operationId: id, cause: cause)
        } catch BridgeError.commandCommitted(let identity) {
            return .applied(operationId: identity.operationId, revision: identity.resultingRevision)
        } catch {
            return .refused(Self.refusal(error))
        }
    }

    /// Resend the same request to learn its outcome: the application returns the operation it recorded under
    /// that identity, or applies it now if it never arrived.
    public func reconcile(_ request: CommandRequest) async -> ApplyOutcome { await apply(request) }

    static func refusal(_ error: Error) -> EditRefusal {
        if case BridgeError.refused(let native) = error {
            // Refusals of the frame itself, or while the workspace is being swapped, never reached the application.
            let unprocessed: [ErrorCode] = [.frameTooLarge, .invalidEncoding, .truncatedFrame, .invalidId, .workspaceChanging]
            return EditRefusal(code: native.code.description, message: native.message, fromApplication: !unprocessed.contains(native.code))
        }
        return EditRefusal(code: "transport", message: describe(error), fromApplication: false)
    }
}

/// The application command an apply sends.
public enum EditCommands {
    public static func apply(_ preview: ProposalPreview, reason: String, actor: JSON, operationId: String = UUIDv7.make()) -> CommandRequest {
        CommandRequest(actor: actor, baseRevision: preview.revision, baseLineage: preview.lineage, operationId: operationId,
                       command: .object(["ApplyChange": .object(["changes": preview.changes, "reason": .string(reason)])]))
    }

    /// A local actor name, `kind:name`, as attribution: it is not an authenticated identity.
    public static func actor(_ text: String) -> JSON? {
        let parts = text.split(separator: ":", maxSplits: 1).map(String.init)
        guard parts.count == 2, !parts[1].isEmpty, let kind = ["human": "Human", "agent": "Agent", "service": "Service"][parts[0].lowercased()] else { return nil }
        return .object(["kind": .string(kind), "name": .string(parts[1])])
    }
}

/// One entity change of the application's diff in words: what is added, removed or changed, and which fields.
public enum ChangeWords {
    public static func describe(_ change: JSON, names: (String) -> String) -> String {
        let collection = change["collection"].string ?? "?"
        let id = change["id"].string ?? "?"
        let before = change["before"], after = change["after"]
        let verb = before == .null ? "add" : (after == .null ? "remove" : "change")
        let entity = after == .null ? before : after
        let label: String
        switch collection {
        case "dependencies":
            label = "\(GanttRelation.abbreviation(entity["kind"].string ?? "")) \(names(entity["predecessor"].string ?? "?")) → \(names(entity["successor"].string ?? "?"))"
        case "work_items":
            label = "\(entity["key"].string ?? names(id)) \(entity["title"].string ?? "")"
        case "decisions":
            label = entity["key"].string ?? names(id)
        default:
            label = id
        }
        let fields = change["fields"].items.compactMap(\.string)
        return "\(verb) \(collection.replacingOccurrences(of: "_", with: " ")): \(label)\(fields.isEmpty ? "" : " (" + fields.joined(separator: ", ") + ")")"
    }
}
