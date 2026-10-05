// Edits to the plan as a person makes them in the views, and the candidate plan they produce.
//
// Nothing here validates or schedules. A draft holds the exported plan it was started from, pinned to
// that revision and lineage, and a list of edits; `candidate` rewrites the exported JSON with them and
// nothing else. Whether the candidate is acceptable, what it changes and what it affects is the
// application's answer to `propose_change`; applying it is the application's `ApplyChange` command.

import DPMNative
import Foundation

/// Which end of a work item a dependency joins.
public enum PlanEnd: String, Sendable, Equatable { case start, finish }

/// One edit a person made, in the plan's own terms.
public enum PlanEdit: Equatable, Sendable {
    /// A dependency whose kind follows the ends joined: finish to start is FS, start to start SS, finish to
    /// finish FF and start to finish SF.
    case link(id: String, from: String, fromEnd: PlanEnd, to: String, toEnd: PlanEnd, lagHours: Double)
    case unlink(dependency: String)
    case estimate(work: String, optimistic: Double, likely: Double, pessimistic: Double)
    case parent(work: String, parent: String?)
    case title(work: String, text: String)
    case objective(work: String, text: String)
    case acceptance(work: String, criteria: [String])
    case kind(work: String, kind: String)
    case newTask(NewTask)
    case gate(decision: String, work: String, blocks: Bool)

    /// The relation kind the plan names for a pair of ends.
    public static func kind(from: PlanEnd, to: PlanEnd) -> String {
        switch (from, to) {
        case (.finish, .start): return "FinishStart"
        case (.start, .start): return "StartStart"
        case (.finish, .finish): return "FinishFinish"
        case (.start, .finish): return "StartFinish"
        }
    }
}

/// A new task as a person describes it; it enters the plan Proposed, as every new task must.
public struct NewTask: Equatable, Sendable {
    public var id: String
    public var key: String
    public var title: String
    public var objective: String
    public var acceptance: [String]
    public var parent: String?
    public var estimate: (optimistic: Double, likely: Double, pessimistic: Double)?

    public init(id: String = UUID().uuidString.lowercased(), key: String, title: String, objective: String, acceptance: [String],
                parent: String?, estimate: (optimistic: Double, likely: Double, pessimistic: Double)?) {
        self.id = id
        self.key = key
        self.title = title
        self.objective = objective
        self.acceptance = acceptance
        self.parent = parent
        self.estimate = estimate
    }

    public static func == (left: NewTask, right: NewTask) -> Bool {
        left.id == right.id && left.key == right.key && left.title == right.title && left.objective == right.objective
            && left.acceptance == right.acceptance && left.parent == right.parent
            && left.estimate.map { [$0.optimistic, $0.likely, $0.pessimistic] } == right.estimate.map { [$0.optimistic, $0.likely, $0.pessimistic] }
    }
}

/// The plan a draft started from: the export, with the revision and lineage it was read at.
public struct PlanBasis: Equatable, Sendable {
    public let plan: JSON
    public let revision: UInt64
    public let lineage: String?

    public init(plan: JSON, revision: UInt64, lineage: String?) {
        self.plan = plan
        self.revision = revision
        self.lineage = lineage
    }
}

/// Edits gathered against one basis, in the order they were made.
public struct PlanDraft: Equatable, Sendable {
    public let basis: PlanBasis
    public private(set) var edits: [PlanEdit] = []

    public init(basis: PlanBasis) { self.basis = basis }

    public var isEmpty: Bool { edits.isEmpty }

    public mutating func add(_ edit: PlanEdit) { edits.append(edit) }

    public mutating func remove(at index: Int) {
        guard edits.indices.contains(index) else { return }
        edits.remove(at: index)
    }

    /// The same edits against a newer basis, after a refusal for a stale revision or lineage: the person's
    /// work is kept, and it is validated again before it can apply.
    public func rebased(on basis: PlanBasis) -> PlanDraft {
        var draft = PlanDraft(basis: basis)
        draft.edits = edits
        return draft
    }

    /// The exported plan with every edit written into it, and nothing else changed.
    public func candidate() -> JSON {
        edits.reduce(basis.plan) { plan, edit in Self.write(edit, into: plan) }
    }

