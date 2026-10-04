// What a Gantt row says in words, built from the projection's own values. It lives with the model so
// the window and the qualification suite read the very same text: the full title, the kind and status,
// the priority apart from criticality, float, hours, and each relation with its kind, lead or lag and
// policy. A calendar date appears only where a query supplied one, and the projection supplies none.

import Foundation

public enum GanttWords {
    public static func hours(_ value: Double) -> String { String(format: "%.1f h", value) }

    public static func percent(_ fraction: Double) -> String { String(format: "%.0f%%", fraction * 100) }

    public static func kind(_ row: GanttRow) -> String {
        switch row.kind {
        case "WorkPackage": return "Work package"
        case "Milestone": return "Milestone"
        default: return row.kind
        }
    }

    /// A supplied hour count with every digit it was written with (no rounding), so the estimate reads as supplied.
    public static func exactHours(_ value: Double) -> String { "\(value) h" }

    /// The task's own three-point estimate as the exported plan supplies it, or an explicit absence. It is
    /// the item's own estimate, apart from the project's p50, p80 and p95, from priority, float and criticality.
    public static func estimate(_ estimate: WorkItem.Estimate?) -> String {
        guard let estimate = estimate else { return "no duration estimate is recorded for this item" }
        return "own estimate: optimistic \(exactHours(estimate.optimisticHours)), likely \(exactHours(estimate.likelyHours)), pessimistic \(exactHours(estimate.pessimisticHours))"
    }

    /// How a relation names a work item: its key, else the start of its identity.
    public static func namer(schedule: GanttSchedule?, inventory: Inventory) -> (String) -> String {
        { identity in schedule?.row(identity)?.key ?? inventory.key(of: identity) ?? String(identity.prefix(8)) }
    }

    /// The label and value assistive technology reads for a row. The label is the kind, key and the
    /// whole title, never shortened; the value is everything else the row shows.
    public static func row(_ item: GanttOutlineRow, schedule: GanttSchedule?, inventory: Inventory, selected: String?) -> (label: String, value: String) {
        let row = item.row
        var parts: [String] = ["status \(row.status)"]
        if !row.priority.isEmpty { parts.append("human priority \(row.priority)") }
        if let span = row.span {
            parts.append(row.isMilestone ? "milestone at \(hours(span.start)) elapsed" : "elapsed hours \(hours(span.start)) to \(hours(span.finish)), \(hours(span.hours)) long")
            parts.append(span.critical ? "on the critical path" : "not on the critical path")
        } else {
            parts.append("not scheduled: \(row.applicability)")
        }
        parts.append(inventory.byIdentity[row.id].map { estimate($0.estimate) } ?? "own estimate not available: the plan snapshot does not list this item")
        parts.append("no calendar date is supplied")
        if let times = row.times { parts.append("total float \(hours(times.totalFloat)), free float \(hours(times.freeFloat))") }
        parts.append(row.criticality.map { "criticality \(percent($0)) of simulated schedules" } ?? "criticality not supplied")
        if let calendar = row.calendar { parts.append(calendar) }
        parts.append("level \(item.depth + 1)")
        if item.children > 0 { parts.append(item.expanded ? "expanded, \(item.children) children shown" : "collapsed, \(item.children) children hidden") }
        if selected == row.id { parts.append("selected") }
        let relations = inventory.relations(of: row.id)
        if !relations.isEmpty {
            let names = namer(schedule: schedule, inventory: inventory)
            parts.append("relations: " + relations.prefix(8).map { $0.words(names: names) }.joined(separator: "; ") + (relations.count > 8 ? "; and \(relations.count - 8) more in Detail" : ""))
        }
        return ("\(kind(row)) \(row.key): \(row.title)", parts.joined(separator: ". "))
    }
}
