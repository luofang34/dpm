// The Gantt's time axis in calendar dates. The projection measures elapsed hours from the instant it was
// evaluated at; adding them to that instant places every tick on the calendar, so a plan finished in 1950
// reads in years and a week of work reads in days, at any zoom. Ticks fall on calendar boundaries (midnight,
// the first of a month, a year divisible by the step) and are spaced so no two labels touch. View only.

import DPMNative
import Foundation

/// One tick of the time axis: where it falls, in elapsed hours, and its calendar label.
public struct GanttTick: Equatable, Sendable {
    public let hours: Double
    public let label: String
}

public enum GanttCalendarAxis {
    enum Unit: Equatable {
        case hours(Int), days(Int), months(Int), years(Int)

        /// The shortest this step can be, so spacing computed from it is a guarantee, not an average.
        var shortestHours: Double {
            switch self {
            case .hours(let n): return Double(n)
            case .days(let n): return Double(24 * n)
            case .months(let n): return Double(24 * 28 * n)
            case .years(let n): return Double(24 * 365 * n)
            }
        }

        /// The longest label this step draws, in characters.
        var labelCharacters: Int {
            switch self {
            case .hours: return 6 // "Jan 31" at midnight, "14:00" otherwise
            case .days: return 6
            case .months: return 8 // "Sep 1950"
            case .years: return 4
            }
        }
    }

    static let ladder: [Unit] = [
        .hours(1), .hours(3), .hours(6), .hours(12), .days(1), .days(2), .days(7), .months(1), .months(3), .months(6),
        .years(1), .years(2), .years(5), .years(10), .years(20), .years(50), .years(100), .years(500), .years(1000),
    ]

    /// Width of one caption digit or letter, and the gap kept between labels, in points.
    public static let characterWidth = 6.5
    public static let labelGap = 10.0

    /// The finest step whose labels cannot touch at `scale` points per hour.
    static func unit(scale: Double) -> Unit {
        ladder.first { $0.shortestHours * scale >= Double($0.labelCharacters) * characterWidth + labelGap } ?? .years(1000)
    }

    /// The ticks from `fromHours` to `toHours` after `origin`, at `scale` points per hour.
    public static func ticks(origin: Instant, scale: Double, fromHours: Double, toHours: Double,
                             timeZone: TimeZone = .current, locale: Locale = .current) -> [GanttTick] {
        guard scale > 0, toHours >= fromHours, fromHours.isFinite, toHours.isFinite else { return [] }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let base = Date(timeIntervalSince1970: Double(origin.seconds) + Double(origin.nanos) / 1e9)
        let step = unit(scale: scale)
        guard var date = floor(base.addingTimeInterval(fromHours * 3600), to: step, calendar) else { return [] }
        let end = base.addingTimeInterval(toHours * 3600)
        var ticks: [GanttTick] = []
        while date <= end, ticks.count < 4000 {
            let hours = date.timeIntervalSince(base) / 3600
            if hours >= fromHours - 1e-6 { ticks.append(GanttTick(hours: hours, label: label(date, step, calendar, locale))) }
            guard let next = advance(date, by: step, calendar), next > date else { break }
            date = next
        }
        return ticks
    }

    /// The year (and, at hour steps, the month) at `hours`, for steps whose labels omit it: drawn at the axis's
    /// left edge so a day or an hour is never read without its year. Nil when the labels carry the year.
    public static func context(origin: Instant, scale: Double, atHours hours: Double,
                               timeZone: TimeZone = .current, locale: Locale = .current) -> String? {
        let step = unit(scale: scale)
        guard scale > 0, hours.isFinite else { return nil }
        let template: String
        switch step {
        case .hours: template = "MMMyyyy"
        case .days: template = "yyyy"
        case .months, .years: return nil
        }
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = timeZone
        let formatter = DateFormatter()
        formatter.calendar = calendar
        formatter.timeZone = timeZone
        formatter.locale = locale
        formatter.setLocalizedDateFormatFromTemplate(template)
        let base = Date(timeIntervalSince1970: Double(origin.seconds) + Double(origin.nanos) / 1e9)
        return formatter.string(from: base.addingTimeInterval(hours * 3600))
    }

    static func floor(_ date: Date, to step: Unit, _ calendar: Calendar) -> Date? {
        var parts = calendar.dateComponents([.year, .month, .day, .hour], from: date)
        switch step {
        case .hours(let n): parts.hour = (parts.hour ?? 0) / n * n
        case .days(let n):
            parts.hour = 0
            if n == 7, let day = calendar.dateInterval(of: .weekOfYear, for: date)?.start { return day }
            parts.day = ((parts.day ?? 1) - 1) / n * n + 1
        case .months(let n):
            parts.hour = 0
            parts.day = 1
            parts.month = ((parts.month ?? 1) - 1) / n * n + 1
        case .years(let n):
            parts.hour = 0
            parts.day = 1
            parts.month = 1
            let year = parts.year ?? 1
            parts.year = Int((Double(year) / Double(n)).rounded(.down)) * n
        }
        return calendar.date(from: parts)
    }

    static func advance(_ date: Date, by step: Unit, _ calendar: Calendar) -> Date? {
        switch step {
        case .hours(let n): return calendar.date(byAdding: .hour, value: n, to: date)
        case .days(let n): return calendar.date(byAdding: .day, value: n, to: date)
        case .months(let n): return calendar.date(byAdding: .month, value: n, to: date)
        case .years(let n): return calendar.date(byAdding: .year, value: n, to: date)
        }
    }

    static func label(_ date: Date, _ step: Unit, _ calendar: Calendar, _ locale: Locale) -> String {
        let formatter = DateFormatter()
        formatter.calendar = calendar
        formatter.timeZone = calendar.timeZone
        formatter.locale = locale
        switch step {
        case .hours: formatter.setLocalizedDateFormatFromTemplate(calendar.component(.hour, from: date) == 0 ? "MMMd" : "HH:mm")
        case .days: formatter.setLocalizedDateFormatFromTemplate("MMMd")
        case .months: formatter.setLocalizedDateFormatFromTemplate("MMMyyyy")
        case .years: formatter.setLocalizedDateFormatFromTemplate("yyyy")
        }
        return formatter.string(from: date)
    }

    /// A calendar date for elapsed `hours` after `origin`, as the strip and inspector say it.
    public static func date(origin: Instant, hours: Double, timeZone: TimeZone = .current, locale: Locale = .current) -> String {
        let formatter = DateFormatter()
        formatter.timeZone = timeZone
        formatter.locale = locale
        formatter.dateStyle = .medium
        formatter.timeStyle = .none
        let base = Date(timeIntervalSince1970: Double(origin.seconds) + Double(origin.nanos) / 1e9)
        return formatter.string(from: base.addingTimeInterval(hours * 3600))
    }
}
