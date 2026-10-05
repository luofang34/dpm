// The Gantt's data and layout: the shared `schedule` projection decoded as written, the relations
// read from the plan snapshot, and the pure rules that turn them into rows to draw.
//
// Nothing here schedules. Every hour, float, criticality and percentile is the application's own
// value; a bar is its `span`, a relation is an edge of the snapshot, and a calendar date exists only
// where a query supplies one (the projection supplies none: its hours are elapsed from the instant
// it was evaluated at). Collapse, filters, zoom and pan are view state and change only what is drawn.

import DPMNative
import Foundation

// MARK: - The projection

public struct GanttSpan: Equatable, Sendable {
    /// Earliest start of the work, or of the earliest descendant of a package, in elapsed hours.
    public let start: Double
    public let finish: Double
    /// Whether the work, or any descendant of a package, is on the critical path, as the query says.
    public let critical: Bool

    public var hours: Double { finish - start }
}

public struct GanttTimes: Equatable, Sendable {
    public let earliestStart: Double
    public let earliestFinish: Double
    public let latestStart: Double
    public let latestFinish: Double
    public let totalFloat: Double
    public let freeFloat: Double
    public let critical: Bool
}

public struct GanttRow: Identifiable, Equatable, Sendable {
    public var id: String { identity }
    public let identity: String
    public let key: String
    public let title: String
    /// `Task`, `WorkPackage` or `Milestone`, as the application names them.
    public let kind: String
    public let parent: String?
    public let status: String
    /// Whether the work is in the active graph, in the application's words.
    public let applicability: String
    public let applicable: Bool
    /// The human priority. It is never criticality and is shown apart from it.
    public let priority: String
    public let span: GanttSpan?
    public let times: GanttTimes?
    /// The fraction of seeded simulations in which the work is critical; nil when the query gave none.
    public let criticality: Double?
    /// Calendar placement, in words, when the plan has calendars. Never a date.
    public let calendar: String?

    public var isMilestone: Bool { kind == "Milestone" }
    public var isPackage: Bool { kind == "WorkPackage" }

    init?(_ json: JSON) {
        guard let identity = json["id"].string, let key = json["key"].string else { return nil }
        self.identity = identity
        self.key = key
        title = json["title"].string ?? key
        kind = json["kind"].string ?? "Task"
        parent = json["parent"].string
        status = json["status"].string ?? "Unknown"
        let state = json["applicability"]["state"].string ?? "applicable"
        applicable = state == "applicable"
        applicability = Self.words(json["applicability"], state: state)
        priority = json["priority"].string ?? ""
        let span = json["span"]
        if let start = span["start_hours"].double, let finish = span["finish_hours"].double {
            self.span = GanttSpan(start: start, finish: finish, critical: span["critical"].bool ?? false)
        } else {
            self.span = nil
        }
        let times = json["times"]
        if let early = times["earliest_start_hours"].double, let earlyFinish = times["earliest_finish_hours"].double,
           let late = times["latest_start_hours"].double, let lateFinish = times["latest_finish_hours"].double,
           let total = times["total_float_hours"].double, let free = times["free_float_hours"].double {
            self.times = GanttTimes(earliestStart: early, earliestFinish: earlyFinish, latestStart: late, latestFinish: lateFinish,
                                    totalFloat: total, freeFloat: free, critical: times["critical"].bool ?? false)
        } else {
            self.times = nil
        }
        criticality = json["criticality"].double
        let placed = json["calendar"]
        if let name = placed["calendar"].string {
            let who = placed["executor"].string.map { ", executor \($0)" } ?? ""
            let how = placed["source"].string.map { ", chosen by \($0)" } ?? ""
            let wait = placed["review_wait_hours"].double.map { $0 > 0 ? ", review waits \(hours($0))" : "" } ?? ""
            calendar = "calendar \(name)\(who)\(how)\(wait)"
        } else {
            calendar = nil
        }
    }

