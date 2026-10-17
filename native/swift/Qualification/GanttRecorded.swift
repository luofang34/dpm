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
        await closeGantt(model)
    }
}
