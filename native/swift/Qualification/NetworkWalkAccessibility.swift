// NETWORK-WALKED-AX-01: after each walk key the inspector's walked text must be the current step's, never the
// previous one. This follows the measured route on the real model (TEST-B selected, a real `p` to TEST-A, then a
// real `s`), with real `NSEvent` keys through the window's handler, and checks after every step that the walked
// text the inspector shows (and the region's value ends with) names the node the cursor is on now. It is a
// synchronization check of the state the view reads; the external AXStaticText of the actual window is the UI
// verifier's strict walk.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func networkWalkedTextFollowsEachStep() async throws {
        print("network: after each real walk key the walked text is the current step's (p to TEST-A, then s, s, p), never the previous step's; nothing is written")
        let database = try await newStore("network-walk-text", plan: try requirePlan(tools.networkPlan, "--network-plan"))
        let model = try await openGanttModel(.database(database))
        let first = await MainActor.run { model.snapshot }
        guard let selected = identity(of: "TEST-B", in: first) else { throw SuiteError(description: "TEST-B has no identity") }
        await MainActor.run { model.select(.work(selected)) }
        _ = try await modelExpect(model, "TEST-B's detail", 30) { $0.detail?.subject == .work(selected) && $0.detail?.loading == false }
        let before = try await readOnlyRecord(database)
        await MainActor.run { model.show(.network) }

        struct Step { var cursor: String?; var focus: String?; var expectedFocus: String?; var walked: String?; var expectedTail: String? }
        func read() async -> Step {
            await MainActor.run {
                let node = model.networkFocusedNode
                return Step(cursor: node?.key, focus: model.focusTarget, expectedFocus: node.map { ObserverModel.element(forNode: $0.key) }, walked: model.network.walked, expectedTail: node.map { "\($0.key), \($0.kindText), \($0.title)" })
            }
        }
        let entered = await read()
        check(entered.cursor == "TEST-B" && entered.walked == nil, "entering the network with TEST-B selected puts the cursor on it with no walk yet: \(entered.cursor ?? "none")")

        var previous = entered.walked
        var origin = "TEST-B"
        var cursor = entered.cursor
        var lastKey: String?
        for (key, name) in [("p", "predecessor"), ("s", "successor"), ("s", "successor"), ("p", "predecessor")] {
            // A repeated key walks on from the same origin; a different key walks from the node the cursor is on.
            if key != lastKey { origin = cursor ?? origin }
            lastKey = key
            let outcome = await MainActor.run { self.pressNetwork(model, key) }
            let now = await read()
            let walked = now.walked ?? ""
            check(outcome != .ignored && now.cursor != nil && now.focus == now.expectedFocus,
                  "a real \(key) moved the cursor and the element to focus together: cursor \(now.cursor ?? "none"), focus \(now.focus ?? "none")")
            check(walked.hasPrefix("\(name) ") && walked.contains(" of \(origin): ") && now.expectedTail.map { walked.hasSuffix($0) } == true,
                  "after \(key) the walked text names this step's origin \(origin) and the node the cursor is on now: \(walked)")
            check(walked != previous, "after \(key) the walked text is not the previous step's: \(previous ?? "none") -> \(walked)")
            previous = walked
            cursor = now.cursor
        }

        let after = try await readOnlyRecord(database)
        check(after == before, "the walk wrote nothing: before \(before), after \(after)")
        await closeGantt(model)
    }
}
