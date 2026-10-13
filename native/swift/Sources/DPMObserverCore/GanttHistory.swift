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
    /// The earliest recorded hour, or 0 when nothing was recorded before the clock reading: where the timeline begins.
    public static func historyHours(_ schedule: GanttSchedule) -> Double {
        schedule.rows.compactMap { $0.actual?.hours?.from }.reduce(0, min)
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