    private static func words(_ json: JSON, state: String) -> String {
        switch state {
        case "applicable": return "in the active plan"
        case "undecided": return "undecided: waits on decision \(json["decision"].string ?? "?") choosing \(json["option"].string ?? "?")"
        case "not_selected": return "not selected: decision \(json["decision"].string ?? "?") chose \(json["selected"].string ?? "?"), this needs \(json["option"].string ?? "?")"
        case "awaiting_choice": return "awaiting a choice upstream: \(json["predecessor"].string ?? "?")"
        default: return state.replacingOccurrences(of: "_", with: " ")
        }
    }
}

public struct GanttUncertainty: Equatable, Sendable {
    public let iterations: Int
    public let seed: UInt64
    public let p50: Double
    public let p80: Double
    public let p95: Double
}

/// The shared schedule projection of every work item at one clock reading, as the application wrote it.
public struct GanttSchedule: Equatable, Sendable {
    /// Distinguishes one reading from the next, so a cached outline is dropped exactly when it changes.
    public let token = UUID()
    public let rows: [GanttRow]
    public let index: [String: Int]
    public let projectFinishHours: Double
    public let uncertainty: GanttUncertainty?
    public let evaluatedAt: Instant
    public let revision: UInt64?
    /// The distinct lifecycle words of the rows, for the status filter.
    public let statuses: [String]

    public static func == (left: GanttSchedule, right: GanttSchedule) -> Bool { left.token == right.token }

    public init(_ view: View) {
        let data = view.envelope.data
        let found = data["work"].items.compactMap(GanttRow.init)
        rows = found
        index = Dictionary(found.enumerated().map { ($0.element.identity, $0.offset) }, uniquingKeysWith: { first, _ in first })
        projectFinishHours = data["project_finish_hours"].double ?? 0
        let spread = data["uncertainty"]
        if let p50 = spread["p50_finish_hours"].double, let p80 = spread["p80_finish_hours"].double, let p95 = spread["p95_finish_hours"].double {
            uncertainty = GanttUncertainty(iterations: spread["iterations"].int ?? 0, seed: spread["seed"].uint ?? 0, p50: p50, p80: p80, p95: p95)
        } else {
            uncertainty = nil
        }
        evaluatedAt = view.evaluatedAt
        revision = view.envelope.revision
        statuses = Array(Set(found.map(\.status))).sorted()
    }

    public func row(_ identity: String) -> GanttRow? { index[identity].map { rows[$0] } }

    public var milestones: Int { rows.filter(\.isMilestone).count }
}

// MARK: - Relations

/// One edge of the plan, from the snapshot the observer already reads: what it constrains, by how
/// much, and whether it can be waived. The words repeat the structure; none of it is derived.
public struct GanttRelation: Identifiable, Hashable, Sendable {
    public var id: String
    public let predecessor: String
    public let successor: String
    /// `FinishStart`, `StartStart`, `FinishFinish` or `StartFinish`, as the plan writes it.
    public let kind: String
    public let lagHours: Double
    /// `Elapsed` or `Working`.
    public let lagBasis: String
    /// `Hard` or `Soft`.
    public let policy: String
    public let waived: Bool

    init?(_ json: JSON) {
        guard let predecessor = json["predecessor"].string, let successor = json["successor"].string else { return nil }
        id = json["id"].string ?? "\(predecessor)>\(successor)"
        self.predecessor = predecessor
        self.successor = successor
        kind = json["kind"].string ?? "FinishStart"
        lagHours = json["lag_hours"].double ?? 0
        lagBasis = json["lag_basis"].string ?? "Elapsed"
        policy = json["policy"].string ?? "Hard"
        waived = json["waiver"] != .null
    }

    public init(id: String, predecessor: String, successor: String, kind: String, lagHours: Double, lagBasis: String = "Elapsed", policy: String = "Hard", waived: Bool = false) {
        self.id = id
        self.predecessor = predecessor
        self.successor = successor
        self.kind = kind
        self.lagHours = lagHours
        self.lagBasis = lagBasis
        self.policy = policy
        self.waived = waived
    }

    /// The two-letter name of the relation.
    public var abbreviation: String { Self.abbreviation(kind) }

    public static func abbreviation(_ kind: String) -> String {
        switch kind {
        case "FinishStart": return "FS"
        case "StartStart": return "SS"
        case "FinishFinish": return "FF"
        case "StartFinish": return "SF"
        default: return kind
        }
    }

