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
            // What the relation means for this work, in the words a person uses for it.
            let wait: String
            switch relation {
            case "FinishStart": wait = "can start once \(key) finishes"
            case "StartStart": wait = "can start once \(key) starts"
            case "FinishFinish": wait = "can finish once \(key) finishes"
            case "StartFinish": wait = "can finish once \(key) starts"
            default: wait = "waits on \(key) (\(relation))"
            }
            let why: String
            switch release {
            case "awaiting_event": why = ""
            case "elapsing": why = "; its lag is still running"
            case "unrecorded_event_time": why = "; when \(key) did so was never recorded, so its lag cannot run"
            case "lag_out_of_range": why = "; its lag runs past the supported calendar"
            default: why = "; \(release.replacingOccurrences(of: "_", with: " "))"
            }
            return (.dependency, key, "\(wait) (\(key) is \(status))\(why)")
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
    /// The export as read, with the revision and lineage it was read at: the basis an edit is made against.
    public private(set) var basis: PlanBasis?

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
        basis = PlanBasis(plan: data, revision: view.envelope.revision ?? 0, lineage: view.envelope.lineageId)
    }

    /// The relations that end or start at this work item: the plan's edges, not a derivation.
    public func relations(of identity: String) -> [GanttRelation] { (relationsAt[identity] ?? []).map { relations[$0] } }

    /// Work awaiting independent acceptance: its recorded lifecycle word is `Submitted`.
    public var submitted: [WorkItem] { items.filter { $0.status == "Submitted" } }
}
