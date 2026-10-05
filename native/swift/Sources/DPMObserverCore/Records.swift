// Typed, immutable values the observer shows, read out of the shared query payloads.
//
// Nothing here decides anything. A readiness verdict, a rank, a freshness class and a gate are read
// from the payload exactly as the application wrote them; this file only gives them names, keeps
// words it does not know, and groups what the payload already says for presentation.

import DPMNative
import Foundation

// MARK: - Reading JSON

extension JSON {
    var double: Double? {
        switch self {
        case .number(let value): return value
        case .integer(let value): return Double(value)
        case .unsigned(let value): return Double(value)
        default: return nil
        }
    }

    var int: Int? { uint.flatMap { Int(exactly: $0) } }

    var items: [JSON] { array ?? [] }

    var instant: Instant? { string.flatMap { Instant($0) } }

    /// A principal as the application spells it on the command line: `agent:worker`.
    var actorName: String? {
        guard let kind = self["kind"].string, let name = self["name"].string else { return nil }
        return "\(kind.lowercased()):\(name)"
    }

    /// The first words of a long text, so a list row never carries a whole objective.
    func excerpt(_ limit: Int = 140) -> String {
        guard let text = string else { return "" }
        let flat = text.replacingOccurrences(of: "\n", with: " ")
        return flat.count > limit ? String(flat.prefix(limit)) + "…" : flat
    }
}

extension Instant {
    public var date: Date { Date(timeIntervalSince1970: TimeInterval(seconds) + TimeInterval(nanos) / 1e9) }
}

// MARK: - Work and decisions

public struct Acceptance: Hashable, Sendable {
    public let text: String
}

/// One ranked candidate of `next`, with the reasons the application gave for it.
public struct Candidate: Identifiable, Hashable, Sendable {
    public var id: String { identity }
    /// The task's persistent identity: what a selection follows, since a key can be renamed.
    public let identity: String
    public let key: String
    public let title: String
    public let priority: String
    public let rank: Int
    public let reasons: [String]
    public let objective: String
    public let acceptance: [String]
    public let critical: Bool
    public let capabilityMatch: Bool

    init?(_ json: JSON) {
        let work = json["work"]
        guard let identity = work["id"].string, let key = work["key"].string else { return nil }
        self.identity = identity
        self.key = key
        title = work["title"].string ?? key
        priority = work["schedule"]["priority"].string ?? ""
        rank = json["global_rank"].int ?? 0
        reasons = json["reasons"].items.compactMap { $0.string }
        objective = work["contract"]["objective"].string ?? ""
        acceptance = work["contract"]["acceptance"].items.compactMap { $0["text"].string }
        critical = json["critical"].bool ?? false
        capabilityMatch = json["capability_match"].bool ?? true
    }
}

/// Why work cannot start, grouped from the payload's own unmet gates. The count is how many tasks
/// the application reports as held by the same thing; no readiness is derived here.
public struct BlockerGroup: Identifiable, Hashable, Sendable {
    public enum Kind: String, Sendable { case decision, dependency, lifecycle, other }
    public var id: String { "\(kind.rawValue):\(key)" }
    public let kind: Kind
    /// The decision key, the predecessor key, the lifecycle word or the unmet type.
    public let key: String
    /// The persistent identity of the predecessor task, when the group is a dependency.
    public let workIdentity: String?
    public let summary: String
    public let tasks: Int
}

public struct StatusSummary: Hashable, Sendable {
    public let revision: UInt64
    public let totalWork: Int
    public let ready: Int
    public let inFlight: Int
    public let blocked: Int
    public let awaitingVerification: Int
    public let complete: Int
    public let openDecisions: Int
    public let basisInvalidated: Int
    public let unestimated: Int
    public let percentComplete: Double
    public let verified: Bool
    public let expectedFinishHours: Double?
    public let p50FinishHours: Double?
    public let p80FinishHours: Double?
    public let blockers: [BlockerGroup]

    init(_ view: View) {
        let data = view.envelope.data
        revision = view.envelope.revision ?? 0
        totalWork = data["total_work"].int ?? 0
        ready = data["ready"].int ?? 0
        inFlight = data["in_flight"].int ?? 0
        blocked = data["blocked"].int ?? 0
        awaitingVerification = data["awaiting_verification"].int ?? 0
        complete = data["complete"].int ?? 0
        openDecisions = data["open_decisions"].int ?? 0
        basisInvalidated = data["basis_invalidated"].int ?? 0
        unestimated = data["unestimated"].items.count
        percentComplete = data["progress"]["percent_complete"].double ?? 0
        verified = data["progress"]["verified"].bool ?? false
        expectedFinishHours = data["expected_finish_hours"].double
        p50FinishHours = data["p50_finish_hours"].double
        p80FinishHours = data["p80_finish_hours"].double
        blockers = Self.groups(data["gates"])
    }