    public static func meaning(_ kind: String) -> String {
        switch kind {
        case "FinishStart": return "finish to start"
        case "StartStart": return "start to start"
        case "FinishFinish": return "finish to finish"
        case "StartFinish": return "start to finish"
        default: return kind
        }
    }

    /// The lead or lag in words: a negative value is a lead.
    public static func lagWords(_ lag: Double, basis: String = "Elapsed") -> String {
        let unit = basis == "Working" ? "working h" : "h"
        if lag == 0 { return "no lag" }
        return lag < 0 ? "lead \(String(format: "%.1f", -lag)) \(unit)" : "lag \(String(format: "%.1f", lag)) \(unit)"
    }

    public static func policyWords(_ policy: String, waived: Bool = false) -> String {
        switch policy {
        case "Soft": return waived ? "Soft (waived)" : "Soft: may be waived by a recorded waiver"
        case "Hard": return "Hard: always enforced"
        default: return policy
        }
    }

    /// The relation as one text, from the point of view of a work item that is one of its ends.
    public func words(names: (String) -> String) -> String {
        "\(abbreviation) (\(Self.meaning(kind))) \(names(predecessor)) → \(names(successor)), \(Self.lagWords(lagHours, basis: lagBasis)), \(Self.policyWords(policy, waived: waived))"
    }
}

// MARK: - View state

public struct GanttFilter: Equatable, Sendable {
    public var text = ""
    /// Only work of this lifecycle word; nil for any.
    public var status: String?
    /// Only work the query puts on the critical path.
    public var criticalOnly = false

    public init() {}

    public var isActive: Bool { !text.trimmingCharacters(in: .whitespaces).isEmpty || status != nil || criticalOnly }

    func matches(_ row: GanttRow) -> Bool {
        if let status = status, row.status != status { return false }
        if criticalOnly, row.span?.critical != true { return false }
        let needle = text.trimmingCharacters(in: .whitespaces).lowercased()
        return needle.isEmpty || [row.key, row.title, row.status, row.kind].contains { $0.lowercased().contains(needle) }
    }

    public var words: String {
        var parts: [String] = []
        if !text.isEmpty { parts.append("text “\(text)”") }
        if let status = status { parts.append("status \(status)") }
        if criticalOnly { parts.append("critical only") }
        return parts.isEmpty ? "no filter" : parts.joined(separator: ", ")
    }
}

/// What a person changed about how the Gantt is drawn. None of it is part of the project.
public struct GanttViewState: Equatable, Sendable {
    public var collapsed: Set<String> = []
    public var filter = GanttFilter()
    public var zoomLevel = GanttLayout.defaultZoom
    /// Offsets in points from the origin of the timeline and of the first row.
    public var panX = 0.0
    public var panY = 0.0
    /// The row the keyboard cursor is on, by persistent identity.
    public var focus: String?
    /// Counts every applied view operation: the generation a draw must carry to complete it.
    public var sequence = 0
    public var viewportRows = 28
    public var viewportWidth = 700.0
    /// Whether the filter field has the keyboard, so typed letters are text and not commands.
    public var editingFilter = false
    /// A scripted scroll request, in points, that overrides the pan on one axis while a measurement runs.
    public var drive: GanttDrive?
    /// Points per elapsed hour that Fit timeline derived from the measured width and the whole plan; while set it
    /// replaces the zoom step's scale. A zoom step, Reset or a new zoom clears it.
    public var fitScale: Double?
    /// Whether the initial fit of the first valid schedule was applied, or the person chose a viewport first.
    public var viewportChosen = false
    /// Why the last Locate selected could not show the selected row (the filter hides it); nil otherwise. The next view
    /// operation clears it.
    public var locateNote: String?

    public init() {}

    /// Points per elapsed hour as drawn now.
    public var scale: Double { fitScale ?? GanttLayout.pointsPerHour(zoomLevel) }
}

