// A plan finished long before the clock reading, as a historical study is: every task of the synthetic
// recorded plan is claimed, started, submitted and verified in 1950. The Gantt fits what happened, not the
// decades between the last finish and the clock reading.

import DPMObserverCore
import Foundation

extension Context {
    /// The recorded plan, every task finished in January 1950 on the CLI: REC-A, then REC-B and REC-C after
    /// it, REC-D after both (REC-B finishes last), and REC-E started with REC-A.
    func recordedStore(_ name: String) async throws -> URL {
        let database = try await newStore(name, plan: tools.recordedPlan)
        for (key, start, finish) in [("REC-A", "1950-01-01", "1950-01-10"), ("REC-E", "1950-01-03", "1950-01-05"),
                                     ("REC-B", "1950-01-10", "1950-01-20"), ("REC-C", "1950-01-12", "1950-01-15"),
                                     ("REC-D", "1950-01-20", "1950-02-01")] {
            for (words, at, actor) in [("claim", start, "agent:worker"), ("start", start, "agent:worker"),
                                       ("submit", finish, "agent:worker"), ("verify", finish, "human:lead")] {
                _ = try await cliJSON(database, [words, key, "--at", "\(at)T00:00:00Z", "--actor", actor])
            }
        }
        return database
    }

    func ganttRecordedPlanIsFitted() async throws {
        print("gantt: a plan finished long ago is fitted to its recorded history, not stretched to the clock reading")
        let database = try await recordedStore("gantt-recorded")
        let model = try await openGanttModel(.database(database))
        await MainActor.run { model.setViewport(rows: 20, width: 900) }
        guard let schedule = await MainActor.run(body: { model.snapshot.gantt }) else {
            throw SuiteError(description: "the Gantt read no schedule")
        }
        let first = schedule.rows.compactMap { $0.actual?.start }.min() ?? .nan
        let last = schedule.rows.compactMap { $0.actual?.finish }.max() ?? .nan
        check(!GanttLayout.hasRemainingWork(schedule), "nothing remains in the finished plan")
        check(GanttLayout.historyHours(schedule) == first && GanttLayout.planHours(schedule) == last && last < 0,
              "the timeline runs from the first recorded start to the last recorded finish: \(GanttLayout.historyHours(schedule))…\(GanttLayout.planHours(schedule)) vs \(first)…\(last)")
        await MainActor.run { model.fitTimeline() }
        let fitted = await MainActor.run { model.gantt }
        let span = (last - first) * fitted.scale + GanttLayout.timelineMargin
        check(abs(fitted.panX - first * fitted.scale) < 1e-6 && abs(span - fitted.viewportWidth) < 1e-6,
              "a fit shows exactly the recorded history: pan \(fitted.panX), width \(span) of \(fitted.viewportWidth)")
        check(-fitted.panX > fitted.viewportWidth, "the clock reading lies beyond the fitted view: \(-fitted.panX) points right")
        calendarAxis(schedule, scale: fitted.scale, from: first, to: last)
        await closeGantt(model)
    }

    /// The axis reads in calendar dates at the fitted scale and at one that spans decades, its labels never touch,
    /// and a day is never shown without its year.
    func calendarAxis(_ schedule: GanttSchedule, scale: Double, from: Double, to: Double) {
        let utc = TimeZone(identifier: "UTC") ?? .current, posix = Locale(identifier: "en_US_POSIX")
        let ticks = GanttCalendarAxis.ticks(origin: schedule.evaluatedAt, scale: scale, fromHours: from, toHours: to, timeZone: utc, locale: posix)
        check(ticks.first?.label.hasPrefix("Jan") == true && ticks.allSatisfy { !$0.label.hasSuffix(" h") },
              "the fitted month is labelled by date: \(ticks.map(\.label))")
        check(GanttCalendarAxis.context(origin: schedule.evaluatedAt, scale: scale, atHours: from, timeZone: utc, locale: posix) == "1950",
              "day labels are read with their year at the left edge")
        let decades = 900.0 / (24 * 365 * 18)
        let years = GanttCalendarAxis.ticks(origin: schedule.evaluatedAt, scale: decades, fromHours: from, toHours: from + 24 * 365 * 18, timeZone: utc, locale: posix)
        check(years.first?.label == "1950" && years.count >= 9 && years.allSatisfy { $0.label.count == 4 },
              "eighteen years across 900 points read as years: \(years.map(\.label))")
        for (name, drawn, at) in [("fitted", ticks, scale), ("decades", years, decades)] {
            let touching = zip(drawn, drawn.dropFirst()).filter { pair in
                (pair.1.hours - pair.0.hours) * at < Double(pair.0.label.count) * GanttCalendarAxis.characterWidth + GanttCalendarAxis.labelGap
            }
            check(drawn.count > 1 && touching.isEmpty, "no two \(name) axis labels touch: \(touching.map { ($0.0.label, $0.1.label) })")
        }
    }
}
