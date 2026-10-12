// Linking by dragging: a press names the bar's end, a drop names the target's end and so the relation kind, a drop on a
// work package is refused in the view, and a whole-bar drag moves nothing but offers a lag on the link that drives the
// bar. Duplicates and cycles are the application's to refuse, with its reason and the revision unchanged; a lag edit
// is reviewed and applied as one ApplyChange like any other edit.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func ganttGesturesProposeAndTheApplicationJudges() async throws {
        print("edit: drag gestures name ends, kinds, refusals and lags; duplicates and cycles are refused by the application")
        pureGestureDecisions()

        let database = try await newStore("edit-gestures")
        let model = try await openGanttModel(.database(database))
        _ = try await modelExpect(model, "the plan to be read for editing", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let (engine, basis, ids, relations) = await MainActor.run { () -> (ObserverEngine?, PlanBasis?, [String: String], [GanttRelation]) in
            (model.engine, model.snapshot.inventory.basis, Dictionary(model.snapshot.inventory.items.map { ($0.key, $0.identity) }, uniquingKeysWith: { a, _ in a }),
             model.snapshot.inventory.relations)
        }
        guard let engine, let basis, let a = ids["TEST-A"], let b = ids["TEST-B"],
              let existing = relations.first(where: { $0.predecessor == a && $0.successor == b && $0.kind == "FinishStart" }) else {
            check(false, "the fixture has its FS link from TEST-A to TEST-B"); return
        }
        let history = try await historyCount(database)

        // The same ordered pair and kind again is a duplicate; the application says so, and nothing is recorded.
        var duplicate = PlanDraft(basis: basis)
        duplicate.add(.link(id: UUID().uuidString.lowercased(), from: a, fromEnd: .finish, to: b, toEnd: .start, lagHours: 0))
        if case .failure(let refusal) = await engine.propose(duplicate.candidate(), basis: duplicate.basis) {
            check(refusal.fromApplication && refusal.message.contains("duplicate"), "a duplicate link is refused with the application's reason: \(refusal.message)")
        } else {
            check(false, "a duplicate link is refused")
        }
        // A link back from B to A closes a cycle; the application names it.
        var cycle = PlanDraft(basis: basis)
        cycle.add(.link(id: UUID().uuidString.lowercased(), from: b, fromEnd: .finish, to: a, toEnd: .start, lagHours: 0))
        if case .failure(let refusal) = await engine.propose(cycle.candidate(), basis: cycle.basis) {
            check(refusal.fromApplication && refusal.message.contains("cycle"), "a cycle is refused with the application's reason: \(refusal.message)")
        } else {
            check(false, "a cycle is refused")
        }
        let recorded = try await historyCount(database)
        let revision = try await cliJSON(database, ["revision"])["revision"].uint
        check(recorded == history && revision == basis.revision, "the refusals recorded nothing and left the revision")

        // The lag a whole-bar drag offers is reviewed and applied as one change of that link's lag alone.
        let driver = GanttGesture.Driver(relation: existing, eventX: 100)
        guard case .success(let lag) = GanttGesture.lag(drivers: [driver], dx: 40, pointsPerHour: 10) else { check(false, "a 40-point drag offers a lag"); return }
        check(lag == .lag(dependency: existing.id, hours: existing.lagHours + 4), "a 40-point drag at 10 points an hour offers 4 h more lag: \(lag)")
        await MainActor.run {
            model.setEditing(true)
            model.setEditActor("human:planner")
            model.edit(lag)
            model.reviewDraft()
        }
        try await waitEdit(model, "the lag to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        let reviewed = await MainActor.run { model.editing.reviewed?.changeList ?? [] }
        let fields = reviewed.flatMap { ($0["fields"].array ?? []).compactMap(\.string) }
        check(reviewed.count == 1 && reviewed.first?["collection"].string == "dependencies" && fields == ["lag_hours"],
              "the review shows exactly the link's lag changing: \(fields)")
        await MainActor.run { model.setEditReason("Wait four hours after A"); model.applyDraft() }
        try await waitEdit(model, "the lag to be applied") { if case .applied = $0.phase { return true } else { return false } }
        let entries = try await cliJSON(database, ["history"])["data"]["entries"].array ?? []
        let applied = entries.last?["operation"]["command"]["ApplyChange"]
        let edges = try await cliJSON(database, ["export"])["data"]["dependencies"].array ?? []
        let lagNow = edges.first { $0["id"].string == existing.id }.flatMap { hours($0["lag_hours"]) }
        check(entries.count == history + 1 && applied?["reason"].string == "Wait four hours after A" && lagNow == existing.lagHours + 4,
              "one ApplyChange recorded the reviewed lag with its reason: lag now \(lagNow ?? -1) h")
        await closeGantt(model)
    }

    private func hours(_ json: JSON) -> Double? {
        switch json {
        case .number(let value): return value
        case .integer(let value): return Double(value)
        case .unsigned(let value): return Double(value)
        default: return nil
        }
    }

    /// The gesture decisions, without a view or a helper.
    private func pureGestureDecisions() {
        func bar(_ kind: String = "Task", status: String = "Planned", left: Double = 100, right: Double = 200, progress: Int = 0) -> GestureBar {
            GestureBar(id: "X", name: "TEST-X", kind: kind, status: status, left: left, right: right, hasEstimate: true, reportedProgress: progress)
        }
        check(GanttGesture.press(bar(), x: 101, option: false) == .link(.start) && GanttGesture.press(bar(), x: 199, option: false) == .link(.finish),
              "a press at either end starts a link from that end")
        check(GanttGesture.press(bar(), x: 150, option: false) == .shift, "a press on the body starts a whole-bar drag, which moves nothing")
        check(GanttGesture.press(bar(), x: 199, option: true) == .resize, "Option on the finish of an unstarted task scales its estimate")
        if case .refused? = GanttGesture.press(bar(status: "InProgress"), x: 199, option: true) {
            check(true, "Option on a started task's finish is refused in the view")
        } else {
            check(false, "Option on a started task's finish is refused in the view")
        }
        check(GanttGesture.press(bar(status: "InProgress", progress: 50), x: 150, option: false) == .progress(edge: 150), "a started task's share edge is its progress handle")
        check(GanttGesture.press(bar(left: 100, right: 106), x: 102, option: false) == .link(.start)
              && GanttGesture.press(bar(left: 100, right: 106), x: 104, option: false) == .link(.finish), "on a narrow bar the half pressed names the end")
        check(GanttGesture.press(bar(), x: 300, option: false) == nil, "a press off the bar starts nothing")
        if case .refused(let words)? = GanttGesture.press(bar("WorkPackage"), x: 150, option: false) {
            check(words.contains("work package"), "a press on a work package is refused with the reason: \(words)")
        } else {
            check(false, "a press on a work package is refused")
        }
        let target = bar()
        check(GanttGesture.drop(on: target, x: 120, from: "Y") == .join(id: "X", end: .start) && GanttGesture.drop(on: target, x: 180, from: "Y") == .join(id: "X", end: .finish),
              "a drop on a bar's left half joins its start, on its right half its finish")
        let kinds = [(PlanEnd.finish, PlanEnd.start), (.start, .start), (.finish, .finish), (.start, .finish)].map { PlanEdit.kind(from: $0.0, to: $0.1) }
        check(kinds == ["FinishStart", "StartStart", "FinishFinish", "StartFinish"], "the ends joined name FS, SS, FF and SF: \(kinds)")
        if case .refused(let words) = GanttGesture.drop(on: bar("WorkPackage"), x: 150, from: "Y") {
            check(words.contains("work package"), "a drop on a work package is refused in the view and proposes nothing: \(words)")
        } else {
            check(false, "a drop on a work package is refused in the view")
        }
        check(GanttGesture.drop(on: target, x: 150, from: "X") == .nothing && GanttGesture.drop(on: nil, x: 0, from: "X") == .nothing,
              "a drop on the bar itself or off every bar proposes nothing")
        let early = GanttRelation(id: "early", predecessor: "P", successor: "X", kind: "FinishStart", lagHours: 0)
        let late = GanttRelation(id: "late", predecessor: "Q", successor: "X", kind: "StartStart", lagHours: 2)
        let drivers = [GanttGesture.Driver(relation: early, eventX: 50), GanttGesture.Driver(relation: late, eventX: 90)]
        check(GanttGesture.lag(drivers: drivers, dx: -15, pointsPerHour: 10) == .success(.lag(dependency: "late", hours: 0.5)),
              "a whole-bar drag offers a lag on the link that drives the bar, in quarter hours, leads included")
        check(GanttGesture.lag(drivers: [], dx: 40, pointsPerHour: 10) == .failure(.noDriver), "a bar with no link into it offers no lag and says why")
        check(GanttGesture.lag(drivers: drivers, dx: 1, pointsPerHour: 10) == .failure(.unmoved), "a drag of less than an eighth of an hour offers nothing")
    }
}
