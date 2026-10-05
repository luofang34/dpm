// The dependency network, qualified through the real helper and the real model against disposable stores of
// the network fixture (FX-J1 with decisions: a package whose own decision gate its children inherit, an open
// decision gating two tasks, a context-only related-work link, the four temporal kinds with lead and lag, Hard
// and Soft). Nodes and edges are compared with the plan the CLI exports, the walks with the CLI's `explain` of
// the same task, and every view operation is shown read-only by revision, history count and file bytes.
// Keys are real `NSEvent` values reduced by `KeyInput(event:)` and applied by `handleNetworkKey`, the handler
// the window's monitor calls; whether the window delivers them is the packaged route's (smoke_observer.py).

import AppKit
import DPMNative
import DPMObserverCore
import Foundation

private struct NetworkSnap: Sendable {
    var page: ObserverModel.Page
    var cursor: String?
    var focus: String?
    var selection: Subject?
    var walked: String?
    var editing: Bool
    var origin: ObserverModel.DetailReturn?
}

@MainActor
private func networkSnap(_ model: ObserverModel) -> NetworkSnap {
    NetworkSnap(page: model.page, cursor: model.networkFocusedNode?.key, focus: model.focusTarget, selection: model.selection, walked: model.network.walked,
                editing: model.network.editingFilter, origin: model.detailReturn)
}

