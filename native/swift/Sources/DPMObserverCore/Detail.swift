// Readable detail for one task or run, built from the shared `explain` and `run` payloads.
//
// Every section is plain text so the interface can wrap and scroll it whole: a complete title,
// objective, acceptance criterion, requirement, decision, risk or reference is never cut for width.
// Words the application wrote (a gate, a reason, a status) are shown as written.

import DPMNative
import Foundation

struct SectionBuilder {
    private(set) var sections: [DetailSection] = []

    mutating func add(_ title: String, _ rows: [(String?, String)]) {
        let kept = rows.filter { !$0.1.isEmpty }
        guard !kept.isEmpty else { return }
        sections.append(DetailSection(title: title, rows: kept.enumerated().map { DetailRow(id: $0.offset, label: $0.element.0, text: $0.element.1) }))
    }

    mutating func list(_ title: String, _ texts: [String]) { add(title, texts.map { (nil, $0) }) }
}

func hours(_ value: Double?) -> String { value.map { String(format: "%.1f h", $0) } ?? "unknown" }

func stamp(_ instant: Instant?) -> String {
    guard let instant = instant else { return "unknown" }
    return instant.description.replacingOccurrences(of: "T", with: " ").replacingOccurrences(of: ".000000000Z", with: "Z")
}

public enum DetailBuilder {
    /// A gate report as the words of its unmet items, or that it is open.
    static func gate(_ report: JSON) -> String {
        if report["ready"].bool == true { return "open: every gate for this transition is satisfied" }
        let reasons = report["unmet"].items.map { StatusSummary.describe($0).2 }
        return reasons.isEmpty ? "not ready" : "not ready: " + reasons.joined(separator: "; ")
    }

    /// One task in full, from its `explain` view.
    public static func work(_ view: View, name: (String) -> String) -> SubjectDetail {
        let data = view.envelope.data
        let work = data["work"]
        let contract = work["contract"]
        let execution = work["execution"]
        let key = work["key"].string ?? "work"
        var out = SectionBuilder()
        out.add("Summary", [
            ("Key", key), ("Kind", work["kind"].string ?? ""), ("Priority", work["schedule"]["priority"].string ?? ""),
            ("Status", execution["status"].string ?? "unknown"),
            ("Owner", execution["owner"].actorName ?? "no owner"),
            ("Reported progress", "\(execution["reported_progress_percent"].int ?? 0)% — the owner's report, not acceptance. Verified: \(data["progress"]["verified"].bool == true ? "yes" : "no")"),
            ("Blocked", execution["block_reason"].string ?? ""),
        ])
        out.list("Why now", data["why_now"].items.compactMap { $0.string })
        out.list("Objective", [contract["objective"].string ?? ""])
        out.list("Acceptance criteria", contract["acceptance"].items.enumerated().compactMap { index, item in item["text"].string.map { "\(index + 1). \($0)" } })
        let instructions = contract["instructions"]
        out.add("Steps", instructions["steps"].items.enumerated().map { ("\($0.offset + 1). \($0.element["action"].string ?? "")", $0.element["expected_result"].string.map { "Expected: \($0)" } ?? "") })
        out.list("In scope", instructions["in_scope"].items.compactMap { $0.string })
        out.list("Out of scope", instructions["out_of_scope"].items.compactMap { $0.string })
        out.list("Verification checks", instructions["verification"].items.compactMap { $0.string })
        let transitions = data["transitions"]
        out.add("Readiness", ["claim", "start", "submit", "verify"].compactMap { step in
            transitions[step] == .null ? nil : (step.capitalized, gate(transitions[step]))
        })
        // The item's own estimate, as `explain` supplies it, apart from priority, float and criticality.
        out.add("Estimate (this item's own, elapsed hours)", [
            ("Three-point estimate", GanttWords.estimate(WorkItem.Estimate(work["schedule"]["estimate"]))),
        ])
        let schedule = data["schedule"]
        if schedule != .null {
            out.add("Schedule (elapsed hours, not calendar dates)", [
                ("Earliest start / finish", "\(hours(schedule["earliest_start_hours"].double)) / \(hours(schedule["earliest_finish_hours"].double))"),
                ("Total float", hours(schedule["total_float_hours"].double)),
                ("Critical path", schedule["critical"].bool == true ? "yes" : "no"),
                ("Criticality in simulations", data["criticality"].double.map { String(format: "%.0f%%", $0 * 100) } ?? "unknown"),
                ("No duration estimate", data["unestimated"].bool == true ? "yes — the forecast counts this task as 0 h" : ""),
            ])
        }
        let context = data["context"]
        out.add("Predecessors", data["predecessors"].items.map { ($0["key"].string, "\($0["title"].string ?? "") — \($0["execution"]["status"].string ?? "")") })
        // Each relation in words: its kind (FS, SS, FF or SF), lead or lag, and Hard or Soft policy.
        out.add("Dependencies", context["dependencies"].items.compactMap { json in
            guard let relation = GanttRelation(json) else { return nil }
            return (relation.abbreviation, relation.words(names: name))
        })
        out.add("Successors", context["successors"].items.map { ($0["key"].string, "\($0["title"].string ?? "") — \($0["execution"]["status"].string ?? "")") })
        out.add("Parents", context["parents"].items.map { ($0["key"].string, $0["title"].string ?? "") })
        out.add("Requirements", context["requirements"].items.map { ($0["key"].string, "\($0["title"].string ?? "") — \($0["statement"].string ?? "")") })
        out.add("Decisions", context["decisions"].items.map {
            ($0["key"].string, "\($0["status"].string ?? "") — \($0["question"].string ?? "")" + ($0["outcome"].string.map { " Outcome: \($0)" } ?? ""))
        })
        out.add("Risks", context["risks"].items.map {
            ($0["key"].string, "\($0["description"].string ?? "") Impact \($0["impact"].string ?? "?"). Mitigation: \($0["mitigation"].string ?? "none recorded")")
        })
        out.add("Artifacts and evidence", context["artifacts"].items.map {
            ($0["label"].string, "\($0["kind"].string ?? "") \($0["uri"].string ?? "")" + ($0["metadata"]["role"].string.map { " (\($0))" } ?? ""))
        })
        out.add("Assets", context["assets"].items.map { ($0["key"].string, $0["label"].string ?? "") })
        out.add("Submissions", execution["attempts"].items.map {
            ("Attempt \($0["number"].int ?? 0)", "submitted \(stamp($0["submitted_at"].instant)); outcome \($0["outcome"]["state"].string ?? "unknown")")
        })
        var detail = SubjectDetail(
            subject: .work(work["id"].string ?? key), title: work["title"].string ?? key, subtitle: "\(key) · \(execution["status"].string ?? "")",
            sections: out.sections, revision: view.envelope.revision, loading: false, error: nil)
        detail.network = NetworkReport(data)
        return detail
    }

