// The presentation routes of the network's edges: view geometry only, from the node placement and the edge
// list, never from a schedule or a rule. An edge between adjacent columns is a curve across the column gap
// between them. Every other edge (one that skips columns, goes back or stays in its column) leaves its source
// into the gap on its right, runs along a row gap, where no node is ever placed, and enters its target from the
// gap on its left, so it never runs through an intervening node. Each node spreads its incoming ends (and its
// outgoing starts) over its side, so two edges into one node never share a track or an arrowhead.
// The routes are the single basis of the drawing, the label anchors, the culling bounds and the fit extent.

import Foundation

public struct NetworkRoute: Equatable, Sendable {
    /// The points the edge is drawn through, from the source's right side to the target's left side.
    public let points: [NetworkPoint]
    /// Whether the route runs along a row gap (straight segments); else it is one curve across a column gap.
    public let detour: Bool

    /// How far the curve's controls reach from its ends, along the horizontal.
    public static let reach = 30.0
    /// What the end mark (an arrowhead or a bar) adds around the route's points.
    static let mark = 8.0

    public var start: NetworkPoint { points[0] }
    public var end: NetworkPoint { points[points.count - 1] }

    /// The rectangle the drawn route and its end mark lie in, at zoom 1.
    public var bounds: (x: Double, y: Double, width: Double, height: Double) {
        let xs = points.map(\.x), ys = points.map(\.y)
        let minX = xs.min()! - Self.mark, minY = ys.min()! - Self.mark
        return (minX, minY, xs.max()! + Self.mark - minX, ys.max()! + Self.mark - minY)
    }

    /// The segment a label sits on: the longest one of the route (for a curve, its chord, whose middle is the curve's).
    public var labelSegment: (from: NetworkPoint, to: NetworkPoint) {
        guard detour else { return (start, end) }
        var best = (points[0], points[1])
        for i in 1..<points.count where Self.length(points[i - 1], points[i]) > Self.length(best.0, best.1) { best = (points[i - 1], points[i]) }
        return best
    }

    /// Points along the route as it is drawn, no more than about 2 points apart.
    public var samples: [NetworkPoint] {
        if !detour {
            let a = start, b = end
            let c1 = NetworkPoint(x: a.x + Self.reach, y: a.y), c2 = NetworkPoint(x: b.x - Self.reach, y: b.y)
            let steps = max(16, Int(Self.length(a, b) / 2))
            return (0...steps).map { i in
                let t = Double(i) / Double(steps), u = 1 - t
                let w0 = u * u * u, w1 = 3 * u * u * t, w2 = 3 * u * t * t, w3 = t * t * t
                return NetworkPoint(x: w0 * a.x + w1 * c1.x + w2 * c2.x + w3 * b.x, y: w0 * a.y + w1 * c1.y + w2 * c2.y + w3 * b.y)
            }
        }
        var out = [points[0]]
        for i in 1..<points.count {
            let a = points[i - 1], b = points[i]
            let steps = max(1, Int((Self.length(a, b) / 2).rounded(.up)))
            for s in 1...steps {
                let t = Double(s) / Double(steps)
                out.append(NetworkPoint(x: a.x + (b.x - a.x) * t, y: a.y + (b.y - a.y) * t))
            }
        }
        return out
    }

    /// Whether the drawn route enters the interior of a rectangle (its border excluded, so a route's own ends on
    /// its nodes' sides do not count).
    public func touches(_ r: (x: Double, y: Double, width: Double, height: Double)) -> Bool {
        samples.contains { $0.x > r.x + 0.5 && $0.x < r.x + r.width - 0.5 && $0.y > r.y + 0.5 && $0.y < r.y + r.height - 0.5 }
    }

    static func length(_ a: NetworkPoint, _ b: NetworkPoint) -> Double { ((b.x - a.x) * (b.x - a.x) + (b.y - a.y) * (b.y - a.y)).squareRoot() }
}

public enum NetworkRouting {
    /// Offsets of the parallel lanes inside one row gap, in the order edges take them; each is within the gap.
    static let lanes = [0.0, 4.0, -4.0, 8.0, -8.0]

    /// The y of row gap `g`: above row 0 for g = 0, else between rows g - 1 and g.
    static func gapY(_ g: Int) -> Double {
        NetworkLayout.origin(column: 0, row: g).y - NetworkLayout.rowGap / 2
    }

