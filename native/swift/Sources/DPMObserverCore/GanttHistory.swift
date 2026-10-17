// Recorded execution before the clock reading: the `actual` times the schedule projection reports beside the
// remaining work, in elapsed hours that are negative in the past. The timeline reaches back to the earliest of
// them, so a fit and a pan show what happened as well as what remains. View geometry only; no hour changes.

import DPMNative
import Foundation

public struct GanttActual: Equatable, Sendable {
    /// Recorded start in elapsed hours; nil when no start time was recorded.
    public let start: Double?
    /// Recorded finish (verification, or a milestone's or package's completion); nil while outstanding.
    public let finish: Double?

    init?(_ json: JSON) {
        start = json["start_hours"].double
        finish = json["finish_hours"].double
        if start == nil, finish == nil { return nil }
    }

    /// The recorded stretch: from the start, or the finish alone, up to the finish or the clock reading.
    public var hours: (from: Double, to: Double)? {
        guard let from = start ?? finish else { return nil }
        return (from, finish ?? max(from, 0))
    }
}

extension GanttLayout {
    /// Where the timeline begins: the earliest recorded hour, or the clock reading (0) when work remains and nothing
    /// was recorded before it.
    public static func historyHours(_ schedule: GanttSchedule) -> Double {
        let recorded = schedule.rows.compactMap { $0.actual?.hours?.from }
        guard hasRemainingWork(schedule) else { return recorded.min() ?? 0 }
        return recorded.reduce(0, min)
    }

    /// Where the timeline ends: the projection's own finish, or the latest p95 or bar finish beyond it; for a plan
    /// with nothing outstanding, its last recorded finish, so the clock reading after it is not part of the plan.
    public static func planHours(_ schedule: GanttSchedule) -> Double {
        guard hasRemainingWork(schedule) else { return recordedFinishHours(schedule) ?? 0 }
        return max(schedule.projectFinishHours, schedule.uncertainty?.p95 ?? 0, schedule.rows.compactMap { $0.span?.finish }.max() ?? 0)
    }

    /// Whether any applicable row is still outstanding: only then does the timeline reach the clock reading.
    public static func hasRemainingWork(_ schedule: GanttSchedule) -> Bool {
        schedule.rows.contains { $0.span != nil && $0.actual?.finish == nil }
    }

    /// The last recorded finish, when anything finished.
    public static func recordedFinishHours(_ schedule: GanttSchedule) -> Double? {
        schedule.rows.compactMap { $0.actual?.finish }.max()
    }

    /// The pan that puts the timeline's beginning at the left edge, at a scale.
    public static func firstX(_ schedule: GanttSchedule, scale: Double) -> Double {
        historyHours(schedule) * scale
    }

    /// Words for a row's recorded execution, for the row's spoken value.
    public static func historyWords(_ actual: GanttActual?) -> String? {
        guard let actual else { return nil }
        let start = actual.start.map { "started at \(GanttWords.hours($0))" }
        let finish = actual.finish.map { "finished at \(GanttWords.hours($0))" } ?? (actual.start == nil ? nil : "not finished")
        return "recorded: " + [start, finish].compactMap { $0 }.joined(separator: ", ") + " elapsed"
    }
}