    /// One run in full, from its `run` view and lifecycle. The contract the run observed when it
    /// started is immutable and shown apart from the task as it stands now.
    public static func run(_ view: View, lifecycle: [LifecycleEntry], lifecycleNote: String? = nil, operations: [OperationEntry], inventory: Inventory) -> SubjectDetail {
        let data = view.envelope.data
        let name = { (identity: String) in inventory.key(of: identity) ?? String(identity.prefix(8)) }
        guard let summary = RunSummary(data) else {
            return SubjectDetail(subject: .run(data["run"]["id"].string ?? ""), title: "Run", subtitle: "", sections: [], revision: view.envelope.revision, loading: false, error: "the answer was not a run")
        }
        var out = SectionBuilder()
        let observation: String
        switch summary.observation {
        case .known(.managed): observation = "managed — a service hosts or watches the executor, so silence is evidence of a problem"
        case .known(.reportedOnly): observation = "reported only — the executor reports itself through the CLI or tools; silence proves nothing and a silent run goes stale, never idle or finished"
        case .unknown(let word): observation = word
        }
        out.add("Run", [
            ("Task", summary.workKey), ("Executor", summary.executor), ("Recorded by", summary.recordedBy), ("Observation", observation),
            ("Started", stamp(summary.startedAt)), ("Parent run", data["run"]["parent"].string ?? ""),
        ])
        let word = summary.status
        out.add("State", [
            ("Executor's last report", "\(summary.state)\(summary.stateDetail.map { " — \($0)" } ?? "") since \(stamp(summary.stateSince))"),
            ("Observed now", word == "stale" ? "stale — nothing has been received for too long; the run is not known to be running or finished" : word),
            ("Last receipt", stamp(summary.lastReceiptAt)),
            ("Turns stale at", stamp(summary.staleAt)),
            ("Task relation", summary.orphan.map { "does not match this run: \($0)" } ?? ""),
            ("History", summary.foreignLineage ? "recorded under another history than this store continues; not known to be running here" : ""),
            ("Meaning", "A run's completion is the executor's report. It never submits, verifies or releases the task."),
        ])
        // What the run saw when it started: fixed for the life of the run, whatever the task became.
        let observed = data["run"]["contract"]
        let contract = observed["contract"]
        out.add("Contract observed when the run started (fixed)", [
            ("Workspace", observed["workspace_id"].string ?? ""), ("Lineage", observed["lineage_id"].string ?? "none"),
            ("Revision", observed["revision"].uint.map { String($0) } ?? ""), ("Task key then", observed["work_key"].string ?? ""),
            ("Earlier submissions then", observed["prior_submissions"].int.map { String($0) } ?? ""),
            ("Objective", contract["objective"].string ?? ""),
        ])
        out.list("Acceptance criteria then", contract["acceptance"].items.enumerated().compactMap { index, item in item["text"].string.map { "\(index + 1). \($0)" } })
        if let item = inventory.byIdentity[summary.workIdentity] {
            out.add("The task now (it may have moved on)", [("Status", item.status), ("Owner", item.owner ?? "none"), ("Reported progress", "\(item.reportedProgress)%")])
        }
        out.add("Source references", data["run"]["sources"].items.map { ("Requested", flat($0)) } + data["run"]["exact_sources"].items.map { ("Exact, captured at the start", flat($0)) })
        let session = data["run"]["session"]
        let provenance = session["provenance"]
        out.add("Provider and configuration", [
            ("Provider", session["provider"].string ?? ""), ("Session", session["session"].string ?? ""), ("Turn", session["turn"].string ?? ""),
            ("Requested model", provenance["requested_model"].string ?? ""), ("Observed model", provenance["observed_model"].string ?? ""),
            ("Runtime", provenance["runtime_version"].string ?? ""), ("Configuration digest", provenance["configuration_digest"].string ?? ""),
        ])
        out.add("Activity", [("Records", "\(summary.recorded) received, \(summary.retained) retained"), ("Latest", summary.latestKind.map { "\($0) at \(stamp(summary.latestAt))" } ?? "none yet")])
        out.add("Lifecycle", (lifecycleNote.map { [(String?.some("Coverage"), $0)] } ?? []) + lifecycle.map { (stamp($0.recordedAt), "\($0.state)\($0.detail.map { " — \($0)" } ?? "") (\($0.recordedBy))") })
        let known = Dictionary(operations.map { ($0.operation, $0) }, uniquingKeysWith: { first, _ in first })
        out.add("Linked operations", summary.operations.map { identity in
            guard let entry = known[identity] else { return (nil, identity) }
            return (entry.verb, "\(entry.actor) \(entry.workIdentity.map { name($0) } ?? "") \(entry.note ?? "")")
        })
        return SubjectDetail(
            subject: .run(summary.identity), title: "Run of \(summary.workKey)", subtitle: "\(summary.executor) · \(summary.state) / \(summary.status)",
            sections: out.sections, revision: view.envelope.revision, loading: false, error: nil)
    }