    /// The unmet gates of every task that is not ready, one group per thing holding tasks back.
    static func groups(_ gates: JSON) -> [BlockerGroup] {
        guard case .object(let byWork) = gates else { return [] }
        var counts: [String: (BlockerGroup.Kind, String, String?, String, Int)] = [:]
        for (_, gate) in byWork where gate["ready"].bool == false {
            var seen = Set<String>()
            for unmet in gate["unmet"].items {
                let (kind, key, summary) = describe(unmet)
                let id = "\(kind.rawValue):\(key)"
                guard seen.insert(id).inserted else { continue }
                let held = counts[id]?.4 ?? 0
                counts[id] = (kind, key, unmet["predecessor"].string, summary, held + 1)
            }
        }
        return counts.values
            .map { BlockerGroup(kind: $0.0, key: $0.1, workIdentity: $0.2, summary: $0.3, tasks: $0.4) }
            .sorted { ($0.tasks, $1.id) > ($1.tasks, $0.id) }
    }

    /// One unmet gate in words, from what the payload carries. A type this build does not know is
    /// named, never guessed at.
    static func describe(_ unmet: JSON) -> (BlockerGroup.Kind, String, String) {
        switch unmet["type"].string {
        case "decision":
            let key = unmet["key"].string ?? "decision"
            return (.decision, key, "waits on decision \(key)")
        case "dependency":
            let key = unmet["key"].string ?? "predecessor"
            let status = unmet["status"].string ?? "unknown"
            let relation = unmet["relation"].string ?? "dependency"
            let release = unmet["release"]["state"].string ?? "unknown"
            return (.dependency, key, "waits on \(key) (\(status)); \(relation) \(release.replacingOccurrences(of: "_", with: " "))")
        case "lifecycle":
            let status = unmet["status"].string ?? "unknown"
            return (.lifecycle, status, "is already \(status)")
        case let other?:
            return (.other, other, "unmet \(other)")
        case nil:
            return (.other, "unknown", "an unmet gate this build cannot name")
        }
    }
}

/// A task as the shared snapshot names it: the only query that lists every task by key.
public struct WorkItem: Identifiable, Hashable, Sendable {
    public var id: String { identity }
    public let identity: String
    public let key: String
    public let title: String
    public let kind: String
    public let status: String
    public let owner: String?
    public let priority: String
    public let objective: String
    public let acceptance: [String]
    public let artifactIds: [String]
    public let attempts: [Attempt]
    public let blockReason: String?
    public let reportedProgress: Int
    public let startedAt: Instant?
    public let submittedAt: Instant?
    public let verifiedAt: Instant?
    /// The task's own three-point duration, exactly as the exported plan supplies it; nil when it has none.
    public let estimate: Estimate?

    /// Optimistic, likely and pessimistic elapsed hours, as written in the plan. Nothing is computed from them.
    public struct Estimate: Hashable, Sendable {
        public let optimisticHours: Double
        public let likelyHours: Double
        public let pessimisticHours: Double

        public init(optimisticHours: Double, likelyHours: Double, pessimisticHours: Double) {
            self.optimisticHours = optimisticHours
            self.likelyHours = likelyHours
            self.pessimisticHours = pessimisticHours
        }

        /// Read from `schedule.estimate` of a work item; nil when it is absent or not all three are numbers.
        init?(_ json: JSON) {
            guard let optimistic = json["optimistic_hours"].double, let likely = json["likely_hours"].double, let pessimistic = json["pessimistic_hours"].double else { return nil }
            self.init(optimisticHours: optimistic, likelyHours: likely, pessimisticHours: pessimistic)
        }
    }

    public struct Attempt: Hashable, Sendable {
        public let number: Int
        public let submittedAt: Instant?
        public let outcome: String
    }

    init?(_ json: JSON) {
        guard let identity = json["id"].string, let key = json["key"].string else { return nil }
        self.identity = identity
        self.key = key
        title = json["title"].string ?? key
        kind = json["kind"].string ?? "Task"
        let execution = json["execution"]
        status = execution["status"].string ?? "Unknown"
        owner = execution["owner"].actorName
        priority = json["schedule"]["priority"].string ?? ""
        estimate = Estimate(json["schedule"]["estimate"])
        objective = json["contract"]["objective"].string ?? ""
        acceptance = json["contract"]["acceptance"].items.compactMap { $0["text"].string }
        artifactIds = execution["artifact_ids"].items.compactMap { $0.string }
        attempts = execution["attempts"].items.map {
            Attempt(number: $0["number"].int ?? 0, submittedAt: $0["submitted_at"].instant, outcome: $0["outcome"]["state"].string ?? "unknown")
        }
        blockReason = execution["block_reason"].string
        reportedProgress = execution["reported_progress_percent"].int ?? 0
        startedAt = execution["events"]["started_at"].instant
        submittedAt = execution["events"]["submitted_at"].instant
        verifiedAt = execution["events"]["verified_at"].instant
    }
}

/// A decision as the shared snapshot records it. `blocks` is what it gates; `related` is context.
public struct DecisionInfo: Identifiable, Hashable, Sendable {
    public var id: String { identity }
    public let identity: String
    public let key: String
    public let question: String
    public let status: String
    public let outcome: String?
    public let rationale: String
    public let blocks: [String]
    public let related: [String]

