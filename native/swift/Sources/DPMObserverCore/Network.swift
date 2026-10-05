// The dependency network's data and layout: the plan snapshot (`export`) as nodes and three kinds of
// edge, the schedule projection's own float and criticality on each node, and what `explain` reports
// about the selected task, all decoded as written.
//
// Nothing here schedules or decides readiness. A temporal edge is a plan dependency with its kind, lag
// and policy; a decision-blocking edge is a decision's `blocks` entry, the only edge that gates; a
// context link is a decision's `related_work` entry and never blocks. Predecessors, successors and unmet
// gates of the selected task are the application's report. Columns, rows and points are view geometry.

import DPMNative
import Foundation

// MARK: - The graph

public enum NetworkEdgeKind: String, CaseIterable, Sendable {
    case temporal, decisionBlock = "decision_block", context

    /// The words a person reads and hears for the kind; distinct for each.
    public var words: String {
        switch self {
        case .temporal: return "temporal dependency"
        case .decisionBlock: return "decision gate, blocking"
        case .context: return "related work, context only, not blocking"
        }
    }

    /// The non-colour cue drawn and listed with the kind: a solid arrow, a barred arrow, a dotted line.
    public var cue: String {
        switch self {
        case .temporal: return "→"
        case .decisionBlock: return "⊣"
        case .context: return "⋯"
        }
    }
}

public struct NetworkNode: Identifiable, Equatable, Sendable {
    public enum Kind: String, Sendable { case task = "Task", milestone = "Milestone", package = "WorkPackage", decision = "Decision" }

    public var id: String { identity }
    public let identity: String
    public let key: String
    public let title: String
    public let kind: Kind
    /// The kind as the application wrote it, when this build does not know it.
    public let kindWord: String
    public let status: String
    /// The owner's block reason, as the snapshot records it.
    public let blockReason: String?
    /// From the schedule projection only, when it was read; nil otherwise.
    public let critical: Bool?
    public let totalFloat: Double?
    public let criticality: Double?

    public var isDecision: Bool { kind == .decision }

    /// The kind in words, with its shape named, so it is not told by colour.
    public var kindText: String {
        switch kind {
        case .task: return "task (rectangle)"
        case .milestone: return "milestone (diamond)"
        case .package: return "work package (double outline)"
        case .decision: return "decision gate (hexagon)"
        }
    }
}

public struct NetworkEdge: Identifiable, Hashable, Sendable {
    public let id: String
    public let kind: NetworkEdgeKind
    public let from: String
    public let to: String
    /// The plan dependency, for a temporal edge.
    public let relation: GanttRelation?

    /// The edge in full words: its kind, its ends, and for a temporal edge its relation, lag and policy.
    public func words(names: (String) -> String) -> String {
        switch kind {
        case .temporal:
            guard let relation = relation else { return "\(kind.cue) temporal dependency \(names(from)) → \(names(to))" }
            return "\(kind.cue) temporal dependency: \(relation.words(names: names))"
        case .decisionBlock:
            return "\(kind.cue) decision gate: \(names(from)) blocks \(names(to)) until it is decided"
        case .context:
            return "\(kind.cue) related work: \(names(from)) mentions \(names(to)) as context; not blocking"
        }
    }
}

/// The network of one snapshot: every work item and decision as a node, every edge the snapshot lists.
public struct NetworkGraph: Equatable, Sendable {
    public let nodes: [NetworkNode]
    public let edges: [NetworkEdge]
    public let index: [String: Int]
    /// Edges into and out of each node, by position in `edges`.
    public let incoming: [String: [Int]]
    public let outgoing: [String: [Int]]
    /// Where each node sits: its column (longest chain of gating edges before it) and its row in the column.
    public let place: [String: NetworkPlace]
    public let columns: Int
    public let tallest: Int
    /// How each edge is drawn, by edge id (`NetworkRouting`): geometry only, the one basis of the drawing, the labels,
    /// the culling and the fit.
    public let routes: [String: NetworkRoute]
    /// The right and bottom limits of every route with its end mark, at zoom 1.
    public let routeExtent: (x: Double, y: Double)