    static func write(_ edit: PlanEdit, into plan: JSON) -> JSON {
        switch edit {
        case let .link(id, from, fromEnd, to, toEnd, lag):
            let dependency: JSON = .object([
                "id": .string(id), "predecessor": .string(from), "successor": .string(to),
                "kind": .string(PlanEdit.kind(from: fromEnd, to: toEnd)), "lag_hours": .number(lag), "policy": .string("Hard"),
            ])
            return plan.setting(["dependencies"], to: .array(plan["dependencies"].items + [dependency]))
        case let .unlink(dependency):
            return plan.setting(["dependencies"], to: .array(plan["dependencies"].items.filter { $0["id"].string != dependency }))
        case let .estimate(work, optimistic, likely, pessimistic):
            let estimate: JSON = .object(["optimistic_hours": .number(optimistic), "likely_hours": .number(likely), "pessimistic_hours": .number(pessimistic)])
            return plan.setting(["work_items", work, "schedule", "estimate"], to: estimate)
        case let .parent(work, parent):
            return plan.setting(["work_items", work, "parent"], to: parent.map(JSON.string) ?? .null)
        case let .title(work, text):
            return plan.setting(["work_items", work, "title"], to: .string(text))
        case let .objective(work, text):
            return plan.setting(["work_items", work, "contract", "objective"], to: .string(text))
        case let .acceptance(work, criteria):
            return plan.setting(["work_items", work, "contract", "acceptance"], to: .array(criteria.map { .object(["text": .string($0)]) }))
        case let .kind(work, kind):
            // A milestone or package has no duration of its own; the application says whether the change is allowed.
            let changed = plan.setting(["work_items", work, "kind"], to: .string(kind))
            return kind == "Task" ? changed : changed.setting(["work_items", work, "schedule", "estimate"], to: .null)
        case let .newTask(task):
            return plan.setting(["work_items", task.id], to: newItem(task, in: plan))
        case let .gate(decision, work, blocks):
            var listed = plan["decisions"][decision]["blocks"].items.compactMap(\.string).filter { $0 != work }
            if blocks { listed.append(work) }
            return plan.setting(["decisions", decision, "blocks"], to: .array(listed.map(JSON.string)))
        }
    }

    /// A new task in the exported form: Proposed, unowned, with no evidence, in its parent's project.
    static func newItem(_ task: NewTask, in plan: JSON) -> JSON {
        let project = task.parent.flatMap { plan["work_items"][$0]["project"].string }
            ?? plan["projects"].objectKeys.sorted().first ?? ""
        let order = (plan["work_items"].objectValues.compactMap { $0["order"][0].int }.max() ?? 0) + 100
        let estimate: JSON = task.estimate.map {
            .object(["optimistic_hours": .number($0.optimistic), "likely_hours": .number($0.likely), "pessimistic_hours": .number($0.pessimistic)])
        } ?? .null
        return .object([
            "id": .string(task.id), "key": .string(task.key), "project": .string(project), "parent": task.parent.map(JSON.string) ?? .null,
            "kind": .string("Task"), "title": .string(task.title),
            "contract": .object([
                "objective": .string(task.objective), "acceptance": .array(task.acceptance.map { .object(["text": .string($0)]) }),
                "capabilities": .array([]), "requirement_ids": .array([]), "assets": .array([]),
            ]),
            "execution": .object([
                "status": .string("Proposed"), "reported_progress_percent": .integer(0), "artifact_ids": .array([]), "owner": .null, "block_reason": .null,
            ]),
            "schedule": .object(["priority": .string("P2"), "estimate": estimate]),
            "order": .array([.integer(Int64(order))]),
        ])
    }

    /// One edit in words, naming work by key.
    public func words(_ edit: PlanEdit, names: (String) -> String) -> String {
        switch edit {
        case let .link(_, from, fromEnd, to, toEnd, lag):
            let kind = GanttRelation.abbreviation(PlanEdit.kind(from: fromEnd, to: toEnd))
            return "Add \(kind) \(names(from)) → \(names(to))\(lag == 0 ? "" : " " + GanttRelation.lagWords(lag, basis: "Elapsed"))"
        case let .unlink(dependency):
            let edge = basis.plan["dependencies"].items.first { $0["id"].string == dependency }
            let kind = GanttRelation.abbreviation(edge?["kind"].string ?? "")
            return "Remove \(kind) \(names(edge?["predecessor"].string ?? "?")) → \(names(edge?["successor"].string ?? "?"))"
        case let .estimate(work, optimistic, likely, pessimistic):
            return "Estimate \(names(work)): \(GanttWords.hours(optimistic)) / \(GanttWords.hours(likely)) / \(GanttWords.hours(pessimistic))"
        case let .parent(work, parent):
            return "Move \(names(work)) under \(parent.map(names) ?? "the top level")"
        case let .title(work, text): return "Rename \(names(work)) to “\(text)”"
        case let .objective(work, _): return "Change the objective of \(names(work))"
        case let .acceptance(work, criteria): return "Set \(criteria.count) acceptance criteria of \(names(work))"
        case let .kind(work, kind): return "Make \(names(work)) a \(kind == "WorkPackage" ? "work package" : kind.lowercased())"
        case let .newTask(task): return "New task \(task.key): \(task.title)"
        case let .gate(decision, work, blocks): return blocks ? "Gate \(names(work)) on \(names(decision))" : "Stop gating \(names(work)) on \(names(decision))"
        }
    }
}

extension JSON {
    /// This value with the value at `path` replaced, creating objects along the way.
    public func setting(_ path: [String], to value: JSON) -> JSON {
        guard let head = path.first else { return value }
        var fields: [String: JSON] = [:]
        if case .object(let existing) = self { fields = existing }
        fields[head] = (fields[head] ?? .null).setting(Array(path.dropFirst()), to: value)
        return .object(fields)
    }

    public var objectKeys: [String] { if case .object(let fields) = self { return Array(fields.keys) } else { return [] } }
    public var objectValues: [JSON] { if case .object(let fields) = self { return Array(fields.values) } else { return [] } }
}
