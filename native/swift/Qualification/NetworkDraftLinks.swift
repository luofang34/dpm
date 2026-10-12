// The network draws links being drafted as the Gantt does: the connector's corners follow one elbow rule, a drafted
// link runs from the node end it leaves to the node end it joins, it is drawn only while it is in the draft and both
// its nodes are shown, and drawing it writes nothing.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func networkDraftLinksAreDrawn() async throws {
        print("network: drafted links and the dragged link are elbow connectors between node ends; drawing them writes nothing")
        let p = { (x: Double, y: Double) in NetworkPoint(x: x, y: y) }
        check(ElbowRoute.points(from: p(0, 0), outward: 10, to: p(100, 40), inward: -10) == [p(0, 0), p(90, 0), p(90, 40), p(100, 40)],
              "finish to a later start turns once, between the ends")
        check(ElbowRoute.points(from: p(0, 0), outward: 10, to: p(50, 40), inward: 10) == [p(0, 0), p(60, 0), p(60, 40), p(60, 40), p(50, 40)],
              "finish to finish turns beyond both ends and enters from the right")
        check(ElbowRoute.points(from: p(100, 0), outward: 10, to: p(0, 40), inward: -10) == [p(100, 0), p(110, 0), p(110, 20), p(-10, 20), p(-10, 40), p(0, 40)],
              "a link running back turns between the rows")

        let database = try await newStore("network-drafts")
        let model = try await openGanttModel(.database(database))
        _ = try await modelExpect(model, "the plan to be read", 60) { $0.inventory.basis != nil && $0.freshness.current }
        let ids = await MainActor.run { Dictionary(model.snapshot.inventory.items.map { ($0.key, $0.identity) }, uniquingKeysWith: { a, _ in a }) }
        guard let d = ids["TEST-D"], let f = ids["TEST-F"] else { check(false, "the fixture is readable"); return }
        let history = try await historyCount(database)
        let drawn = await MainActor.run { () -> ([NetworkDraftLink], NetworkPoint?, NetworkPoint?) in
            model.setEditing(true)
            model.edit(.link(id: UUID().uuidString.lowercased(), from: d, fromEnd: .finish, to: f, toEnd: .finish, lagHours: 0))
            return (model.networkDraftLinks, model.networkEnd(d, .finish), model.networkEnd(f, .finish))
        }
        check(drawn.0.count == 1 && drawn.0.first?.kind == "FinishFinish" && drawn.0.first?.points.first == drawn.1 && drawn.0.first?.points.last == drawn.2,
              "a drafted FF link is drawn from TEST-D's finish edge to TEST-F's finish edge: \(drawn.0)")
        let hidden = await MainActor.run { () -> Int in model.setNetworkFilterText("TEST-D"); defer { model.setNetworkFilterText("") }; return model.networkDraftLinks.count }
        check(hidden == 0, "a drafted link with an end filtered out of the network is not drawn")
        let kept = await MainActor.run { (model.networkDraftLinks.count, model.editing.draft?.edits.count ?? 0) }
        check(kept == (1, 1), "clearing the filter draws it again; the draft kept it throughout")
        let removed = await MainActor.run { () -> Int in model.removeEdit(at: 0); return model.networkDraftLinks.count }
        check(removed == 0, "a link taken out of the draft is no longer drawn")
        let discarded = await MainActor.run { () -> Int in
            model.edit(.link(id: UUID().uuidString.lowercased(), from: d, fromEnd: .finish, to: f, toEnd: .start, lagHours: 0))
            model.discardDraft()
            return model.networkDraftLinks.count
        }
        check(discarded == 0, "a discarded draft draws no link")
        let after = try await historyCount(database)
        check(after == history, "drawing drafted links recorded nothing")
        await closeGantt(model)
    }
}