    public static func ==(left: NetworkGraph, right: NetworkGraph) -> Bool {
        left.nodes == right.nodes && left.edges == right.edges
    }

    public static let empty = NetworkGraph(inventory: .empty, schedule: nil)

    public init(inventory: Inventory, schedule: GanttSchedule?) {
        var nodes: [NetworkNode] = inventory.items.map { item in
            let row = schedule?.row(item.identity)
            let kind = NetworkNode.Kind(rawValue: item.kind) ?? .task
            return NetworkNode(identity: item.identity, key: item.key, title: item.title, kind: kind, kindWord: item.kind, status: item.status,
                               blockReason: item.blockReason, critical: row?.times?.critical ?? row?.span?.critical, totalFloat: row?.times?.totalFloat,
                               criticality: row?.criticality)
        }
        nodes += inventory.decisions.map { decision in
            NetworkNode(identity: decision.identity, key: decision.key, title: decision.question.isEmpty ? decision.key : decision.question, kind: .decision,
                        kindWord: "Decision", status: decision.status, blockReason: nil, critical: nil, totalFloat: nil, criticality: nil)
        }
        let index = Dictionary(nodes.enumerated().map { ($0.element.identity, $0.offset) }, uniquingKeysWith: { first, _ in first })
        var edges: [NetworkEdge] = inventory.relations.map { relation in
            NetworkEdge(id: "dependency:\(relation.id)", kind: .temporal, from: relation.predecessor, to: relation.successor, relation: relation)
        }
        for decision in inventory.decisions {
            edges += decision.blocks.map { NetworkEdge(id: "blocks:\(decision.identity)>\($0)", kind: .decisionBlock, from: decision.identity, to: $0, relation: nil) }
            edges += decision.related.map { NetworkEdge(id: "related:\(decision.identity)>\($0)", kind: .context, from: decision.identity, to: $0, relation: nil) }
        }
        var incoming: [String: [Int]] = [:], outgoing: [String: [Int]] = [:]
        for (position, edge) in edges.enumerated() {
            outgoing[edge.from, default: []].append(position)
            incoming[edge.to, default: []].append(position)
        }
        self.nodes = nodes
        self.edges = edges
        self.index = index
        self.incoming = incoming
        self.outgoing = outgoing
        let laid = Self.layout(nodes: nodes, edges: edges, index: index)
        place = laid.place
        columns = laid.columns
        tallest = laid.tallest
        routes = NetworkRouting.routes(edges: edges, place: laid.place, tallest: laid.tallest)
        routeExtent = routes.values.reduce((0.0, 0.0)) { far, route in
            let b = route.bounds
            return (max(far.0, b.x + b.width), max(far.1, b.y + b.height))
        }
    }

    /// Columns by the longest chain of temporal and blocking edges before a node (a bounded layered layout:
    /// every such edge points rightwards), rows in plan order within a column. Context links do not place
    /// nodes. A node on a cycle the snapshot should not contain stays in column 0. Geometry only.
    static func layout(nodes: [NetworkNode], edges: [NetworkEdge], index: [String: Int]) -> (place: [String: NetworkPlace], columns: Int, tallest: Int) {
        var indegree = [Int](repeating: 0, count: nodes.count)
        var next = [[Int]](repeating: [], count: nodes.count)
        for edge in edges where edge.kind != .context {
            guard let from = index[edge.from], let to = index[edge.to], from != to else { continue }
            next[from].append(to)
            indegree[to] += 1
        }
        var column = [Int](repeating: 0, count: nodes.count)
        var queue = indegree.indices.filter { indegree[$0] == 0 }
        var head = 0
        while head < queue.count {
            let at = queue[head]
            head += 1
            for to in next[at] {
                column[to] = max(column[to], column[at] + 1)
                indegree[to] -= 1
                if indegree[to] == 0 { queue.append(to) }
            }
        }
        var rowsIn: [Int: Int] = [:]
        var place: [String: NetworkPlace] = [:]
        for (position, node) in nodes.enumerated() {
            let c = indegree[position] > 0 ? 0 : column[position]
            let r = rowsIn[c, default: 0]
            rowsIn[c] = r + 1
            place[node.identity] = NetworkPlace(column: c, row: r)
        }
        return (place, (rowsIn.keys.max() ?? -1) + 1, rowsIn.values.max() ?? 0)
    }

