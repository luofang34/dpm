// The Gantt's recorded history, qualified through the real helper and model: work recorded long after it
// happened carries the projection's `actual` hours, the timeline reaches back to the earliest of them, a fit
// shows that history beside the remaining work, and a pan stops at its beginning rather than at hour 0.

import DPMNative
import DPMObserverCore
import Foundation

/// A number the query wrote, in any of the JSON forms a number takes.
private func hours(_ json: DPMNative.JSON) -> Double? {
    switch json {
    case .number(let value): return value
    case .integer(let value): return Double(value)
    case .unsigned(let value): return Double(value)
    default: return nil
    }
}

extension Context {
    func ganttHistoryIsReachable() async throws {
        print("gantt: recorded work before the clock reading is drawn, fitted and reachable by panning")
        let database = try await newStore("gantt-history")
        for (words, actor) in [(["decide", "TEST-GATE", "Proceed", "--at", "2026-01-04T00:00:00Z"], "human:lead"),
                               (["claim", "TEST-A", "--at", "2026-01-05T00:00:00Z"], "agent:worker"),
                               (["start", "TEST-A", "--at", "2026-01-05T01:00:00Z"], "agent:worker"),
                               (["submit", "TEST-A", "--at", "2026-01-06T00:00:00Z"], "agent:worker"),
                               (["verify", "TEST-A", "--at", "2026-01-07T00:00:00Z"], "human:lead")] {
            _ = try await cliJSON(database, words + ["--actor", actor])
        }
        let work = try await cliJSON(database, ["--clock", tools.clock, "schedule"])["data"]["work"].array ?? []
        let recorded: DPMNative.JSON = work.first { $0["key"].string == "TEST-A" }?["actual"] ?? .null
        let model = try await openGanttModel(.database(database))
        await MainActor.run { model.setViewport(rows: 20, width: 900) }
        let (found, outline, inventory) = await MainActor.run { (model.snapshot.gantt, model.outline, model.snapshot.inventory) }
        guard let schedule = found, let row = schedule.rows.first(where: { $0.key == "TEST-A" }) else { throw SuiteError(description: "the Gantt read no TEST-A row") }
        let start = hours(recorded["start_hours"]), finish = hours(recorded["finish_hours"])
        check(start != nil && start ?? 0 < 0 && row.actual?.start == start && row.actual?.finish == finish,
              "the row's recorded start and finish are the query's, before the clock reading: \(String(describing: row.actual)) vs \(start ?? .nan), \(finish ?? .nan)")
        let history = GanttLayout.historyHours(schedule)
        check(history == start, "the timeline begins at the earliest recorded hour \(history)")
        await MainActor.run { model.fitTimeline() }
        let fitted = await MainActor.run { model.gantt }
        let span = (GanttLayout.planHours(schedule) - history) * fitted.scale + GanttLayout.timelineMargin
        check(abs(fitted.panX - history * fitted.scale) < 1e-6 && abs(span - fitted.viewportWidth) < 1e-6,
              "a fit starts at the recorded history and spans it with the remaining work: pan \(fitted.panX), width \(span) of \(fitted.viewportWidth)")
        await MainActor.run {
            _ = press(model, "+")
            model.pan(dx: -1e9, dy: 0)
        }
        let leftmost = await MainActor.run { model.gantt }
        check(abs(leftmost.panX - history * leftmost.scale) < 1e-6, "a pan to the left stops at the recorded beginning, not hour 0: \(leftmost.panX)")
        if let item = outline.first(where: { $0.row.key == "TEST-A" }) {
            let words = GanttWords.row(item, schedule: schedule, inventory: inventory, selected: nil).value
            check(words.contains("recorded: started at"), "the row says its recorded times in words: \(words)")
        } else {
            check(false, "TEST-A is in the outline")
        }
        await closeGantt(model)
    }
}