    /// A decision: what it asks and has decided, and the work it gates, from the shared snapshot.
    public static func decision(_ info: DecisionInfo, inventory: Inventory) -> SubjectDetail {
        var out = SectionBuilder()
        out.add("Decision", [("Key", info.key), ("Status", info.status), ("Outcome", info.outcome ?? "not decided")])
        out.list("Question", [info.question])
        out.list("Rationale", [info.rationale])
        let gated = info.blocks.map { identity in inventory.byIdentity[identity].map { ($0.key as String?, "\($0.title) — \($0.status)") } ?? (nil, identity) }
        out.add("Work this decision gates (\(gated.count))", Array(gated.prefix(60)) + (gated.count > 60 ? [(nil, "and \(gated.count - 60) more; find them with the search in Detail")] : []))
        let related = info.related.map { identity in inventory.byIdentity[identity].map { ($0.key as String?, $0.title) } ?? (nil, identity) }
        out.add("Related work (context only, not gated)", Array(related.prefix(30)))
        return SubjectDetail(subject: .decision(info.identity), title: info.key, subtitle: "Decision · \(info.status)", sections: out.sections, revision: nil, loading: false, error: nil)
    }

    /// A JSON value as one line of `name: value` pairs, for references whose shape grows over time.
    static func flat(_ json: JSON, prefix: String = "") -> String {
        switch json {
        case .object(let fields):
            return fields.keys.sorted().map { flat(fields[$0] ?? .null, prefix: prefix.isEmpty ? $0 : "\(prefix).\($0)") }.filter { !$0.isEmpty }.joined(separator: ", ")
        case .array(let items): return items.map { flat($0, prefix: prefix) }.joined(separator: "; ")
        case .string(let text): return prefix.isEmpty ? text : "\(prefix): \(text)"
        case .null: return ""
        default: return prefix.isEmpty ? "\(json)" : "\(prefix): \(json)"
        }
    }
}