    public func node(_ identity: String) -> NetworkNode? { index[identity].map { nodes[$0] } }

    public func name(_ identity: String) -> String { node(identity)?.key ?? String(identity.prefix(8)) }

    /// The edges into a node, of the kinds asked for.
    public func edges(into identity: String, kinds: Set<NetworkEdgeKind> = Set(NetworkEdgeKind.allCases)) -> [NetworkEdge] {
        (incoming[identity] ?? []).map { edges[$0] }.filter { kinds.contains($0.kind) }
    }

    public func edges(outOf identity: String, kinds: Set<NetworkEdgeKind> = Set(NetworkEdgeKind.allCases)) -> [NetworkEdge] {
        (outgoing[identity] ?? []).map { edges[$0] }.filter { kinds.contains($0.kind) }
    }

    /// The one visible projection both the text list and the canvas present: the edges of the kinds shown
    /// whose two ends are both shown (`shown` nil means every node). An edge to a filtered-out node is in
    /// neither presentation; each node's text says how many such edges are hidden, and clearing the filter
    /// shows them all.
    public func visibleEdges(kinds: Set<NetworkEdgeKind>, shown: Set<String>?) -> [NetworkEdge] {
        edges.filter { kinds.contains($0.kind) && Self.within($0, shown) }
    }

    static func within(_ edge: NetworkEdge, _ shown: Set<String>?) -> Bool {
        guard let shown else { return true }
        return shown.contains(edge.from) && shown.contains(edge.to)
    }

    /// Counts of the text alternative, per J1.11: nodes, listed predecessor entries, listed successor
    /// entries (each equal to the temporal edges), decision blocks and context links.
    public var counts: NetworkCounts {
        var predecessors = 0, successors = 0
        for node in nodes {
            predecessors += edges(into: node.identity, kinds: [.temporal]).count
            successors += edges(outOf: node.identity, kinds: [.temporal]).count
        }
        return NetworkCounts(nodes: nodes.count, temporal: edges.filter { $0.kind == .temporal }.count,
                             predecessorEntries: predecessors, successorEntries: successors,
                             decisionBlocks: edges.filter { $0.kind == .decisionBlock }.count, contextLinks: edges.filter { $0.kind == .context }.count)
    }
}

/// A node's column and its row within the column: geometry only.
public struct NetworkPlace: Equatable, Sendable {
    public let column: Int
    public let row: Int
}

public struct NetworkCounts: Equatable, Sendable {
    public let nodes: Int
    public let temporal: Int
    public let predecessorEntries: Int
    public let successorEntries: Int
    public let decisionBlocks: Int
    public let contextLinks: Int

    public var words: String {
        "\(nodes) nodes; \(temporal) temporal dependencies (\(predecessorEntries) predecessor and \(successorEntries) successor entries); \(decisionBlocks) decision gates; \(contextLinks) context-only links"
    }
}

// MARK: - The text alternative

