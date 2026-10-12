// Typed, immutable values for runs and their feeds: activity, lifecycle and the operation history, read
// out of the shared query payloads. As with the work records, every word is the payload's own, kept even
// when this build does not know it.

import DPMNative
import Foundation

// MARK: - Runs and their feeds

/// A word of a closed vocabulary, or the word this build does not know, kept as written.
public enum Word<Case: RawRepresentable & Hashable & Sendable>: Hashable, Sendable where Case.RawValue == String {
    case known(Case)
    case unknown(String)

    init(_ text: String?, _ unknown: String = "unknown") {
        if let text = text, let known = Case(rawValue: text) { self = .known(known) } else { self = .unknown(text ?? unknown) }
    }

    public var text: String {
        switch self {
        case .known(let value): return value.rawValue
        case .unknown(let text): return text
        }
    }
}

public enum ActivityKind: String, Sendable { case toolStarted = "tool_started", toolResult = "tool_result", progress, inputRequested = "input_requested", heartbeat }

public struct ActivityEntry: Identifiable, Hashable, Sendable {
    public var id: UInt64 { sequence }
    public let sequence: UInt64
    public let run: String
    public let sourceSequence: UInt64
    public let kind: Word<ActivityKind>
    public let text: String
    public let recordedAt: Instant?
    public let recordedBy: String

    init?(_ json: JSON) {
        guard let sequence = json["sequence"].uint, let run = json["run"].string else { return nil }
        self.sequence = sequence
        self.run = run
        sourceSequence = json["source_sequence"].uint ?? 0
        kind = Word(json["kind"].string)
        text = json["text"].string ?? ""
        recordedAt = json["recorded_at"].instant
        recordedBy = json["recorded_by"].actorName ?? "unknown"
    }
}

public struct LifecycleEntry: Identifiable, Hashable, Sendable {
    public var id: UInt64 { sequence }
    public let sequence: UInt64
    public let run: String
    public let state: String
    public let detail: String?
    public let recordedAt: Instant?
    public let recordedBy: String

    init?(_ json: JSON) {
        guard let sequence = json["sequence"].uint, let run = json["run"].string else { return nil }
        self.sequence = sequence
        self.run = run
        state = json["state"].string ?? "unknown"
        detail = json["detail"].string
        recordedAt = json["recorded_at"].instant
        recordedBy = json["recorded_by"].actorName ?? "unknown"
    }
}

public enum Observation: String, Sendable { case managed, reportedOnly = "reported_only" }

public struct RunSummary: Identifiable, Hashable, Sendable {
    public var id: String { identity }
    public let identity: String
    /// The persistent identity of the task the run executes; the key is shown, the identity followed.
    public let workIdentity: String
    public let workKey: String
    public let executor: String
    public let recordedBy: String
    public let observation: Word<Observation>
    /// The run's last recorded lifecycle word: the executor's own report.
    public let state: String
    public let stateDetail: String?
    /// What can be said now: `stale` and `unknown` mean the run is not known to be running or done.
    public let status: String
    public let stateSince: Instant?
    public let startedAt: Instant?
    public let lastReceiptAt: Instant?
    public let staleAt: Instant?
    public let recorded: Int
    public let retained: Int
    public let latestKind: String?
    public let latestAt: Instant?
    public let foreignLineage: Bool
    public let orphan: String?
    public let operations: [String]
    public let provider: String?
    public let evaluatedAt: Instant?

    init?(_ json: JSON) {
        guard let identity = json["run"]["id"].string else { return nil }
        self.identity = identity
        workIdentity = json["run"]["work"].string ?? ""
        workKey = json["run"]["contract"]["work_key"].string ?? "unknown work"
        executor = json["run"]["executor"].actorName ?? "unknown"
        recordedBy = json["run"]["recorded_by"].actorName ?? "unknown"
        observation = Word(json["run"]["observation"].string)
        state = json["state"].string ?? "unknown"
        stateDetail = json["state_detail"].string
        status = json["status"].string ?? "unknown"
        stateSince = json["state_since"].instant
        startedAt = json["run"]["started_at"].instant
        lastReceiptAt = json["last_receipt_at"].instant
        staleAt = json["stale_at"].instant
        recorded = json["activity"]["recorded"].int ?? 0
        retained = json["activity"]["retained"].int ?? 0
        latestKind = json["activity"]["latest"]["kind"].string
        latestAt = json["activity"]["latest"]["recorded_at"].instant
        foreignLineage = json["lineage"].string == "foreign"
        orphan = Self.orphan(json["orphan"])
        operations = json["operations"].items.compactMap { $0["operation"].string ?? $0["operation_id"].string ?? $0["id"].string }
        provider = [json["run"]["session"]["provider"].string, json["run"]["session"]["session"].string].compactMap { $0 }.joined(separator: " ").nilIfEmpty
        evaluatedAt = json["evaluated_at"].instant
    }

    /// The executor's report of ending, never a verdict on the work.
    public var finished: Bool { ["completed", "failed", "interrupted"].contains(state) }

    private static func orphan(_ json: JSON) -> String? {
        switch json["reason"].string {
        case "owner_changed": return "the task now belongs to \(json["owner"].actorName ?? "no one")"
        case "not_executing": return "the task is \(json["status"].string ?? "no longer executing")"
        case "work_missing": return "the task is not in the plan"
        case let other?: return other
        case nil: return nil
        }
    }
}

extension String {
    var nilIfEmpty: String? { isEmpty ? nil : self }
}

/// A project operation as the feed or history delivers it, with the task named when the shared
/// snapshot can name it.
public struct OperationEntry: Identifiable, Hashable, Sendable {
    public var id: String { operation }
    public let sequence: UInt64
    public let operation: String
    public let actor: String
    public let verb: String
    public let workIdentity: String?
    public let note: String?
    public let timestamp: Instant?
    public let resultingRevision: UInt64?

    /// An entry of the project feed or of history: `{sequence, operation: {id, actor, command, …}}`.
    init?(_ json: JSON) {
        let body = json["operation"]
        guard let sequence = json["sequence"].uint, let operation = body["id"].string else { return nil }
        self.sequence = sequence
        self.operation = operation
        actor = body["actor"].actorName ?? "unknown"
        timestamp = body["timestamp"].instant
        resultingRevision = body["resulting_revision"].uint
        if case .object(let command) = body["command"], let first = command.first {
            verb = first.key
            workIdentity = first.value["work"].string
            note = first.value["note"].string ?? first.value["reason"].string
        } else {
            verb = body["command"].string ?? "unknown"
            workIdentity = nil
            note = nil
        }
    }
}