/// One scripted scroll request of a measurement. Its identity is its window, its axis and its number in the
/// window, so no two requests of a process share one, across axes, across windows or across repetitions.
public struct GanttDrive: Equatable, Sendable {
    public enum Axis: String, Sendable { case vertical, horizontal }
    /// Points of logical travel per request, as the measurement record defines it.
    public static let step = 40.0

    public var axis: Axis
    /// The scroll window this request belongs to; unique in the process.
    public var window: Int
    /// The request's number within its window, from 1.
    public var sequence: Int

    public init(axis: Axis, window: Int, sequence: Int) {
        self.axis = axis
        self.window = window
        self.sequence = sequence
    }

    /// The distance the request asks for, unbounded: the content is finite, so it is not an offset.
    public var logicalDistance: Double { Self.step * Double(sequence) }
    /// The generation as the log names it.
    public var generation: String { "\(window):\(axis.rawValue):\(sequence)" }
    /// The key of the expected facts and of the completion.
    public var key: String { "frame:\(generation)" }
}

/// The drawing side of a scripted scroll over finite content. A request asks for a logical distance; the
/// content wraps over its scroll range (its extent less the viewport), so a request past the end shows the
/// content again from its start. With no scroll range there is nothing to scroll, and the offset is 0.
public enum ScrollGeometry {
    public static func range(extent: Double, viewport: Double) -> Double { max(0, extent - viewport) }

    public static func effectiveOffset(logical: Double, extent: Double, viewport: Double) -> Double {
        let span = range(extent: extent, viewport: viewport)
        guard span > 0 else { return 0 }
        return logical.truncatingRemainder(dividingBy: span)
    }
}

/// The driver's side: the offset a request must show, worked out here and not read from the draw, so a draw
/// that places the content elsewhere disagrees with it. nil when there is no scroll range.
public enum ScrollPlan {
    public static func expectedOffset(distance: Double, range: Double) -> Double? {
        guard range > 0 else { return nil }
        return distance - range * (distance / range).rounded(.down)
    }
}

public struct GanttOutlineRow: Identifiable, Equatable, Sendable {
    public var id: String { row.identity }
    public let row: GanttRow
    public let depth: Int
    /// How many of its children the filter keeps; a row with none has no disclosure.
    public let children: Int
    public let expanded: Bool
    /// Its position among the rows drawn.
    public let position: Int
}

public enum GanttLayout {
    public static let rowHeight = 28.0
    /// Points per elapsed hour at each zoom level.
    public static let zoomSteps: [Double] = [0.25, 0.5, 1, 2, 4, 8, 16, 32]
    public static let defaultZoom = 3
    /// A bar of no length is still drawn this wide, so it can be seen and read; its text says 0 h.
    public static let minimumBar = 3.0

    public static func pointsPerHour(_ level: Int) -> Double { zoomSteps[max(0, min(zoomSteps.count - 1, level))] }

    /// The rows to draw, in plan hierarchy order: a work package's children follow it, in the order the
    /// projection lists them. A collapsed package hides its descendants; a filter keeps matching rows
    /// and the packages that contain them. Selection is never an input: it cannot be lost here.
    public static func outline(_ schedule: GanttSchedule, collapsed: Set<String>, filter: GanttFilter) -> [GanttOutlineRow] {
        let rows = schedule.rows
        var parentOf = [Int?](repeating: nil, count: rows.count)
        var children = [[Int]](repeating: [], count: rows.count)
        var roots: [Int] = []
        for (position, row) in rows.enumerated() {
            if let parent = row.parent, let found = schedule.index[parent], found != position {
                parentOf[position] = found
                children[found].append(position)
            } else {
                roots.append(position)
            }
        }
        var keep = [Bool](repeating: true, count: rows.count)
        if filter.isActive {
            keep = rows.map { filter.matches($0) }
            for position in rows.indices where keep[position] {
                var cursor = parentOf[position]
                var hops = 0
                while let up = cursor, !keep[up], hops < rows.count {
                    keep[up] = true
                    cursor = parentOf[up]
                    hops += 1
                }
            }
        }
        var out: [GanttOutlineRow] = []
        out.reserveCapacity(rows.count)
        var stack: [(Int, Int)] = roots.reversed().map { ($0, 0) }
        while let (position, depth) = stack.popLast() {
            guard keep[position] else { continue }
            let kept = children[position].filter { keep[$0] }
            let open = !collapsed.contains(rows[position].identity)
            out.append(GanttOutlineRow(row: rows[position], depth: depth, children: kept.count, expanded: open, position: out.count))
            if open { for child in kept.reversed() { stack.append((child, depth + 1)) } }
        }
        return out
    }