/// Every node with its predecessors, successors, gates, related work, blockers and the schedule's own values,
/// restricted to the same visible projection the canvas draws (`NetworkGraph.visibleEdges`).
public enum NetworkText {
    public static func lines(_ node: NetworkNode, graph: NetworkGraph, kinds: Set<NetworkEdgeKind>, shown: Set<String>? = nil) -> [String] {
        let name = { (id: String) in graph.node(id).map { "\($0.key) (\($0.title))" } ?? String(id.prefix(8)) }
        var out: [String] = ["\(node.key), \(node.kindText), status \(node.status): \(node.title)"]
        if let reason = node.blockReason { out.append("Blocked, as its owner recorded: \(reason)") }
        if !node.isDecision {
            var values: [String] = []
            if let critical = node.critical { values.append(critical ? "on the critical path" : "not on the critical path") }
            if let float = node.totalFloat { values.append("total float \(String(format: "%.1f", float)) h") }
            if let criticality = node.criticality { values.append(String(format: "criticality %.0f%% of simulations", criticality * 100)) }
            out.append(values.isEmpty ? "Schedule values: not read" : "Schedule (from the query): " + values.joined(separator: ", "))
        }
        let incoming = graph.edges(into: node.identity, kinds: kinds).filter { NetworkGraph.within($0, shown) }
        let outgoing = graph.edges(outOf: node.identity, kinds: kinds).filter { NetworkGraph.within($0, shown) }
        for edge in incoming where edge.kind == .temporal { out.append("Predecessor: \(edge.words(names: name))") }
        for edge in outgoing where edge.kind == .temporal { out.append("Successor: \(edge.words(names: name))") }
        for edge in incoming where edge.kind == .decisionBlock { out.append("Gated by: \(edge.words(names: name))") }
        for edge in outgoing where edge.kind == .decisionBlock { out.append("Gates: \(edge.words(names: name))") }
        for edge in incoming + outgoing where edge.kind == .context { out.append("Context: \(edge.words(names: name))") }
        let hidden = (graph.edges(into: node.identity, kinds: kinds) + graph.edges(outOf: node.identity, kinds: kinds)).count - incoming.count - outgoing.count
        if hidden > 0 { out.append("\(hidden) more \(hidden == 1 ? "edge goes" : "edges go") to nodes the filter hides; clear the filter (x) to read \(hidden == 1 ? "it" : "them")") }
        return out
    }

    /// The edge ids a node's lines present, for comparing the list with the canvas.
    public static func edgeIds(_ node: NetworkNode, graph: NetworkGraph, kinds: Set<NetworkEdgeKind>, shown: Set<String>?) -> Set<String> {
        Set((graph.edges(into: node.identity, kinds: kinds) + graph.edges(outOf: node.identity, kinds: kinds)).filter { NetworkGraph.within($0, shown) }.map(\.id))
    }
}

// MARK: - What explain reports about one task

/// The selected task's predecessors, successors and unmet gates, as `explain` reports them.
public struct NetworkReport: Equatable, Sendable {
    public struct Gate: Equatable, Sendable {
        /// `decision`, `dependency`, `lifecycle` or the unmet type as written.
        public let type: String
        /// The decision or predecessor key it names, if any.
        public let key: String?
        public let words: String
    }

    public let predecessors: [String]
    public let successors: [String]
    public let unmet: [Gate]
    public let blockReason: String?

    init(_ data: JSON) {
        predecessors = data["predecessors"].items.compactMap { $0["id"].string }
        successors = data["context"]["successors"].items.compactMap { $0["id"].string }
        unmet = data["gates"]["unmet"].items.map { json in
            let (_, _, words) = StatusSummary.describe(json)
            return Gate(type: json["type"].string ?? "unknown", key: json["key"].string, words: words)
        }
        blockReason = data["work"]["execution"]["block_reason"].string
    }
}

// MARK: - View state

public struct NetworkFilter: Equatable, Sendable {
    public var text = ""
    /// The edge kinds drawn and listed; all three unless a person hides one.
    public var kinds: Set<NetworkEdgeKind> = Set(NetworkEdgeKind.allCases)

    public init() {}

    public var isActive: Bool { !text.trimmingCharacters(in: .whitespaces).isEmpty || kinds.count != NetworkEdgeKind.allCases.count }

    func matches(_ node: NetworkNode) -> Bool {
        let needle = text.trimmingCharacters(in: .whitespaces).lowercased()
        return needle.isEmpty || [node.key, node.title, node.status, node.kindWord].contains { $0.lowercased().contains(needle) }
    }

    public var words: String {
        var parts: [String] = []
        if !text.isEmpty { parts.append("text “\(text)”") }
        let hidden = NetworkEdgeKind.allCases.filter { !kinds.contains($0) }
        if !hidden.isEmpty { parts.append("hiding " + hidden.map(\.words).joined(separator: ", ")) }
        return parts.isEmpty ? "no filter" : parts.joined(separator: "; ")
    }
}

