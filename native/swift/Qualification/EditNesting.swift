// Estimates and nesting edited from the views: an Option-drag of a bar's finish scales its three-point estimate, Tab
// and Shift-Tab in the Gantt rows propose a parent change only while editing (otherwise Tab is the window's), the review
// shows the values an apply writes, one edit can be taken out of the draft, and an applied change is reverted through
// the application's revert as a new reviewed change. Protected work, its ancestors and its prerequisites are refused by
// the application with its reason, and the revision stays.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func nestingAndEstimatesAreReviewedEdits() async throws {
        print("edit: resize scales an estimate, Tab and Shift-Tab propose parent changes, the review shows values, and an apply reverts")
        check(GanttGesture.resize(work: "X", optimistic: 2, likely: 4, pessimistic: 8, left: 100, right: 200, to: 250)
              == .estimate(work: "X", optimistic: 3, likely: 6, pessimistic: 12), "a finish dragged to 1.5 times the length scales all three hours")
        check(GanttGesture.resize(work: "X", optimistic: 2, likely: 4, pessimistic: 8, left: 100, right: 200, to: 200) == nil, "an unchanged length proposes nothing")
        check(KeyDecoder.intent(for: KeyInput(characters: "\t")) == .indent && KeyDecoder.intent(for: KeyInput(characters: "\t", shift: true)) == .outdent
              && KeyDecoder.intent(for: KeyInput(characters: "\u{19}", shift: true)) == .outdent && KeyDecoder.intent(for: KeyInput(characters: "\t", control: true)) == nil,
              "Tab means indent and Shift-Tab outdent; Control-Tab is never the Gantt's")

        let database = try await newStore("edit-nesting", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        // An outer package, and a new task TEST-PRE with an FF link into TEST-A: TEST-A stays claimable while TEST-PRE,
        // unstarted, becomes a prerequisite of it.
        let outer = try await addPackageAndLink(database, key: "TEST-OUTER")
        let model = try await openGanttModel(.database(database))
        _ = try await modelExpect(model, "the plan to be read for editing", 60) { snapshot in
            snapshot.inventory.basis != nil && snapshot.freshness.current && snapshot.gantt != nil && snapshot.inventory.items.contains { $0.key == "TEST-OUTER" }
        }
        let ids = await MainActor.run { Dictionary(model.snapshot.inventory.items.map { ($0.key, $0.identity) }, uniquingKeysWith: { a, _ in a }) }
        guard let a = ids["TEST-A"], let pre = ids["TEST-PRE"], let package = ids["TEST-PKG"] else { check(false, "the Gantt fixture is readable"); return }
        // The row Tab can indent: the first one listed just below a work package at its own level, as drawn.
        let candidate = await MainActor.run { () -> (String, String)? in
            let rows = model.outline
            for (index, row) in rows.enumerated() where !row.row.isPackage {
                let above = rows[..<index].last { model.editParent(of: $0.id) == model.editParent(of: row.id) }
                if let above, above.row.isPackage { return (row.id, above.id) }
            }
            return nil
        }
        guard let (work, into) = candidate else { check(false, "a row is drawn just below a work package at its level"); return }

        // Without editing, Tab is not the Gantt's; with it, Tab proposes the row under that package.
        let plain = await MainActor.run { () -> KeyOutcome in model.focusRow(work); return model.handleKey(KeyInput(characters: "\t")) }
        check(plain == .ignored, "without editing, Tab is left to the window to move the keyboard")
        let indented = await MainActor.run { () -> [PlanEdit] in
            model.setEditing(true)
            model.setEditActor("human:planner")
            model.focusRow(work)
            _ = model.handleKey(KeyInput(characters: "\t"))
            return model.editing.draft?.edits ?? []
        }
        check(indented == [.parent(work: work, parent: into)], "Tab proposes the row under the package drawn above it: \(indented)")
        // The draft counts: Shift-Tab right after Tab takes the row back out, and Tab puts it in again.
        let undone = await MainActor.run { () -> ([PlanEdit], [PlanEdit]) in
            model.focusRow(work)
            _ = model.handleKey(KeyInput(characters: "\t", shift: true))
            let out = model.editing.draft?.edits ?? []
            _ = model.handleKey(KeyInput(characters: "\t"))
            return (out, model.editing.draft?.edits ?? [])
        }
        check(undone.0 == [.parent(work: work, parent: nil)] && undone.1 == [.parent(work: work, parent: into)],
              "Shift-Tab after Tab proposes the row back at its level, and Tab again under the package: \(undone)")
        let outdented = await MainActor.run { () -> PlanEdit? in
            model.focusRow(a)
            _ = model.handleKey(KeyInput(characters: "\t", shift: true))
            return model.editing.draft?.edits.last
        }
        check(outdented == .parent(work: a, parent: nil), "Shift-Tab proposes TEST-A beside its package: \(String(describing: outdented))")

        // The outdent and a stray title are taken out before the review; the indent and an estimate stay.
        let kept = await MainActor.run { () -> [PlanEdit] in
            model.edit(.estimate(work: work, optimistic: 3, likely: 6, pessimistic: 12))
            model.edit(.title(work: work, text: "Not meant"))
            model.removeEdit(at: 3)
            model.removeEdit(at: 1)
            return model.editing.draft?.edits ?? []
        }
        check(kept == [.parent(work: work, parent: into), .estimate(work: work, optimistic: 3, likely: 6, pessimistic: 12)], "edits are taken out of the draft and the others stay: \(kept)")
        await MainActor.run { model.reviewDraft() }
        try await waitEdit(model, "the indent and estimate to be reviewed") { if case .reviewed = $0.phase { return true } else { return false } }
        let shown = await MainActor.run { (model.editing.reviewed?.changeList ?? []).flatMap { ChangeWords.values($0, names: model.editName) } }
        let intoKey = await MainActor.run { model.editName(into) }
        check(shown.contains { $0.contains("parent: none → \(intoKey)") } && shown.contains { $0.contains("likely_hours") && $0.hasSuffix("→ 6") },
              "the review shows the parent and the hours an apply writes: \(shown)")
        await MainActor.run { model.setEditReason("Group it and widen it"); model.applyDraft() }
        try await waitEdit(model, "the indent and estimate to be applied") { if case .applied = $0.phase { return true } else { return false } }
        let applied = try await cliJSON(database, ["history"])["data"]["entries"].array?.last?["operation"]["id"].string ?? ""

        // The application turns the applied change back as a new reviewed change.
        let preview = try await cliJSON(database, ["plan", "revert-preview", applied])
        let revision = try await cliJSON(database, ["revision"])["revision"].uint ?? 0
        let fields = (preview["data"]["changes"].array ?? []).flatMap { ($0["fields"].array ?? []).compactMap(\.string) }.sorted()
        check(fields == ["parent", "schedule"], "the revert preview turns back the parent and the estimate: \(fields)")
        _ = try await cliJSON(database, ["plan", "revert", applied, "--reason", "Ungroup it", "--actor", "human:lead", "--base-revision", "\(revision)"])
        let workKey = await MainActor.run { model.editName(work) }
        let reverted = try await cliJSON(database, ["show", workKey])["data"]["parent"]
        check(reverted == .null, "the revert puts \(workKey) back at the top level: \(reverted)")

        // Claimed work, its prerequisite and its ancestor: each parent change is refused by the application, nothing recorded.
        try await worker(database, ["claim", "TEST-A"])
        _ = try await modelExpect(model, "the claim to be read", 30) { snapshot in snapshot.inventory.items.contains { $0.key == "TEST-A" && $0.status == "Claimed" } && snapshot.freshness.current }
        let before = try await historyCount(database)
        for (what, proposal) in [("the claimed TEST-A", PlanEdit.parent(work: a, parent: nil)), ("its prerequisite TEST-PRE", .parent(work: pre, parent: outer)),
                                 ("its ancestor TEST-PKG", .parent(work: package, parent: outer))] {
            await MainActor.run { model.discardDraft(); model.edit(proposal); model.reviewDraft() }
            try await waitEdit(model, "the parent change of \(what) to be judged") {
                switch $0.phase { case .reviewed, .refused: return true; default: return false }
            }
            let refusal = await MainActor.run { () -> EditRefusal? in if case .refused(let refusal) = model.editing.phase { return refusal } else { return nil } }
            check(refusal?.fromApplication == true && refusal?.message.contains("protected") == true,
                  "a parent change of \(what) is refused by the application with its reason: \(refusal?.message ?? "not refused")")
        }
        let after = try await historyCount(database)
        check(after == before, "the refused parent changes recorded nothing")
        await closeGantt(model)
    }

    /// Add a top-level work package, a task TEST-PRE and an FF link from it into TEST-A as one reviewed change; the
    /// package's identity.
    private func addPackageAndLink(_ database: URL, key: String) async throws -> String {
        let identity = UUID().uuidString.lowercased()
        var plan = try await cliJSON(database, ["export"])["data"]
        let project = plan["work_items"].objectValues.first?["project"] ?? .null
        let item: JSON = .object([
            "id": .string(identity), "key": .string(key), "project": project, "parent": .null, "kind": .string("WorkPackage"),
            "title": .string("An outer package"),
            "contract": .object(["objective": .string("Hold other work"), "acceptance": .array([.object(["text": .string("Holds it")])]),
                                 "capabilities": .array([]), "requirement_ids": .array([]), "assets": .array([])]),
            "execution": .object(["status": .string("Planned"), "reported_progress_percent": .integer(0), "artifact_ids": .array([]), "owner": .null, "block_reason": .null]),
            "schedule": .object(["priority": .string("P2"), "estimate": .null]),
            "order": .array([.integer(10)]),
        ])
        plan = plan.setting(["work_items", identity], to: item)
        let preIdentity = UUID().uuidString.lowercased()
        var task = item.setting(["id"], to: .string(preIdentity)).setting(["key"], to: .string("TEST-PRE")).setting(["kind"], to: .string("Task"))
        task = task.setting(["title"], to: .string("A prerequisite")).setting(["schedule", "estimate"], to: .object([
            "optimistic_hours": .number(1), "likely_hours": .number(2), "pessimistic_hours": .number(4)])).setting(["order"], to: .array([.integer(20)]))
            .setting(["execution", "status"], to: .string("Proposed"))
        plan = plan.setting(["work_items", preIdentity], to: task)
        let keys = Dictionary(plan["work_items"].objectValues.compactMap { item in item["key"].string.map { ($0, item["id"]) } }, uniquingKeysWith: { a, _ in a })
        let link: JSON = .object(["id": .string(UUID().uuidString.lowercased()), "predecessor": keys["TEST-PRE"] ?? .null, "successor": keys["TEST-A"] ?? .null,
                                  "kind": .string("FinishFinish"), "lag_hours": .number(0), "policy": .string("Hard")])
        plan = plan.setting(["dependencies"], to: .array((plan["dependencies"].array ?? []) + [link]))
        let file = scratch("\(key).json")
        try JSONEncoder().encode(plan).write(to: file)
        _ = try await cliJSON(database, ["plan", "apply", file.path, "--reason", "An outer package", "--actor", "human:lead"])
        return identity
    }
}
