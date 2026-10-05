// The network's edge routes, qualified on the ORIGINAL 9-node FX (the Gantt fixture: TEST-A, B and C in one row of
// three adjacent columns, the direct SS +2 h Soft edge TEST-A -> TEST-C beside the FS chain TEST-A -> TEST-B ->
// TEST-C). The direct edge is traceable on its own: its route is apart from the chain and its arrowhead.
// Pure view geometry of the real model's graph: the direct route goes around B,
// keeps apart from the chain and its arrowhead, no route crosses a node, the fit holds every route, and the routes
// are the same on every computation and under every filter. Nothing is written.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func networkSkipEdgeIsRouted() async throws {
        print("network: on the 9-node FX the direct SS edge TEST-A -> TEST-C is routed around TEST-B, apart from the FS chain and its arrowhead; the fit holds every route")
        let database = try await newStore("network-routes", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        let before = try await readOnlyRecord(database)
        let shape = await MainActor.run { () -> (Int, NetworkPlace?, NetworkPlace?, NetworkPlace?, NetworkRoute?, NetworkRoute?, NetworkRoute?, [String]) in
            let graph = model.networkGraph
            let id = { (key: String) in graph.nodes.first { $0.key == key }?.identity ?? "" }
            let a = id("TEST-A"), b = id("TEST-B"), c = id("TEST-C")
            let edge = { (from: String, to: String, kind: String) in graph.edges.first { $0.from == from && $0.to == to && $0.relation?.abbreviation == kind } }
            let ac = edge(a, c, "SS"), ab = edge(a, b, "FS"), bc = edge(b, c, "FS")
            // Every route against every node it does not end on: none may enter a node's interior.
            var crossing: [String] = []
            for item in graph.edges {
                guard let route = graph.routes[item.id] else { crossing.append("\(item.id) has no route"); continue }
                for node in graph.nodes where node.identity != item.from && node.identity != item.to {
                    if let at = graph.place[node.identity], route.touches(NetworkLayout.rect(at)) { crossing.append("\(graph.name(item.from))>\(graph.name(item.to)) crosses \(node.key)") }
                }
            }
            return (graph.nodes.count, graph.place[a], graph.place[b], graph.place[c], ac.flatMap { graph.routes[$0.id] }, ab.flatMap { graph.routes[$0.id] }, bc.flatMap { graph.routes[$0.id] }, crossing)
        }
        guard let pa = shape.1, let pb = shape.2, let pc = shape.3, let ac = shape.4, let ab = shape.5, let bc = shape.6 else {
            check(false, "the 9-node FX has TEST-A, B and C placed with the SS edge A -> C and the FS edges A -> B, B -> C routed")
            await closeGantt(model)
            return
        }
        check(shape.0 == 9 && pa.row == pb.row && pb.row == pc.row && pb.column == pa.column + 1 && pc.column == pa.column + 2,
              "the original FX geometry is kept: \(shape.0) nodes, TEST-A, B and C in row \(pa.row), columns \(pa.column), \(pb.column), \(pc.column)")
        check(ac.detour && !ac.touches(NetworkLayout.rect(pb)), "the direct A -> C route goes around TEST-B: \(ac.points.map { "(\(Int($0.x)), \(Int($0.y)))" })")
        check(shape.7.isEmpty, "no route enters the interior of a node it does not end on: \(shape.7.prefix(5))")
        // Apart from the chain: most of A -> C lies more than 3 points from every point of A -> B and B -> C (a crossing is allowed).
        let chain = ab.samples + bc.samples
        let apart = ac.samples.filter { p in chain.allSatisfy { q in ((p.x - q.x) * (p.x - q.x) + (p.y - q.y) * (p.y - q.y)).squareRoot() > 3 } }.count
        check(Double(apart) >= 0.8 * Double(ac.samples.count), "A -> C keeps apart from the A -> B -> C tracks: \(apart) of \(ac.samples.count) of its points are more than 3 pt from them")
        // The arrowheads (7 pt long, 8 pt high) end at their own points of TEST-C's side, and the starts at TEST-A's.
        check(abs(ac.end.y - bc.end.y) >= 8 && ac.end.x == bc.end.x, "A -> C and B -> C end at their own points of TEST-C's side: y \(ac.end.y) and \(bc.end.y)")
        check(abs(ac.start.y - ab.start.y) >= 8, "A -> C and A -> B leave TEST-A at their own points: y \(ac.start.y) and \(ab.start.y)")
        let ends = await MainActor.run { () -> [String] in
            let graph = model.networkGraph
            var close: [String] = []
            for node in graph.nodes {
                let into = graph.edges(into: node.identity).compactMap { graph.routes[$0.id]?.end.y }.sorted()
                for i in into.indices.dropFirst() where into[i] - into[i - 1] < 5 { close.append(node.key) }
            }
            return close
        }
        check(ends.isEmpty, "every node's incoming ends are at least 5 pt apart, so no two arrowheads coincide: \(ends)")
        // The label of A -> C sits on its own route.
        let label = await MainActor.run { () -> (Bool, Double?) in
            let graph = model.networkGraph
            var candidates: [NetworkLabels.Candidate] = []
            for item in graph.edges where item.kind == .temporal {
                guard let route = graph.routes[item.id], let relation = item.relation else { continue }
                let segment = route.labelSegment
                candidates.append(NetworkLabels.Candidate(edge: item.id, text: relation.abbreviation, from: segment.from, to: segment.to, priority: false))
            }
            let boxes = graph.nodes.compactMap { graph.place[$0.identity].map(NetworkLayout.rect) }
            let segment = ac.labelSegment
            let onSegment = ac.points.indices.dropFirst().contains { ac.points[$0 - 1] == segment.from && ac.points[$0] == segment.to }
            let key = { (id: String) in graph.name(id) }
            let ssId = graph.edges.first { key($0.from) == "TEST-A" && key($0.to) == "TEST-C" && $0.relation?.abbreviation == "SS" }?.id
            let placed = NetworkLabels.place(candidates, nodes: boxes).placed.first { $0.edge == ssId }
            let distance = placed.map { p -> Double in
                let cx = p.x + p.width / 2, cy = p.y + p.height / 2
                return ac.samples.map { (($0.x - cx) * ($0.x - cx) + ($0.y - cy) * ($0.y - cy)).squareRoot() }.min() ?? .infinity
            }
            return (onSegment, distance)
        }
        check(label.0 && (label.1.map { $0 <= 7.5 } ?? true),
              "the A -> C label is anchored on its own route's longest segment (\(label.1.map { "placed \($0) pt from its route" } ?? "left off the canvas; its relation stays in the text list"))")
        // The fit holds every route, end marks included, at an ordinary and at the minimum canvas.
        for viewport in [(706.0, 386.0), (499.0, 276.5)] {
            let outside = await MainActor.run { () -> [String] in
                model.setNetworkViewport(width: viewport.0, height: viewport.1)
                _ = self.pressNetwork(model, "f")
                let state = model.network, graph = model.networkGraph, size = NetworkLayout.extent(graph)
                var out: [String] = []
                for (id, route) in graph.routes {
                    let b = route.bounds
                    if b.x + b.width > size.width || b.y + b.height > size.height { out.append("\(id) beyond the extent") }
                    for p in route.points where p.x * state.scale - state.panX < -0.5 || p.y * state.scale - state.panY < -0.5
                        || p.x * state.scale - state.panX > viewport.0 + 0.5 || p.y * state.scale - state.panY > viewport.1 + 0.5 { out.append("\(id) outside the canvas") }
                }
                return out
            }
            check(outside.isEmpty, "Fit in a \(viewport.0) x \(viewport.1) canvas holds every route inside the extent and the canvas: \(outside.prefix(3))")
        }
        // Deterministic: the same routes on a fresh computation, and the same under every node and edge filter.
        let stable = await MainActor.run { () -> (Bool, [String]) in
            let snapshot = model.snapshot
            let once = NetworkGraph(inventory: snapshot.inventory, schedule: snapshot.gantt), again = NetworkGraph(inventory: snapshot.inventory, schedule: snapshot.gantt)
            let same = once.routes == again.routes && once.routes == model.networkGraph.routes && once.edges.map(\.id) == again.edges.map(\.id)
            let all = model.networkGraph.routes
            var changed: [String] = []
            for (text, keys) in [("TEST-A", ["1", "2", "3"]), ("TEST-C", ["1"]), ("", ["2"])] {
                model.setNetworkFilterText(text)
                for key in [""] + keys {
                    if !key.isEmpty { _ = self.pressNetwork(model, key) }
                    for edge in model.networkVisibleEdges where model.networkGraph.routes[edge.id] != all[edge.id] { changed.append("\(text)/\(key): \(edge.id)") }
                }
                _ = self.pressNetwork(model, "x")
            }
            return (same, changed)
        }
        check(stable.0, "the routes are the same on every computation of the graph, in the same edge order")
        check(stable.1.isEmpty, "a node or edge filter only hides edges: every edge still drawn keeps its route \(stable.1.prefix(3))")
        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }
}