/// What a person changed about how the network is drawn. None of it is part of the project.
public struct NetworkViewState: Equatable, Sendable {
    public var filter = NetworkFilter()
    public var zoomLevel = NetworkLayout.defaultZoom
    public var panX = 0.0
    public var panY = 0.0
    /// The node the keyboard cursor is on, by persistent identity.
    public var focus: String?
    /// Counts every applied view operation.
    public var sequence = 0
    /// Whether the filter field has the keyboard, so typed letters are text and not commands.
    public var editingFilter = false
    /// Where the walk keys last went, in words, for the region's accessibility value.
    public var walked: String?
    /// The walk in progress, so pressing its key again goes to the next node from the same origin.
    public var walk: NetworkWalk?
    /// The canvas size the window actually gives the graph (0 until measured): bounds the pan from both sides.
    public var viewportWidth = 0.0
    public var viewportHeight = 0.0
    /// Where the pointer rests on the canvas (canvas points from its top left), never a copied node: the node it
    /// inspects is hit again against the current pan, scale and shown nodes (`ObserverModel.networkHover`), so a pan,
    /// a zoom or a filter under a stationary pointer inspects what is under it now. A resize clears it.
    public var hoverPoint: NetworkPoint?
    /// The scale Fit derived from the canvas and the whole graph; while set it replaces the zoom step's scale.
    public var fitScale: Double?
    /// The honest note when Locate cannot show the selected node (the filter hides it); cleared by the next view change.
    public var locateNote: String?
    /// A request to bring the cursor node's row to the top of the text list, advanced (wrapping) only by an explicit
    /// Locate, so a Locate on the unchanged cursor node is seen too; never by a pan, a zoom, a filter or a refresh.
    public var listReveal = 0

    public init() {}

    /// The scale drawn now: the fitted one, else the zoom step's.
    public var scale: Double { fitScale ?? NetworkLayout.scale(zoomLevel) }
}

public struct NetworkPoint: Equatable, Sendable {
    public let x: Double
    public let y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public struct NetworkWalk: Equatable, Sendable {
    public let name: String
    public let origin: String
    public let targets: [String]
    public let index: Int
    public let target: String
}

/// Where the canvas puts the short edge labels (FS, FF + 2 h, Soft): each at the first spot along its edge whose
/// rectangle touches no node and no label already placed, the edges of the cursor node first. A label with no free
/// spot is left off the canvas and counted; every relation stays complete in the text list and the inspector.
/// Presentation only: a label's words are its relation's, unchanged.
public enum NetworkLabels {
    public struct Candidate: Sendable {
        public let edge: String
        public let text: String
        public let from: NetworkPoint
        public let to: NetworkPoint
        public let priority: Bool

        public init(edge: String, text: String, from: NetworkPoint, to: NetworkPoint, priority: Bool) {
            self.edge = edge
            self.text = text
            self.from = from
            self.to = to
            self.priority = priority
        }
    }

    public struct Placed: Equatable, Sendable {
        public let edge: String
        public let text: String
        /// The label's rectangle at zoom 1, in canvas points; its centre is where the text is drawn.
        public let x: Double, y: Double, width: Double, height: Double

        public func touches(_ r: (x: Double, y: Double, width: Double, height: Double)) -> Bool {
            x < r.x + r.width && r.x < x + width && y < r.y + r.height && r.y < y + height
        }
    }

    static let fractions = [0.5, 0.36, 0.64, 0.24, 0.76]
    static let lifts = [-7.0, 7.0]
    /// At most this many labels are tried in one draw; the rest are counted as left off.
    public static let limit = 400

    public static func size(_ text: String) -> (width: Double, height: Double) { (Double(text.count) * 5.2 + 6, 12) }

