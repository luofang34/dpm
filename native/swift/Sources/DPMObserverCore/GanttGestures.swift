// What a pointer gesture on the Gantt timeline proposes, decided from the bar pressed and where, with no view and no
// model: which end a press takes, what a drop joins or refuses, and the lag a whole-bar drag offers instead of moving
// the bar, since its dates are derived. The application still validates every proposal; nothing here judges cycles,
// duplicates or protection.

import Foundation

/// One bar as a gesture sees it: what it is and where it is drawn, in timeline points.
public struct GestureBar: Equatable, Sendable {
    public let id: String
    public let name: String
    /// `Task`, `Milestone` or `WorkPackage`, as the plan writes it.
    public let kind: String
    public let status: String
    public let left: Double
    public let right: Double
    public let hasEstimate: Bool
    public let reportedProgress: Int

    public init(id: String, name: String, kind: String, status: String, left: Double, right: Double, hasEstimate: Bool, reportedProgress: Int) {
        self.id = id
        self.name = name
        self.kind = kind
        self.status = status
        self.left = left
        self.right = right
        self.hasEstimate = hasEstimate
        self.reportedProgress = reportedProgress
    }

    public var isPackage: Bool { kind == "WorkPackage" }
    public var unstarted: Bool { ["Proposed", "Planned", "Claimed"].contains(status) }
}

public enum GanttGesture {
    /// How near an end, in points, a press takes that end.
    public static let handle = 7.0

    /// What a press on a bar starts.
    public enum Press: Equatable, Sendable {
        /// A link from this end of the bar.
        case link(PlanEnd)
        /// The finish dragged with Option: the bar's own estimate scaled.
        case resize
        /// The edge of a started task's reported share, at `edge`.
        case progress(edge: Double)
        /// The whole bar: it does not move; a drop offers a lag on the link that drives it.
        case shift
        /// Nothing starts, for the reason given.
        case refused(String)
    }

    /// What a press at `x` on `bar` starts, or nil when it is off the bar.
    public static func press(_ bar: GestureBar, x: Double, option: Bool) -> Press? {
        if bar.isPackage {
            return .refused("A work package's span comes from its tasks: link or change a task, or edit \(bar.name) in the inspector.")
        }
        guard x >= bar.left - handle, x <= bar.right + handle else { return nil }
        // On a bar too narrow for two handles, the side of its middle pressed names the end.
        let narrow = bar.right - bar.left < 2 * handle
        let atStart = narrow ? x < (bar.left + bar.right) / 2 : abs(x - bar.left) <= handle
        let atFinish = narrow ? !atStart : abs(x - bar.right) <= handle
        if atFinish, option {
            guard bar.kind == "Task", bar.unstarted, bar.hasEstimate, bar.right - bar.left >= 12 else {
                return .refused("Only a task not yet started, with an estimate and a bar wide enough to drag, can have its estimate dragged; use the inspector.")
            }
            return .resize
        }
        if atStart { return .link(.start) }
        if atFinish { return .link(.finish) }
        if bar.status == "InProgress" {
            // Progress moves only from its own handle, the edge of the reported share.
            let edge = bar.left + (bar.right - bar.left) * Double(bar.reportedProgress) / 100
            if abs(x - edge) <= handle { return .progress(edge: edge) }
        }
        return .shift
    }

    /// What a link dragged from `from` and released over `bar` at `x` does.
    public enum Drop: Equatable, Sendable {
        case join(id: String, end: PlanEnd)
        case refused(String)
        case nothing
    }

    public static func drop(on bar: GestureBar?, x: Double, from: String) -> Drop {
        guard let bar, bar.id != from else { return .nothing }
        if bar.isPackage { return .refused("A link joins tasks and milestones; \(bar.name) is a work package.") }
        return .join(id: bar.id, end: x < (bar.left + bar.right) / 2 ? .start : .finish)
    }

    /// A link into the dragged bar and where its predecessor's event is drawn, the event the bar waits for.
    public struct Driver: Equatable, Sendable {
        public let relation: GanttRelation
        public let eventX: Double
        public init(relation: GanttRelation, eventX: Double) {
            self.relation = relation
            self.eventX = eventX
        }
    }

    /// What dropping a whole bar dragged `dx` points offers: a new lag on the link whose predecessor event is latest, the
    /// one that drives the bar, moved by the drag in quarter hours; or the reason there is none.
    public static func lag(drivers: [Driver], dx: Double, pointsPerHour: Double) -> Result<PlanEdit, ShiftRefusal> {
        guard let driver = drivers.max(by: { $0.eventX < $1.eventX }) else { return .failure(.noDriver) }
        let delta = quarter(dx / max(pointsPerHour, .leastNonzeroMagnitude))
        guard delta != 0 else { return .failure(.unmoved) }
        return .success(.lag(dependency: driver.relation.id, hours: quarter(driver.relation.lagHours + delta)))
    }

    /// Why a whole-bar drag offers no lag.
    public enum ShiftRefusal: Error, Equatable, Sendable {
        case noDriver
        case unmoved

        public var words: String {
            switch self {
            case .noDriver: return "Dates are derived, so a bar does not move: link it to a predecessor, then drag it to offer a lag."
            case .unmoved: return "Dates are derived, so a bar does not move; drag it along the timeline to offer a lag on the link that drives it."
            }
        }
    }

    /// How much longer the bar was dragged, as a ratio of lengths rounded to a twentieth, at least a tenth.
    public static func ratio(left: Double, right: Double, to x: Double) -> Double {
        let ratio = max(x - left, 1) / max(right - left, 1)
        return max(0.1, (ratio * 20).rounded() / 20)
    }

    /// The share of the bar left of `x`, in steps of 5%.
    public static func percent(left: Double, right: Double, at x: Double) -> Int {
        let share = (x - left) / max(right - left, 1)
        return Int((min(1, max(0, share)) * 20).rounded()) * 5
    }

    public static func quarter(_ hours: Double) -> Double { (hours * 4).rounded() / 4 }
}