extension Context {
    @MainActor
    func pressNetwork(_ model: ObserverModel, _ characters: String, shift: Bool = false, command: Bool = false) -> NetworkKeyOutcome {
        var flags: NSEvent.ModifierFlags = []
        if shift { flags.insert(.shift) }
        if command { flags.insert(.command) }
        guard let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                                           characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: 0),
              let input = KeyInput(event: event) else { return .ignored }
        return model.handleNetworkKey(input)
    }

    private func networkStore(_ name: String) async throws -> URL {
        try await newStore(name, plan: try requirePlan(tools.networkPlan, "--network-plan"))
    }

    // MARK: Content

    func networkContentIsTheApplications() async throws {
        print("network: nodes and the three edge kinds are the exported plan's, with kind, lag and policy; a context link is not a gate")
        let database = try await networkStore("network-content")
        let plan = try await cliJSON(database, ["export"])["data"]
        let model = try await openGanttModel(.database(database), page: .network)
        _ = try await modelExpect(model, "the schedule to be read for the network", 60) { $0.gantt != nil }
        let graph = await MainActor.run { model.networkGraph }
        let workCount: Int = { if case .object(let items) = plan["work_items"] { return items.count }; return plan["work_items"].array?.count ?? 0 }()
        let decisionsJSON: [JSON] = { if case .object(let items) = plan["decisions"] { return Array(items.values) }; return plan["decisions"].array ?? [] }()
        let dependencies = plan["dependencies"].array ?? []
        check(graph.nodes.count == workCount + decisionsJSON.count && workCount > 0, "one node per work item and decision of the export: \(graph.nodes.count) nodes, \(workCount) items and \(decisionsJSON.count) decisions")
        let temporal = graph.edges.filter { $0.kind == .temporal }
        check(Set(temporal.map { $0.relation?.id ?? "" }) == Set(dependencies.compactMap { $0["id"].string }), "the temporal edges are exactly the exported dependencies (\(temporal.count) of \(dependencies.count))")
        var blocks = Set<String>(), related = Set<String>()
        for decision in decisionsJSON {
            let id = decision["id"].string ?? ""
            for target in (decision["blocks"].array ?? []).compactMap(\.string) { blocks.insert("blocks:\(id)>\(target)") }
            for target in (decision["related_work"].array ?? []).compactMap(\.string) { related.insert("related:\(id)>\(target)") }
        }
        check(!blocks.isEmpty && Set(graph.edges.filter { $0.kind == .decisionBlock }.map(\.id)) == blocks, "the decision-blocking edges are the decisions' `blocks`: \(blocks.count)")
        check(!related.isEmpty && Set(graph.edges.filter { $0.kind == .context }.map(\.id)) == related, "the context links are the decisions' `related_work`: \(related.count)")
        check(Set(temporal.compactMap { $0.relation?.abbreviation }) == ["FS", "SS", "FF", "SF"] && Set(temporal.compactMap { $0.relation?.policy }) == ["Hard", "Soft"],
              "all four temporal kinds and both policies are present")
        let names = { (id: String) in graph.name(id) }
        let words = graph.edges.map { ($0.kind, $0.words(names: names)) }
        check(words.contains { $0.0 == .temporal && $0.1.contains("SS (start to start)") && $0.1.contains("lag 2.0 h") && $0.1.contains("Soft") }, "a temporal edge reads its kind, lag and Soft policy")
        check(words.contains { $0.0 == .temporal && $0.1.contains("FF (finish to finish)") && $0.1.contains("lead 1.0 h") && $0.1.contains("Hard") }, "a temporal edge reads its lead and Hard policy")
        check(words.filter { $0.0 == .context }.allSatisfy { $0.1.contains("not blocking") && !$0.1.contains("blocks") && $0.1.hasPrefix(NetworkEdgeKind.context.cue) },
              "every context link reads 'not blocking' with its own cue, never as a gate")
        check(words.filter { $0.0 == .decisionBlock }.allSatisfy { $0.1.contains("blocks") && $0.1.hasPrefix(NetworkEdgeKind.decisionBlock.cue) }, "every decision-blocking edge reads that it blocks, with its own cue")
        check(Set(NetworkEdgeKind.allCases.map(\.cue)).count == 3 && Set(NetworkEdgeKind.allCases.map(\.words)).count == 3, "the three kinds have distinct words and distinct non-colour cues")
        let kinds = Set(graph.nodes.map(\.kindText))
        check(kinds.contains("milestone (diamond)") && kinds.contains("decision gate (hexagon)") && kinds.contains("task (rectangle)") && kinds.contains("work package (double outline)"), "task, milestone, package and decision nodes read their distinct shapes: \(kinds.sorted())")
        // The schedule's values on each node are the shared query's, never computed here.
        let schedule = try await cliJSON(database, ["--clock", tools.clock, "schedule"])["data"]["work"].array ?? []
        var differences: [String] = []
        for row in schedule {
            guard let id = row["id"].string, let node = graph.node(id) else { differences.append("missing \(row["key"].string ?? "?")"); continue }
            let float: Double? = { switch row["times"]["total_float_hours"] { case .number(let v): return v; case .integer(let v): return Double(v); default: return nil } }()
            if node.totalFloat != float { differences.append("\(node.key) float") }
        }
        check(differences.isEmpty, "each node's float is the schedule query's: \(differences)")
        // A context link places no node and gates nothing: no walk reaches a node through it.
        let contextTargets = Set(graph.edges.filter { $0.kind == .context }.map(\.to))
        check(contextTargets.allSatisfy { target in graph.edges(into: target, kinds: [.decisionBlock]).allSatisfy { edge in !graph.edges.contains { $0.kind == .context && $0.from == edge.from && $0.to == target } } },
              "no context link is also listed as a block of the same target")
        await closeGantt(model)
    }

    // MARK: One projection for the list and the canvas (N2), and J1.11 counts

    func networkListAndCanvasShareOneProjection() async throws {
        print("network: the text list and the canvas present the same edges under every node and edge filter; the full counts equal the plan's (J1.11); nothing is written")
        let database = try await networkStore("network-projection")
        let model = try await openGanttModel(.database(database), page: .network)
        _ = try await modelExpect(model, "the schedule to be read for the network", 60) { $0.gantt != nil }
        let before = try await readOnlyRecord(database)
        let plan = try await cliJSON(database, ["export"])["data"]
        let full = await MainActor.run { model.networkGraph.counts }
        check(full.predecessorEntries == full.temporal && full.successorEntries == full.temporal && full.temporal == (plan["dependencies"].array ?? []).count,
              "J1.11 (b): listed predecessor and successor entries each equal the temporal edges: \(full.words)")
        func compare(_ label: String) async {
            let (listed, drawn, hiddenNoted) = await MainActor.run { () -> (Set<String>, Set<String>, Bool) in
                let graph = model.networkGraph
                let shown = model.networkShown
                var listed = Set<String>()
                var noted = true
                for node in model.networkNodes {
                    listed.formUnion(NetworkText.edgeIds(node, graph: graph, kinds: model.network.filter.kinds, shown: shown))
                    let all = (graph.edges(into: node.identity, kinds: model.network.filter.kinds) + graph.edges(outOf: node.identity, kinds: model.network.filter.kinds)).count
                    let mine = NetworkText.edgeIds(node, graph: graph, kinds: model.network.filter.kinds, shown: shown).count
                    let lines = NetworkText.lines(node, graph: graph, kinds: model.network.filter.kinds, shown: shown)
                    if all > mine && !lines.contains(where: { $0.contains("to nodes the filter hides") }) { noted = false }
                }
                return (listed, Set(model.networkVisibleEdges.map(\.id)), noted)
            }
            check(listed == drawn, "\(label): the list presents exactly the canvas's edges (\(listed.count) listed, \(drawn.count) drawn; only in list \(listed.subtracting(drawn).sorted().prefix(3)), only drawn \(drawn.subtracting(listed).sorted().prefix(3)))")
            check(hiddenNoted, "\(label): a node whose edges go to filtered-out nodes says so in its text")
        }
        await compare("unfiltered")
        // Filter to one endpoint of an edge of each kind: TEST-GATE (blocks and related work) and TEST-A (temporal).
        for text in ["TEST-GATE", "TEST-A"] {
            await MainActor.run { model.setNetworkFilterText(text) }
            await compare("filtered to \(text)")
            for key in ["1", "2", "3"] {
                _ = await MainActor.run { self.pressNetwork(model, key) }
                await compare("filtered to \(text), kind \(key) toggled off")
                _ = await MainActor.run { self.pressNetwork(model, key) }
            }
        }
        let cleared = await MainActor.run { () -> (Int, Int) in
            _ = self.pressNetwork(model, "x")
            return (model.networkNodes.count, model.networkVisibleEdges.count)
        }
        check(cleared.0 == full.nodes && cleared.1 == full.temporal + full.decisionBlocks + full.contextLinks, "clearing the filter presents every node and edge again: \(cleared)")
        await compare("cleared")
        // Zoom and pan change only the view.
        await MainActor.run {
            for key in ["=", "=", "-"] { _ = self.pressNetwork(model, key) }
            _ = self.pressNetwork(model, KeyDecoder.right, shift: true)
            _ = self.pressNetwork(model, KeyDecoder.down, shift: true)
        }
        let view = await MainActor.run { model.network }
        check(view.zoomLevel == NetworkLayout.defaultZoom + 1 && view.panX > 0 && view.panY > 0, "zoom and pan moved the view: zoom \(view.zoomLevel), pan \(view.panX), \(view.panY)")
        let after = try await readOnlyRecord(database)
        check(after == before, "R of J1.1: revision, history and file bytes are identical after filter, zoom and pan: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: Entry with a selection, the walk and Detail (N3, N4, J1.9, J1.15)

    func networkEntryWalkAndDetail() async throws {
        print("network: entered with a nested task selected elsewhere, the cursor is on it; p, s, g, b reach what explain reports (an inherited gate included); Detail returns to the node")
        let database = try await networkStore("network-walk")
        let model = try await openGanttModel(.database(database))
        let first = await MainActor.run { model.snapshot }
        guard let nested = identity(of: "TEST-C", in: first) else { throw SuiteError(description: "TEST-C has no identity") }
        await MainActor.run { model.select(.work(nested)) }
        _ = try await modelExpect(model, "TEST-C's detail on the Gantt", 30) { $0.detail?.subject == .work(nested) && $0.detail?.loading == false }
        let before = try await readOnlyRecord(database)
        await MainActor.run { model.show(.network) }
        var state = await MainActor.run { networkSnap(model) }
        check(state.page == .network && state.cursor == "TEST-C" && state.focus == "network.node.TEST-C" && state.selection == .work(nested),
              "entering the network puts the cursor on the selected nested task, not the first node: cursor \(state.cursor ?? "none"), focus \(state.focus ?? "none")")
        let explain = try await cliJSON(database, ["--clock", tools.clock, "explain", "TEST-C"])["data"]
        let graph = await MainActor.run { model.networkGraph }
        let keyOf = { (id: String) in graph.node(id)?.key ?? id }
        let predecessors = (explain["predecessors"].array ?? []).compactMap { $0["id"].string }.map(keyOf)
        let successors = (explain["context"]["successors"].array ?? []).compactMap { $0["id"].string }.map(keyOf)
        let unmet = explain["gates"]["unmet"].array ?? []
        let gates = unmet.filter { $0["type"].string == "decision" }.compactMap { $0["key"].string }
        let blockers = unmet.compactMap { $0["key"].string }
        check(gates.contains("TEST-PKG-GATE"), "the application reports the parent package's decision as TEST-C's gate: \(gates)")
        check(graph.edges(into: nested, kinds: [.decisionBlock]).isEmpty, "TEST-C has no direct decision-blocking edge in the plan, so the gate is inherited")
        func walkAll(_ key: String, _ count: Int) async -> [String] {
            var reached: [String] = []
            for _ in 0..<max(count, 1) {
                let outcome = await MainActor.run { self.pressNetwork(model, key) }
                let at = await MainActor.run { networkSnap(model) }
                if outcome != .ignored, let cursor = at.cursor, cursor != "TEST-C" { reached.append(cursor) }
            }
            // Back to the origin for the next walk.
            await MainActor.run { model.focusNode(nested) }
            return reached
        }
        let walkedPredecessors = await walkAll("p", predecessors.count)
        check(!predecessors.isEmpty && walkedPredecessors == predecessors, "p reaches every predecessor explain reports, in its order: \(walkedPredecessors) vs \(predecessors)")
        let walkedSuccessors = await walkAll("s", successors.count)
        check(!successors.isEmpty && walkedSuccessors == successors, "s reaches every successor explain reports: \(walkedSuccessors) vs \(successors)")
        let walkedGates = await walkAll("g", gates.count)
        check(walkedGates == gates, "g reaches the decision gates explain reports, the inherited one included: \(walkedGates) vs \(gates)")
        let walkedBlockers = await walkAll("b", blockers.count)
        check(!blockers.isEmpty && walkedBlockers == blockers, "b reaches the blockers explain reports: \(walkedBlockers) vs \(blockers)")
        state = await MainActor.run { networkSnap(model) }
        check(state.selection == .work(nested), "walking moved the cursor only; the shared selection is still TEST-C")
        // The related-work target is never a gate of anything: walking g from TEST-F reaches nothing through a context link.
        if let related = identity(of: "TEST-F", in: first) {
            await MainActor.run { model.focusNode(related, selecting: true) }
            _ = try await modelExpect(model, "TEST-F's detail", 30) { $0.detail?.subject == .work(related) && $0.detail?.loading == false }
            let unmetF = try await cliJSON(database, ["--clock", tools.clock, "explain", "TEST-F"])["data"]["gates"]["unmet"].array ?? []
            let explainF = unmetF.filter { $0["type"].string == "decision" }.compactMap { $0["key"].string }
            let walked = await MainActor.run { () -> [String] in model.networkWalk(.gate).map(keyOf) }
            check(!walked.contains("TEST-GATE") && walked == explainF, "a context-only link does not gate: TEST-F's gates are explain's \(explainF), the walk gives \(walked)")
            await MainActor.run { model.focusNode(nested, selecting: true) }
            _ = try await modelExpect(model, "TEST-C's detail again", 30) { $0.detail?.subject == .work(nested) && $0.detail?.loading == false }
        }
        // Detail by Return, a followed link inside, and Escape: back to the originating node.
        var outcome = await MainActor.run { self.pressNetwork(model, "\r") }
        state = await MainActor.run { networkSnap(model) }
        check(outcome == .handled(.openDetail) && state.page == .detail && state.origin?.page == .network && state.origin?.element == "network.node.TEST-C", "Return opens Detail and remembers the node: \(String(describing: state.origin))")
        if let other = identity(of: "TEST-A", in: first) { await MainActor.run { model.select(.work(other)) } }
        outcome = await MainActor.run { () -> NetworkKeyOutcome in model.closeDetail(); return .handled(.closeDetail) }
        state = await MainActor.run { networkSnap(model) }
        check(state.page == .network && state.cursor == "TEST-C" && state.focus == "network.node.TEST-C", "closing Detail returns to the network with the cursor and the focus request on the originating node, after a followed link: \(state.cursor ?? "none")")
        let down = await MainActor.run { self.pressNetwork(model, KeyDecoder.down) }
        check(down == .handled(.moveDown), "one Down after the return is the network's")
        let after = try await readOnlyRecord(database)
        check(after == before, "the walk, Detail and the return wrote nothing: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: The filter's focus (N1), model path

    /// MODEL PATH: `/` only requests the field; letters stay commands until the field reports it has the keyboard;
    /// then letters are text and an ordinary Escape returns the cursor node. Real field focus, marked text and Tab
    /// are the packaged route's and the person's (the window is not hosted here).
    func networkFilterFocusIsTheFields() async throws {
        print("network: [model path] slash requests the filter field without claiming it; editing is what the field reports; Escape hands back to the node")
        let database = try await networkStore("network-filter")
        let model = try await openGanttModel(.database(database), page: .network)
        let result = await MainActor.run { () -> (String?, Bool, NetworkKeyOutcome, Bool, NetworkKeyOutcome, String?, Bool, NetworkKeyOutcome) in
            model.reconcileNetworkFocus()
            _ = self.pressNetwork(model, "/")
            let requested = model.focusTarget
            let claimed = model.network.editingFilter
            model.networkFilterFocus(true)
            let letter = self.pressNetwork(model, "p")
            let editing = model.network.editingFilter
            let escape = self.pressNetwork(model, "\u{1B}")
            let back = model.focusTarget
            let left = model.network.editingFilter
            let down = self.pressNetwork(model, KeyDecoder.down)
            return (requested, claimed, letter, editing, escape, back, left, down)
        }
        check(result.0 == "network.filter" && !result.1, "slash requests the filter (\(result.0 ?? "none")) and does not claim it is being typed in before the field says so")
        check(result.2 == .ignored && result.3, "once the field reports the keyboard, a letter is text, not a walk")
        check(result.4 == .handled(.closeDetail) && result.5?.hasPrefix("network.node.") == true && !result.6, "an ordinary Escape leaves the field and names the cursor node: \(result.5 ?? "none")")
        check(result.7 == .handled(.moveDown), "then one Down is the network's")
        let tabbed = await MainActor.run { () -> (Bool, String?) in
            _ = self.pressNetwork(model, "/")
            model.networkFilterFocus(true)
            model.networkFilterFocus(false)
            return (model.network.editingFilter, model.focusTarget)
        }
        check(!tabbed.0 && tabbed.1 == nil, "leaving the field by Tab (focus lost, no Escape) ends editing and names no node: \(tabbed.1 ?? "none")")
        await closeGantt(model)
    }

    // MARK: Stale and lost

    func networkKeepsItsViewWhenLost() async throws {
        print("network: a lost helper keeps the last graph, the cursor and the selection; recovery reads it again")
        // The Gantt fixture, without the package gate, so TEST-A can be claimed from outside (in the network fixture the
        // inherited TEST-PKG-GATE rightly refuses that claim).
        let database = try await newStore("network-stale", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database), page: .network) { $0.reconnectDelays = [0.4, 0.6] }
        _ = try await modelExpect(model, "the schedule to be read for the network", 60) { $0.gantt != nil }
        let first = await MainActor.run { model.snapshot }
        guard let id = identity(of: "TEST-B", in: first) else { throw SuiteError(description: "TEST-B has no identity") }
        await MainActor.run { model.focusNode(id, selecting: true) }
        let nodes = await MainActor.run { model.networkGraph.nodes.count }
        guard let pid = first.helperProcess else { throw SuiteError(description: "no helper process") }
        kill(pid, SIGKILL)
        let lost = try await modelExpect(model, "the lost helper to be shown", 30) { if case .reconnecting = $0.connection { return true } else { return false } }
        let held = await MainActor.run { (model.networkGraph.nodes.count, networkSnap(model)) }
        check(!lost.freshness.current && held.0 == nodes && held.1.cursor == "TEST-B" && held.1.selection == .work(id), "while lost the graph (\(held.0) nodes), cursor and selection stay, marked not current")
        _ = try await modelExpect(model, "recovery", 60) { $0.connection == .connected && $0.freshness.current && $0.gantt != nil && $0.counters.reconnects >= 1 }
        let back = await MainActor.run { (model.networkGraph.nodes.count, networkSnap(model)) }
        check(back.0 == nodes && back.1.cursor == "TEST-B", "after recovery the graph and the cursor are as they were")
        try await worker(database, ["claim", "TEST-A"])
        _ = try await modelExpect(model, "the external claim to reach the network", 30) { $0.inventory.items.contains { $0.key == "TEST-A" && $0.status == "Claimed" } && $0.freshness.current }
        let status = await MainActor.run { model.networkGraph.nodes.first { $0.key == "TEST-A" }?.status }
        check(status == "Claimed", "an external commit is shown on the node: TEST-A is \(status ?? "missing")")
        await closeGantt(model)
    }

    // MARK: Dense graph (DS-D, 1000 tasks)

    func networkDenseGraph() async throws {
        print("network: the 1000-task dense overlay (DS-D) is complete in the list and navigable by keys; nothing is written")
        let database = try await newStore("network-dense", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        let before = try await readOnlyRecord(database)
        let (counts, columns, placed) = await MainActor.run { () -> (NetworkCounts, Int, Int) in
            let graph = model.networkGraph
            return (graph.counts, graph.columns, graph.place.count)
        }
        check(counts.nodes == 1100 && counts.temporal == 1994 && counts.predecessorEntries == 1994 && counts.successorEntries == 1994, "DS-D: 1100 nodes and 2N-6 = 1994 temporal edges, all listed: \(counts.words)")
        check(placed == 1100 && columns >= 334, "every node is placed; the layered layout has at least the longest path's 334 columns: \(columns)")
        // The REAL End key, checked on its own: it goes to the last node in plan order, which in DS-D with its hierarchy
        // overlay is the package PERF-P100; a package has no temporal predecessor, so that is asserted as what it is.
        let ended = await MainActor.run { () -> (String?, String?, Int) in
            _ = self.pressNetwork(model, KeyDecoder.end)
            return (model.networkFocusedNode?.key, model.networkNodes.last?.key, model.networkWalk(.predecessor).count)
        }
        check(ended.0 != nil && ended.0 == ended.1, "End goes to the last node in plan order: \(ended.0 ?? "none") (last listed \(ended.1 ?? "none"), \(ended.2) predecessors)")
        let walked = await MainActor.run { () -> (String?, Int, String?) in
            // The cursor is positioned on PERF-1000 through the model API (focusNode), NOT by a key; then a real p walks.
            if let task = model.networkGraph.nodes.first(where: { $0.key == "PERF-1000" }) { model.focusNode(task.identity) }
            let last = model.networkFocusedNode?.key
            let found = model.networkWalk(.predecessor).count
            _ = self.pressNetwork(model, "p")
            return (last, found, model.networkFocusedNode?.key)
        }
        check(walked.0 != nil && walked.2 != walked.0, "model-positioned on PERF-1000 (focusNode), a real p walks the dense graph: from \(walked.0 ?? "none") to \(walked.2 ?? "none") (\(walked.1) predecessors)")
        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: Long titles

    func networkLongTitlesStayWhole() async throws {
        print("network: a 500-character title is in the node's text in full")
        let database = try await newStore("network-long", plan: try requirePlan(tools.longPlan, "--long-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        let whole = await MainActor.run { () -> (Int, Bool) in
            let graph = model.networkGraph
            let long = graph.nodes.filter { $0.title.count == 500 }
            return (long.count, long.allSatisfy { NetworkText.lines($0, graph: graph, kinds: Set(NetworkEdgeKind.allCases)).first?.hasSuffix($0.title) == true })
        }
        check(whole.0 == 10 && whole.1, "ten nodes carry a 500-character title and each node's first line ends with the whole title (\(whole.0))")
        await closeGantt(model)
    }
}