    init?(_ json: JSON) {
        guard let identity = json["id"].string, let key = json["key"].string else { return nil }
        self.identity = identity
        self.key = key
        question = json["question"].string ?? ""
        status = json["status"].string ?? "Unknown"
        outcome = json["outcome"].string
        rationale = json["rationale"].string ?? ""
        blocks = json["blocks"].items.compactMap { $0.string }
        related = json["related_work"].items.compactMap { $0.string }
    }
}

public struct Artifact: Hashable, Sendable {
    public let identity: String
    public let label: String
    public let kind: String
    public let uri: String
    public let role: String?
}

public struct Inventory: Sendable, Equatable {
    public let projectTitle: String
    public let items: [WorkItem]
    public let artifacts: [String: Artifact]
    public let decisions: [DecisionInfo]
    public let byIdentity: [String: WorkItem]
    public let decisionByIdentity: [String: DecisionInfo]
    /// Every edge of the plan, as the snapshot lists it, with the edges at each end of a work item.
    public let relations: [GanttRelation]
    private let relationsAt: [String: [Int]]

    public static let empty = Inventory(items: [], artifacts: [:], decisions: [], projectTitle: "")

    init(items: [WorkItem], artifacts: [String: Artifact], decisions: [DecisionInfo], projectTitle: String, relations: [GanttRelation] = []) {
        self.items = items
        self.artifacts = artifacts
        self.decisions = decisions
        self.projectTitle = projectTitle
        self.relations = relations
        var at: [String: [Int]] = [:]
        for (position, relation) in relations.enumerated() {
            at[relation.predecessor, default: []].append(position)
            if relation.successor != relation.predecessor { at[relation.successor, default: []].append(position) }
        }
        relationsAt = at
        byIdentity = Dictionary(items.map { ($0.identity, $0) }, uniquingKeysWith: { first, _ in first })
        decisionByIdentity = Dictionary(decisions.map { ($0.identity, $0) }, uniquingKeysWith: { first, _ in first })
    }

    /// The decision that goes by this key, if the snapshot has one.
    public func decision(key: String) -> DecisionInfo? { decisions.first { $0.key == key } }

    /// The distinct lifecycle words of the tasks, for a filter.
    public var statuses: [String] { Array(Set(items.map(\.status))).sorted() }

    /// The tasks whose key, title, status or owner contains `text`, optionally of one status, the first
    /// `limit` of them, and how many matched in all. Drawing only `limit` rows keeps a large plan
    /// light, and the total says how many more a narrower search would reach.
    public func search(_ text: String, status: String? = nil, limit: Int) -> (items: [WorkItem], total: Int) {
        let needle = text.trimmingCharacters(in: .whitespaces).lowercased()
        var found: [WorkItem] = []
        var total = 0
        for item in items where status == nil || item.status == status {
            guard needle.isEmpty || [item.key, item.title, item.status, item.owner ?? ""].contains(where: { $0.lowercased().contains(needle) }) else { continue }
            total += 1
            if found.count < limit { found.append(item) }
        }
        return (found, total)
    }

    /// The decisions whose key or question contains `text`.
    public func decisions(matching text: String) -> [DecisionInfo] {
        let needle = text.trimmingCharacters(in: .whitespaces).lowercased()
        return needle.isEmpty ? decisions : decisions.filter { $0.key.lowercased().contains(needle) || $0.question.lowercased().contains(needle) }
    }

    /// The key a task goes by now. Keys can be renamed by a reviewed change; identity cannot.
    public func key(of identity: String) -> String? { byIdentity[identity]?.key }

    init(_ view: View) {
        let data = view.envelope.data
        var items: [WorkItem] = []
        if case .object(let work) = data["work_items"] { items = work.values.compactMap(WorkItem.init) }
        items.sort { $0.key.localizedStandardCompare($1.key) == .orderedAscending }
        var artifacts: [String: Artifact] = [:]
        if case .object(let found) = data["artifacts"] {
            for (identity, artifact) in found {
                artifacts[identity] = Artifact(
                    identity: identity, label: artifact["label"].string ?? identity, kind: artifact["kind"].string ?? "",
                    uri: artifact["uri"].string ?? "", role: artifact["metadata"]["role"].string)
            }
        }
        var decisions: [DecisionInfo] = []
        if case .object(let found) = data["decisions"] { decisions = found.values.compactMap(DecisionInfo.init) }
        decisions.sort { $0.key.localizedStandardCompare($1.key) == .orderedAscending }
        self.init(items: items, artifacts: artifacts, decisions: decisions, projectTitle: data["project"]["title"].string ?? data["projects"].items.first?["title"].string ?? "",
                  relations: data["dependencies"].items.compactMap(GanttRelation.init))
    }

    /// The relations that end or start at this work item: the plan's edges, not a derivation.
    public func relations(of identity: String) -> [GanttRelation] { (relationsAt[identity] ?? []).map { relations[$0] } }

    /// Work awaiting independent acceptance: its recorded lifecycle word is `Submitted`.
    public var submitted: [WorkItem] { items.filter { $0.status == "Submitted" } }
}

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
