// NETWORK-PERT-01: the network reads as an activity-on-node (PERT/CPM) diagram on the 9-node FX. Each scheduled box
// carries the projection's own ES, duration and EF over LS, TF and LF; critical work leads every column, so the
// critical chain TEST-A -> TEST-B -> TEST-C runs along the top row; every connector is drawn with horizontal and vertical
// segments only, leaving and entering the ends its relation names; a plain finish-to-start link without lag is unlabelled while every other link names its relation. The
// text list says every box value in words, and what holds the selected work back reads in plain words. Pure view geometry and text of the real model's graph; nothing is written.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func networkReadsAsPert() async throws {
        print("network: activity-on-node boxes hold the projection's times, critical work leads each column, connectors are right-angled and plain FS links unlabelled")
        let database = try await newStore("network-pert", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        let (graph, schedule) = await MainActor.run { (model.networkGraph, model.snapshot.gantt) }
        guard let schedule else { throw SuiteError(description: "no schedule was read") }

        var wrong: [String] = []
        for node in graph.nodes where !node.isDecision {
            guard let times = schedule.row(node.identity)?.times else { continue }
            let number = { (hours: Double) in String(format: "%.1f", hours) }
            let expected = [number(times.earliestStart), number(times.earliestFinish), number(times.latestStart), number(times.totalFloat), number(times.latestFinish)]
            let cells = node.boxCells?.map(\.value) ?? []
            let shown = cells.count == 6 ? [cells[0], cells[2], cells[3], cells[4], cells[5]] : []
            if shown != expected { wrong.append("\(node.key) box \(cells) vs schedule \(expected)") }
            let words = NetworkText.lines(node, graph: graph, kinds: Set(NetworkEdgeKind.allCases)).joined(separator: " ")
            for fact in ["earliest start \(number(times.earliestStart)) h", "earliest finish \(number(times.earliestFinish)) h", "latest start \(number(times.latestStart)) h",
                         "latest finish \(number(times.latestFinish)) h", "total float \(number(times.totalFloat)) h", "free float \(number(times.freeFloat)) h"] where !words.contains(fact) {
                wrong.append("\(node.key) text list lacks \"\(fact)\"")
            }
        }
        check(wrong.isEmpty && graph.nodes.contains { $0.boxCells != nil }, "every scheduled box holds the projection's ES, EF, LS, TF and LF, and the text list says each: \(wrong)")

        // Critical work leads each column, and the critical chain is the top row.
        var columns: [Int: [NetworkNode]] = [:]
        for node in graph.nodes { if let at = graph.place[node.identity] { columns[at.column, default: []].append(node) } }
        let leading = columns.values.allSatisfy { members in
            let critical = members.filter { $0.critical == true }.compactMap { graph.place[$0.identity]?.row }
            let other = members.filter { $0.critical != true }.compactMap { graph.place[$0.identity]?.row }
            return (critical.max() ?? -1) < (other.min() ?? Int.max)
        }
        let rowOf = { (key: String) in graph.nodes.first { $0.key == key }.flatMap { graph.place[$0.identity]?.row } }
        check(leading && rowOf("TEST-A") == 0 && rowOf("TEST-B") == 0 && rowOf("TEST-C") == 0,
              "critical work leads every column and TEST-A, B and C run along row 0: \(graph.nodes.map { "\($0.key)@\(graph.place[$0.identity].map { "\($0.column),\($0.row)" } ?? "-")" })")

        // Right-angled connectors only.
        let slanted = graph.routes.filter { _, route in route.points.indices.dropFirst().contains { route.points[$0 - 1].x != route.points[$0].x && route.points[$0 - 1].y != route.points[$0].y } }
        check(slanted.isEmpty, "every connector is drawn with horizontal and vertical segments only: \(slanted.keys.sorted())")

        // Each link leaves and enters the ends its relation names: a finish is the right side, a start the left, and the
        // arrow points into the side it enters.
        var misplaced: [String] = []
        for edge in graph.edges where edge.kind == .temporal {
            guard let route = graph.routes[edge.id], let kind = edge.relation?.kind, let a = graph.place[edge.from], let b = graph.place[edge.to] else { continue }
            let from = NetworkLayout.rect(a), to = NetworkLayout.rect(b)
            let leaveRight = kind == "FinishStart" || kind == "FinishFinish", enterLeft = kind == "FinishStart" || kind == "StartStart"
            let startOK = route.start.x == (leaveRight ? from.x + from.width : from.x)
            let endOK = route.end.x == (enterLeft ? to.x : to.x + to.width) && route.entersFromLeft == enterLeft
            if !startOK || !endOK { misplaced.append("\(graph.name(edge.from)) \(GanttRelation.abbreviation(kind)) \(graph.name(edge.to)): \(route.start.x) -> \(route.end.x)") }
        }
        let kindsSeen = Set(graph.edges.compactMap { $0.relation?.kind })
        check(misplaced.isEmpty && kindsSeen.isSuperset(of: ["FinishStart", "StartStart", "FinishFinish"]),
              "every link leaves and enters the ends its relation names (kinds \(kindsSeen.sorted())): \(misplaced)")

        // Plain FS links are unlabelled; every other temporal link names its relation.
        let temporal = graph.edges.filter { $0.kind == .temporal }
        let plain = temporal.filter { $0.relation?.kind == "FinishStart" && $0.relation?.lagHours == 0 && $0.relation?.policy != "Soft" }
        let other = temporal.filter { edge in !plain.contains(edge) }
        check(!plain.isEmpty && plain.allSatisfy { $0.connectorLabel == nil } && !other.isEmpty && other.allSatisfy { ($0.connectorLabel ?? "").hasPrefix($0.relation?.abbreviation ?? "?") },
              "\(plain.count) plain FS links are unlabelled and \(other.count) others name their relation: \(other.map { $0.connectorLabel ?? "nil" })")
        // What holds the selected work back is said in plain words: TEST-C waits for TEST-B to finish and TEST-A to start.
        guard let c = graph.nodes.first(where: { $0.key == "TEST-C" }) else { throw SuiteError(description: "the FX has no TEST-C") }
        await MainActor.run { model.focusNode(c.identity, selecting: true) }
        _ = try await modelExpect(model, "explain's report for TEST-C", 30) { _ in MainActor.assumeIsolated { model.networkReport != nil } }
        let waits = await MainActor.run { model.networkReport?.unmet.map(\.words) ?? [] }
        check(waits.contains { $0.hasPrefix("can start once TEST-B finishes") } && waits.contains { $0.hasPrefix("can start once TEST-A starts") }
              && !waits.contains { $0.contains("FinishStart") || $0.contains("StartStart") || $0.contains("awaiting event") },
              "TEST-C's unmet dependencies read as plain words: \(waits)")
        // Progress marks follow the recorded lifecycle: none before a start, one once TEST-A is started.
        check(graph.nodes.filter { !$0.isDecision && $0.status == "Planned" }.allSatisfy { $0.progressMarks == 0 } && graph.nodes.filter(\.isDecision).allSatisfy { $0.progressMarks == nil },
              "planned work has no progress mark and decisions none at all")
        _ = try await cliJSON(database, ["claim", "TEST-A", "--actor", "agent:worker"])
        _ = try await cliJSON(database, ["start", "TEST-A", "--actor", "agent:worker"])
        await MainActor.run { model.reload() }
        _ = try await modelExpect(model, "TEST-A to read as started", 30) { $0.inventory.items.contains { $0.key == "TEST-A" && $0.status == "InProgress" } }
        let started = await MainActor.run { model.networkGraph.nodes.first { $0.key == "TEST-A" } }
        check(started?.progressMarks == 1, "a started task carries one progress diagonal: \(String(describing: started?.status)) -> \(String(describing: started?.progressMarks))")
        await closeGantt(model)
    }
}