    public static func place(_ candidates: [Candidate], nodes: [(x: Double, y: Double, width: Double, height: Double)]) -> (placed: [Placed], omitted: Int) {
        let ordered = candidates.enumerated().sorted { a, b in a.element.priority != b.element.priority ? a.element.priority : a.offset < b.offset }.map(\.element)
        var placed: [Placed] = []
        var omitted = 0
        for (index, label) in ordered.enumerated() {
            guard index < limit else { omitted += 1; continue }
            let size = size(label.text)
            var spot: Placed?
            search: for fraction in fractions {
                for lift in lifts {
                    let cx = label.from.x + (label.to.x - label.from.x) * fraction, cy = label.from.y + (label.to.y - label.from.y) * fraction + lift
                    let candidate = Placed(edge: label.edge, text: label.text, x: cx - size.width / 2, y: cy - size.height / 2, width: size.width, height: size.height)
                    if !nodes.contains(where: { candidate.touches($0) }), !placed.contains(where: { candidate.touches(($0.x, $0.y, $0.width, $0.height)) }) {
                        spot = candidate
                        break search
                    }
                }
            }
            if let spot { placed.append(spot) } else { omitted += 1 }
        }
        return (placed, omitted)
    }
}

public enum NetworkLayout {
    public static let nodeWidth = 180.0
    public static let nodeHeight = 44.0
    public static let columnGap = 70.0
    public static let rowGap = 22.0
    public static let zoomSteps: [Double] = [0.25, 0.4, 0.6, 0.8, 1, 1.25, 1.6]
    public static let defaultZoom = 4

    public static func scale(_ level: Int) -> Double { zoomSteps[max(0, min(zoomSteps.count - 1, level))] }

    /// The node's rectangle at zoom 1, in points from the canvas origin.
    public static func origin(column: Int, row: Int) -> (x: Double, y: Double) {
        (Double(column) * (nodeWidth + columnGap) + 20, Double(row) * (nodeHeight + rowGap) + 20)
    }

    /// The whole drawing at zoom 1: every node and every edge route with its end mark, plus the margin.
    public static func extent(_ graph: NetworkGraph) -> (width: Double, height: Double) {
        (max(Double(graph.columns) * (nodeWidth + columnGap) + 40, graph.routeExtent.x + 20),
         max(Double(graph.tallest) * (nodeHeight + rowGap) + 40, graph.routeExtent.y + 20))
    }

    /// The node's rectangle at zoom 1.
    public static func rect(_ place: NetworkPlace) -> (x: Double, y: Double, width: Double, height: Double) {
        let at = origin(column: place.column, row: place.row)
        return (at.x, at.y, nodeWidth, nodeHeight)
    }

    /// The largest pan along each axis: the scaled content beyond the viewport, never below 0.
    public static func maxPan(_ graph: NetworkGraph, level: Int, viewport: (width: Double, height: Double)) -> (x: Double, y: Double) {
        maxPan(graph, scale: scale(level), viewport: viewport)
    }

    public static func maxPan(_ graph: NetworkGraph, scale: Double, viewport: (width: Double, height: Double)) -> (x: Double, y: Double) {
        let size = extent(graph)
        return (max(0, size.width * scale - viewport.width), max(0, size.height * scale - viewport.height))
    }

    /// The scale at which the whole extent, margins included, fits the canvas along both axes: the smaller of the two
    /// ratios, never above the largest zoom step. Nil without a measured canvas. Geometry only.
    public static func fitScale(_ graph: NetworkGraph, viewport: (width: Double, height: Double)) -> Double? {
        let size = extent(graph)
        guard viewport.width > 0, viewport.height > 0, size.width > 0, size.height > 0 else { return nil }
        return min(zoomSteps[zoomSteps.count - 1], viewport.width / size.width, viewport.height / size.height)
    }

    /// The node drawn at a point of the canvas (canvas coordinates, top left), if any: geometry only.
    public static func hit(_ graph: NetworkGraph, shown: [NetworkNode], at point: (x: Double, y: Double), pan: (x: Double, y: Double), level: Int) -> NetworkNode? {
        hit(graph, shown: shown, at: point, pan: pan, scale: scale(level))
    }

    public static func hit(_ graph: NetworkGraph, shown: [NetworkNode], at point: (x: Double, y: Double), pan: (x: Double, y: Double), scale: Double) -> NetworkNode? {
        let x = (point.x + pan.x) / scale, y = (point.y + pan.y) / scale
        return shown.first { node in
            guard let place = graph.place[node.identity] else { return false }
            let r = rect(place)
            return x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
        }
    }
}