    /// How far the `k`th vertical stub of a node sits from its side, inside the column gap: starts use the gap's
    /// left half and ends its right half, so the two never share a track.
    static func stub(_ k: Int) -> Double { 8 + 5 * Double(k % 6) }

    /// Every edge's route, by edge id, in a deterministic order (the edge list's, then the node rows). Nodes
    /// without a place have no route.
    public static func routes(edges: [NetworkEdge], place: [String: NetworkPlace], tallest: Int) -> [String: NetworkRoute] {
        struct Plan { let index: Int; let edge: NetworkEdge; let from: NetworkPlace; let to: NetworkPlace; let detour: Bool; let gap: Int; let lane: Double; let leave: Double; let arrive: Double }
        let centre = { (p: NetworkPlace) in NetworkLayout.origin(column: p.column, row: p.row).y + NetworkLayout.nodeHeight / 2 }
        var plans: [Plan] = []
        var taken: [Int: Int] = [:]
        for (index, edge) in edges.enumerated() {
            guard let a = place[edge.from], let b = place[edge.to] else { continue }
            let detour = b.column != a.column + 1
            var gap = 0, lane = 0.0
            if detour {
                // The row gap nearest the middle of the two ends; on a tie the lower one.
                let middle = (centre(a) + centre(b)) / 2
                for g in 0...max(tallest, 1) where abs(gapY(g) - middle) <= abs(gapY(gap) - middle) { gap = g }
                lane = lanes[taken[gap, default: 0] % lanes.count]
                taken[gap, default: 0] += 1
            }
            let track = gapY(gap) + lane
            plans.append(Plan(index: index, edge: edge, from: a, to: b, detour: detour, gap: gap, lane: lane,
                              leave: detour ? track : centre(b), arrive: detour ? track : centre(a)))
        }
        // Each node's starts and ends, spread over its side in the order their routes leave or arrive.
        var outs: [String: [Int]] = [:], ins: [String: [Int]] = [:]
        for (position, plan) in plans.enumerated() {
            outs[plan.edge.from, default: []].append(position)
            ins[plan.edge.to, default: []].append(position)
        }
        var startY = [Double](repeating: 0, count: plans.count), endY = startY
        var startStub = [Int](repeating: 0, count: plans.count), endStub = startStub
        func spread(_ groups: [String: [Int]], key: (Plan) -> Double, place: (Plan) -> NetworkPlace, into y: inout [Double], stubs: inout [Int]) {
            for (_, members) in groups {
                let ordered = members.sorted { key(plans[$0]) != key(plans[$1]) ? key(plans[$0]) < key(plans[$1]) : plans[$0].index < plans[$1].index }
                var detours = 0
                for (rank, position) in ordered.enumerated() {
                    let p = place(plans[position])
                    y[position] = NetworkLayout.origin(column: p.column, row: p.row).y + NetworkLayout.nodeHeight * Double(rank + 1) / Double(ordered.count + 1)
                    if plans[position].detour { stubs[position] = detours; detours += 1 }
                }
            }
        }
        spread(outs, key: \.leave, place: \.from, into: &startY, stubs: &startStub)
        spread(ins, key: \.arrive, place: \.to, into: &endY, stubs: &endStub)
        var routes: [String: NetworkRoute] = [:]
        for (position, plan) in plans.enumerated() {
            let from = NetworkLayout.origin(column: plan.from.column, row: plan.from.row), to = NetworkLayout.origin(column: plan.to.column, row: plan.to.row)
            let start = NetworkPoint(x: from.x + NetworkLayout.nodeWidth, y: startY[position])
            let end = NetworkPoint(x: to.x, y: endY[position])
            guard plan.detour else {
                routes[plan.edge.id] = NetworkRoute(points: [start, end], detour: false)
                continue
            }
            let track = gapY(plan.gap) + plan.lane
            let out = start.x + stub(startStub[position]), into = max(2, end.x - stub(endStub[position]))
            routes[plan.edge.id] = NetworkRoute(points: [start, NetworkPoint(x: out, y: start.y), NetworkPoint(x: out, y: track),
                                                         NetworkPoint(x: into, y: track), NetworkPoint(x: into, y: end.y), end], detour: true)
        }
        return routes
    }
}