    /// Where a bar sits on the timeline, in points from the timeline's origin.
    public static func bar(_ span: GanttSpan, level: Int) -> (x: Double, width: Double) {
        bar(span, scale: pointsPerHour(level))
    }

    public static func bar(_ span: GanttSpan, scale: Double) -> (x: Double, width: Double) {
        (span.start * scale, max(span.hours * scale, minimumBar))
    }

    /// The content extent along an axis and the viewport it is seen through, in points: the timeline's width
    /// and the window's width, or the rows' height and the rows' viewport.
    public static func scrollExtent(_ axis: GanttDrive.Axis, schedule: GanttSchedule?, rows: Int, state: GanttViewState) -> (extent: Double, viewport: Double) {
        switch axis {
        case .horizontal: return (schedule.map { timelineWidth($0, scale: state.scale) } ?? 1, state.viewportWidth)
        case .vertical: return (max(rowHeight, Double(rows) * rowHeight), Double(state.viewportRows) * rowHeight)
        }
    }

    /// The width of the whole timeline: the projection's own finish, or the latest p95 beyond it.
    public static func timelineWidth(_ schedule: GanttSchedule, level: Int) -> Double {
        timelineWidth(schedule, scale: pointsPerHour(level))
    }

    public static func timelineWidth(_ schedule: GanttSchedule, scale: Double) -> Double {
        planHours(schedule) * scale + timelineMargin
    }

    /// The trailing room after the last hour, for the last bar's text.
    public static let timelineMargin = 80.0

    /// The hours the whole timeline spans: the projection's own finish, or the latest p95 or bar finish beyond it.
    public static func planHours(_ schedule: GanttSchedule) -> Double {
        max(schedule.projectFinishHours, schedule.uncertainty?.p95 ?? 0, schedule.rows.compactMap { $0.span?.finish }.max() ?? 0)
    }

    /// The scale at which the whole timeline, its trailing room included, is exactly as wide as `width`: geometry only.
    /// Nil when there is no width or no hour to fit.
    public static func fitScale(_ schedule: GanttSchedule, width: Double) -> Double? {
        let hours = planHours(schedule)
        guard width > timelineMargin + 1, hours > 0 else { return nil }
        return (width - timelineMargin) / hours
    }

    /// The width of the row-label column for a body this wide: room for keys and titles, never more than a third of
    /// the body once it is narrow, so the timeline keeps most of the width. Full titles stay in the rows' text.
    public static func labelWidth(body: Double) -> Double {
        max(220, min(400, body * 0.34))
    }
}

/// Where the finish-forecast labels go on the chart. Every label is drawn on one line below the time axis
/// labels; a label whose rectangle would touch one already placed is omitted from the chart (its fact stays in
/// the Schedule basis, the row words and Detail). Earlier labels have priority: finish, p50, p80, p95.
public enum GanttAxisLabels {
    public struct Label: Equatable, Sendable {
        public var text: String
        public var x: Double
        public var minX: Double { x }
        public var maxX: Double { x + Double(text.count) * 5.4 + 4 }
    }

    /// The y of the label line, in points from the top of the chart.
    public static let lineY = 20.0

    /// The labels that fit, for markers at the given x positions: a marker outside `0...width` has none, and the
    /// text starts 3 points right of its marker.
    public static func place(_ markers: [(x: Double, text: String)], width: Double) -> [Label] {
        var placed: [Label] = []
        for marker in markers where marker.x >= 0 && marker.x <= width {
            let label = Label(text: marker.text, x: marker.x + 3)
            if placed.allSatisfy({ label.minX >= $0.maxX + 2 || label.maxX + 2 <= $0.minX }) { placed.append(label) }
        }
        return placed
    }
}
