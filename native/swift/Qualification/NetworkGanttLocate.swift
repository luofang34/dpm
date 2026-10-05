// The Gantt's Locate selected, qualified on the real model against a disposable store with the FX-J1 fixture, through
// the model's own operations only (select, Collapse all, the filter field's text, Locate): a selected row hidden by
// collapsed packages is revealed by expanding exactly its collapsed ancestors and put under the cursor in view; a
// selected row the filter hides is not panned to, the note says why, and the selection, filter, cursor and view stay.
// Nothing here writes: the read-only record is compared before and after.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func ganttLocateRevealsOnlyAncestors() async throws {
        print("gantt: Locate selected after Collapse all expands only the selected row's collapsed ancestors; a filtered-out selection is kept and explained, never panned to")
        let database = try await newStore("gantt-locate", plan: try requirePlan(tools.networkPlan, "--network-plan"))
        let model = try await openGanttModel(.database(database), page: .gantt)
        _ = try await modelExpect(model, "the schedule to be read", 60) { $0.gantt != nil }
        let before = try await readOnlyRecord(database)
        let revealed = await MainActor.run { () -> (nested: Bool, hiddenAfterCollapse: Bool, ancestors: Set<String>, collapsedBefore: Set<String>, collapsedAfter: Set<String>, focus: String?, inView: Bool, selected: Bool) in
            model.setViewport(rows: 6, width: 700)
            guard let schedule = model.snapshot.gantt, let b = schedule.rows.first(where: { $0.key == "TEST-B" }) else {
                return (false, false, [], [], [], nil, false, false)
            }
            var ancestors: Set<String> = []
            var up = b.parent
            while let id = up, !ancestors.contains(id) { ancestors.insert(id); up = schedule.row(id)?.parent }
            model.focusRow(b.id)
            model.perform(.collapseAll)
            let collapsedBefore = model.gantt.collapsed
            let hidden = !model.outline.contains { $0.id == b.id }
            model.locateSelected()
            let state = model.gantt
            let at = model.outline.firstIndex { $0.id == b.id }
            let top = Double(at ?? -1) * GanttLayout.rowHeight
            let inView = at != nil && top >= state.panY && top + GanttLayout.rowHeight <= state.panY + Double(state.viewportRows) * GanttLayout.rowHeight
            return (b.parent != nil, hidden, ancestors, collapsedBefore, state.collapsed, model.focusedRow?.row.key, inView, model.selection == .work(b.id))
        }
        check(revealed.nested && revealed.hiddenAfterCollapse, "TEST-B is nested and Collapse all hides its row (collapsed \(revealed.collapsedBefore.count))")
        check(revealed.collapsedAfter == revealed.collapsedBefore.subtracting(revealed.ancestors) && revealed.collapsedBefore.intersection(revealed.ancestors).count > 0,
              "Locate expanded exactly TEST-B's collapsed ancestors: \(revealed.collapsedBefore.count) collapsed before, \(revealed.collapsedAfter.count) after, \(revealed.ancestors.count) ancestors")
        check(revealed.focus == "TEST-B" && revealed.inView && revealed.selected, "TEST-B is under the cursor (\(revealed.focus ?? "none")), inside the rows in view (\(revealed.inView)) and still selected")
        let filtered = await MainActor.run { () -> (kept: Bool, note: String?, same: Bool, filter: String) in
            guard case .work(let b)? = model.selection else { return (false, nil, false, "") }
            model.setFilterText("TEST-D")
            let hidden = !model.outline.contains { $0.id == b }
            let view = model.gantt
            model.locateSelected()
            var after = model.gantt
            let note = after.locateNote
            after.locateNote = nil
            return (hidden && model.selection == .work(b), note, after == view, model.gantt.filter.text)
        }
        check(filtered.kept && filtered.same && filtered.filter == "TEST-D", "Locate with TEST-B filtered out keeps the selection, the filter and the whole view (cursor, pan, zoom, collapse)")
        check((filtered.note?.contains("TEST-B") ?? false) && (filtered.note?.contains("filter") ?? false), "and says why it cannot show it: \(filtered.note ?? "no note")")
        let cleared = await MainActor.run { () -> Bool in
            model.perform(.clearFilter)
            return model.gantt.locateNote == nil
        }
        check(cleared, "the next view operation (Clear filter) removes the note")
        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }
}
