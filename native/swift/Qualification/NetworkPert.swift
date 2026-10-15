// NETWORK-PERT-01: the network reads as an activity-on-node (PERT/CPM) diagram on the 9-node FX. Each scheduled box
// carries the projection's own ES, duration and EF over LS, TF and LF; critical work leads every column, so the
// critical chain TEST-A -> TEST-B -> TEST-C runs along the top row; every connector is drawn with horizontal and vertical
// segments only; a plain finish-to-start link without lag is unlabelled while every other link names its relation. The
// text list says every box value in words. Pure view geometry and text of the real model's graph; nothing is written.

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

        // Plain FS links are unlabelled; every other temporal link names its relation.
        let temporal = graph.edges.filter { $0.kind == .temporal }
        let plain = temporal.filter { $0.relation?.kind == "FinishStart" && $0.relation?.lagHours == 0 && $0.relation?.policy != "Soft" }
        let other = temporal.filter { edge in !plain.contains(edge) }
        check(!plain.isEmpty && plain.allSatisfy { $0.connectorLabel == nil } && !other.isEmpty && other.allSatisfy { ($0.connectorLabel ?? "").hasPrefix($0.relation?.abbreviation ?? "?") },
              "\(plain.count) plain FS links are unlabelled and \(other.count) others name their relation: \(other.map { $0.connectorLabel ?? "nil" })")
        await closeGantt(model)
    }
}
