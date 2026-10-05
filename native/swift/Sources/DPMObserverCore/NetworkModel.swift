// How the shared model carries the dependency network: the graph of the installed snapshot, the
// keyboard cursor and walk, and the view operations. Like the Gantt's, every operation here changes
// what is drawn and nothing else; the only call that leaves this file is the shared `select` (and
// `openDetail`, which selects), which makes a read. Filter, zoom and pan never touch the selection.

import Foundation

public enum NetworkIntent: String, CaseIterable, Sendable {
    case moveUp, moveDown, first, last
    case select, openDetail, closeDetail
    case predecessor, successor, gate, blocker
    case zoomIn, zoomOut, panLeft, panRight, panUp, panDown
    case toggleTemporal, toggleBlocks, toggleContext, clearFilter, focusFilter
    case fit, locate, resetView
}

/// Which network intent a key press means, if any. It applies nothing.
public enum NetworkKeyDecoder {
    public static func intent(for input: KeyInput) -> NetworkIntent? {
        if input.control { return nil }
        if input.command {
            switch input.characters {
            case "d": return .openDetail
            case "[": return .closeDetail
            default: return nil
            }
        }
        if input.option { return nil }
        switch input.characters {
        case KeyDecoder.up: return input.shift ? .panUp : .moveUp
        case KeyDecoder.down: return input.shift ? .panDown : .moveDown
        case KeyDecoder.left: return input.shift ? .panLeft : nil
        case KeyDecoder.right: return input.shift ? .panRight : nil
        case KeyDecoder.home: return .first
        case KeyDecoder.end: return .last
        case " ": return .select
        case "\r", "\u{3}": return .openDetail
        case "\u{1B}": return .closeDetail
        case "p": return .predecessor
        case "s": return .successor
        case "g": return .gate
        case "b": return .blocker
        case "=", "+": return .zoomIn
        case "-", "_": return .zoomOut
        case "1": return .toggleTemporal
        case "2": return .toggleBlocks
        case "3": return .toggleContext
        case "x": return .clearFilter
        case "/": return .focusFilter
        case "f": return .fit
        case "l": return .locate
        case "0": return .resetView
        default: return nil
        }
    }
}

public enum NetworkKeyOutcome: Equatable, Sendable {
    case ignored
    case handled(NetworkIntent)
}

extension ObserverModel {
    static let networkPanStep = 120.0

    /// The graph of the installed snapshot: the inventory (`export`) and, while it is read, the schedule.
    public var networkGraph: NetworkGraph {
        let token = snapshot.gantt?.token
        if let cached = networkCache, cached.schedule == token, cached.inventory == snapshot.inventory { return cached.graph }
        let graph = NetworkGraph(inventory: snapshot.inventory, schedule: snapshot.gantt)
        networkCache = (snapshot.inventory, token, graph)
        return graph
    }

    /// The nodes of the text list, in plan order, after the text filter. Hidden edge kinds hide edges, not nodes.
    public var networkNodes: [NetworkNode] {
        let filter = network.filter
        return networkGraph.nodes.filter { filter.matches($0) }
    }

    /// The identities of the nodes shown, or nil when the text filter shows every node.
    public var networkShown: Set<String>? {
        network.filter.text.trimmingCharacters(in: .whitespaces).isEmpty ? nil : Set(networkNodes.map(\.identity))
    }

    /// The edges both the text list and the canvas present: one projection for both.
    public var networkVisibleEdges: [NetworkEdge] { networkGraph.visibleEdges(kinds: network.filter.kinds, shown: networkShown) }

    /// On entering the network, and when the snapshot's nodes change: the cursor is the shared selection's
    /// node when it is in the graph, else the node it was on, else the first node shown. Never changes the selection.
    public func reconcileNetworkFocus() {
        let graph = networkGraph
        var target: String?
        switch selection {
        case .work(let id)? where graph.node(id) != nil: target = id
        case .decision(let id)? where graph.node(id) != nil: target = id
        default: target = network.focus.flatMap { graph.node($0) != nil ? $0 : nil } ?? networkNodes.first?.identity
        }
        guard let id = target, let node = graph.node(id) else { return }
        if network.focus != id {
            network.focus = id
            network.walk = nil
            network.walked = nil
            revealNetworkFocus()
        }
        if page == .network, !network.editingFilter, focusTarget != "network.filter" { focusTarget = Self.element(forNode: node.key) }
    }

    public static func element(forNode key: String) -> String { "network.node.\(key)" }

    public var networkFocusedNode: NetworkNode? { network.focus.flatMap { networkGraph.node($0) } }

    /// What explain reported about the focused node, when it is the selected task and its answer is installed.
    public var networkReport: NetworkReport? {
        guard let id = network.focus, case .work(id)? = selection, let detail = snapshot.detail, detail.subject == .work(id), !detail.loading else { return nil }
        return detail.network
    }

    // MARK: Keys

    /// The one place a key press on the network is applied, from the window and from the suite alike.
    @discardableResult
    public func handleNetworkKey(_ input: KeyInput) -> NetworkKeyOutcome {
        guard page == .network, let intent = NetworkKeyDecoder.intent(for: input) else { return .ignored }
        if network.editingFilter, intent != .closeDetail { return .ignored }
        performNetwork(intent)
        return .handled(intent)
    }

    public func performNetwork(_ intent: NetworkIntent) {
        switch intent {
        case .moveUp: moveNetworkFocus(by: -1)
        case .moveDown: moveNetworkFocus(by: 1)
        case .first: moveNetworkFocus(to: 0)
        case .last: moveNetworkFocus(to: Int.max)
        case .select:
            if let node = networkFocusedNode { select(node.isDecision ? .decision(node.identity) : .work(node.identity)) }
        case .openDetail:
            if let node = networkFocusedNode {
                openDetail(for: node.isDecision ? .decision(node.identity) : .work(node.identity), from: Self.element(forNode: node.key), row: node.identity)
            }
        case .closeDetail:
            if network.editingFilter {
                network.editingFilter = false
                focusTarget = network.focus.flatMap { networkGraph.node($0) }.map { Self.element(forNode: $0.key) }
            }
        case .predecessor: walk("predecessor", networkWalk(.predecessor))
        case .successor: walk("successor", networkWalk(.successor))
        case .gate: walk("decision gate", networkWalk(.gate))
        case .blocker: walk("blocker", networkWalk(.blocker))
        case .zoomIn:
            networkOperation { state in
                if let fitted = state.fitScale {
                    state.zoomLevel = NetworkLayout.zoomSteps.firstIndex { $0 > fitted + 1e-9 } ?? NetworkLayout.zoomSteps.count - 1
                    state.fitScale = nil
                } else {
                    state.zoomLevel = min(NetworkLayout.zoomSteps.count - 1, state.zoomLevel + 1)
                }
            }
            revealNetworkFocus()
        case .zoomOut:
            let graph = networkGraph
            networkOperation { state in
                if let fitted = state.fitScale {
                    state.zoomLevel = NetworkLayout.zoomSteps.lastIndex { $0 < fitted - 1e-9 } ?? 0
                    // Below the smallest step the fitted scale stays: zooming out never loses the whole-graph view.
                    if NetworkLayout.zoomSteps[state.zoomLevel] < fitted { state.fitScale = nil }
                } else {
                    state.zoomLevel = max(0, state.zoomLevel - 1)
                }
                let most = NetworkLayout.maxPan(graph, scale: state.scale, viewport: (state.viewportWidth, state.viewportHeight))
                state.panX = min(most.x, state.panX)
                state.panY = min(most.y, state.panY)
            }
        case .fit:
            // The scale at which the whole graph, margins included, is inside the canvas on both axes, from its top
            // left: derived from the canvas and the extent, so a dense overview fits as well as a small plan.
            let scale = NetworkLayout.fitScale(networkGraph, viewport: (network.viewportWidth, network.viewportHeight))
            networkOperation { state in
                if let scale {
                    state.fitScale = scale
                    state.zoomLevel = NetworkLayout.zoomSteps.lastIndex { $0 <= scale } ?? 0
                }
                state.panX = 0
                state.panY = 0
            }
        case .locate:
            locateSelectedNode()
        case .resetView:
            networkOperation { state in
                state.zoomLevel = NetworkLayout.defaultZoom
                state.fitScale = nil
                state.panX = 0
                state.panY = 0
            }
            revealNetworkFocus()
        case .panLeft: panNetwork(dx: -Self.networkPanStep, dy: 0)
        case .panRight: panNetwork(dx: Self.networkPanStep, dy: 0)
        case .panUp: panNetwork(dx: 0, dy: -Self.networkPanStep)
        case .panDown: panNetwork(dx: 0, dy: Self.networkPanStep)
        case .toggleTemporal: toggleKind(.temporal)
        case .toggleBlocks: toggleKind(.decisionBlock)
        case .toggleContext: toggleKind(.context)
        case .clearFilter: networkOperation { $0.filter = NetworkFilter() }
        case .focusFilter:
            // A request only: the field's actual editor taking the keyboard reports `networkFilterFocus(true)`.
            focusTarget = "network.filter"
        }
    }

    /// The nodes a walk key reaches from the focused node. Predecessors and successors are the ends of its
    /// temporal edges (as explain lists them when its answer for this node is installed); a gate is a decision
    /// whose `blocks` names it; a blocker is a decision or predecessor named by explain's unmet gates.
    public func networkWalk(_ intent: NetworkIntent) -> [String] {
        guard let id = network.focus else { return [] }
        let graph = networkGraph
        let report = networkReport
        switch intent {
        case .predecessor:
            return report?.predecessors ?? graph.edges(into: id, kinds: [.temporal]).map(\.from)
        case .successor:
            return report?.successors ?? graph.edges(outOf: id, kinds: [.temporal]).map(\.to)
        case .gate:
            // The decision gates explain reports for this task, a gate inherited from a parent package included.
            // A context link never gates.
            guard let report = report else { return [] }
            let byKey = Dictionary(graph.nodes.filter(\.isDecision).map { ($0.key, $0.identity) }, uniquingKeysWith: { first, _ in first })
            return report.unmet.filter { $0.type == "decision" }.compactMap { $0.key.flatMap { byKey[$0] } }
        case .blocker:
            guard let report = report else { return [] }
            let byKey = Dictionary(graph.nodes.map { ($0.key, $0.identity) }, uniquingKeysWith: { first, _ in first })
            return report.unmet.compactMap { $0.key.flatMap { byKey[$0] } }
        default:
            return []
        }
    }

    /// Go to the next node of a walk. Pressing the same walk key again, still on the node it reached, goes to the
    /// next one from the same origin, so every predecessor (or successor, gate, blocker) is reached in turn.
    private func walk(_ name: String, _ found: [String]) {
        let graph = networkGraph
        var origin = networkFocusedNode
        var targets = found
        var at = 0
        if let walk = network.walk, walk.name == name, walk.target == network.focus, let from = graph.node(walk.origin) {
            origin = from
            targets = walk.targets
            at = (walk.index + 1) % max(1, walk.targets.count)
        }
        guard let from = origin else { return }
        guard !targets.isEmpty else {
            network.walked = "\(from.key) has no \(name)" + ((name == "blocker" || name == "decision gate") && networkReport == nil ? " reported yet: select it (Space) so explain is read" : "")
            return
        }
        let target = targets[at]
        guard let node = graph.node(target) else { return }
        var state = network
        state.walk = NetworkWalk(name: name, origin: from.identity, targets: targets, index: at, target: target)
        state.focus = target
        state.walked = "\(name) \(at + 1) of \(targets.count) of \(from.key): \(node.key), \(node.kindText), \(node.title)"
        network = state
        focusTarget = Self.element(forNode: node.key)
        revealNetworkFocus()
    }

    private func moveNetworkFocus(by delta: Int) {
        let nodes = networkNodes
        guard !nodes.isEmpty else { return }
        let current = network.focus.flatMap { id in nodes.firstIndex { $0.identity == id } }
        moveNetworkFocus(to: current.map { $0 + delta } ?? (delta > 0 ? 0 : nodes.count - 1))
    }

    private func moveNetworkFocus(to position: Int) {
        let nodes = networkNodes
        guard !nodes.isEmpty else { return }
        let node = nodes[max(0, min(nodes.count - 1, position))]
        network.focus = node.identity
        network.walked = nil
        focusTarget = Self.element(forNode: node.key)
        revealNetworkFocus()
    }

    /// Put the cursor on a node by pointer or by the list; the shared selection follows only when asked.
    public func focusNode(_ identity: String, selecting: Bool = false) {
        guard let node = networkGraph.node(identity) else { return }
        network.focus = identity
        network.walked = nil
        focusTarget = Self.element(forNode: node.key)
        revealNetworkFocus()
        if selecting { select(node.isDecision ? .decision(identity) : .work(identity)) }
    }

    /// Locate selected: the selected node (else the cursor node) under the cursor and in view at a readable scale. A
    /// selected node the filter hides is not panned to: the note says it is filtered out and how to show it, and the
    /// shared selection, the filter and the view stay as they are.
    private func locateSelectedNode() {
        let graph = networkGraph
        var target = network.focus
        switch selection {
        case .work(let id)? where graph.node(id) != nil: target = id
        case .decision(let id)? where graph.node(id) != nil: target = id
        default: break
        }
        guard let target, let node = graph.node(target) else { return }
        if let shown = networkShown, !shown.contains(target) {
            networkOperation { $0.locateNote = "\(node.key) is selected but the filter “\($0.filter.text)” hides it, so it cannot be shown: clear the filter (x) to locate it. The selection is kept." }
            return
        }
        network.focus = target
        focusTarget = Self.element(forNode: node.key)
        networkOperation { state in
            // From an overview too small to read, Locate returns to the default scale.
            if state.scale < NetworkLayout.scale(NetworkLayout.defaultZoom - 1) {
                state.zoomLevel = NetworkLayout.defaultZoom
                state.fitScale = nil
            }
            // The text list brings the node's row back as well, even when the cursor was already on it.
            state.listReveal = state.listReveal &+ 1
        }
        revealNetworkFocus()
    }

    // MARK: View operations

    private func networkOperation(_ change: (inout NetworkViewState) -> Void) {
        var state = network
        state.locateNote = nil
        change(&state)
        state.sequence = network.sequence &+ 1
        network = state
    }

    /// The node the resting pointer inspects now: hit against the current pan, scale and shown nodes.
    public var networkHover: NetworkNode? {
        guard let point = network.hoverPoint else { return nil }
        let state = network
        return NetworkLayout.hit(networkGraph, shown: networkNodes, at: (point.x, point.y), pan: (state.panX, state.panY), scale: state.scale)
    }

    /// Pan by points, bounded on both sides by the actual canvas and the scaled content, so the graph can never be lost.
    public func panNetwork(dx: Double, dy: Double) {
        let graph = networkGraph
        networkOperation { state in
            let most = NetworkLayout.maxPan(graph, scale: state.scale, viewport: (state.viewportWidth, state.viewportHeight))
            state.panX = max(0, min(most.x, state.panX + dx))
            state.panY = max(0, min(most.y, state.panY + dy))
        }
    }

    /// The canvas size the window gives the graph; the pan stays within the new bounds. A view fact, not an operation.
    public func setNetworkViewport(width: Double, height: Double) {
        guard width > 0, height > 0, abs(width - network.viewportWidth) > 0.5 || abs(height - network.viewportHeight) > 0.5 else { return }
        let first = network.viewportWidth <= 0
        var state = network
        state.viewportWidth = width
        state.viewportHeight = height
        // The pointer's resting point was measured in the old canvas: the inspection ends until it moves again.
        state.hoverPoint = nil
        let most = NetworkLayout.maxPan(networkGraph, scale: state.scale, viewport: (width, height))
        state.panX = max(0, min(most.x, state.panX))
        state.panY = max(0, min(most.y, state.panY))
        network = state
        if first { revealNetworkFocus() }
    }

    /// Scroll the cursor node into the canvas if it is outside it (ensure-visible), within the pan bounds.
    public func revealNetworkFocus() {
        guard let id = network.focus, let place = networkGraph.place[id], network.viewportWidth > 0 else { return }
        let scale = network.scale, r = NetworkLayout.rect(place), margin = 24.0
        let left = r.x * scale - margin, right = (r.x + r.width) * scale + margin
        let top = r.y * scale - margin, bottom = (r.y + r.height) * scale + margin
        var x = network.panX, y = network.panY
        if left < x { x = left } else if right > x + network.viewportWidth { x = right - network.viewportWidth }
        if top < y { y = top } else if bottom > y + network.viewportHeight { y = bottom - network.viewportHeight }
        let most = NetworkLayout.maxPan(networkGraph, scale: network.scale, viewport: (network.viewportWidth, network.viewportHeight))
        x = max(0, min(most.x, x))
        y = max(0, min(most.y, y))
        if x != network.panX || y != network.panY {
            network.panX = x
            network.panY = y
        }
    }

    /// Where the pointer rests on the canvas, or nil when it left: a temporary inspection that changes neither the
    /// cursor nor the selection.
    public func hoverNetwork(at point: NetworkPoint?) {
        if network.hoverPoint != point { network.hoverPoint = point }
    }

    private func toggleKind(_ kind: NetworkEdgeKind) {
        networkOperation { state in
            if state.filter.kinds.contains(kind) { state.filter.kinds.remove(kind) } else { state.filter.kinds.insert(kind) }
        }
    }

    public func setNetworkFilterText(_ text: String) {
        guard text != network.filter.text else { return }
        networkOperation { $0.filter.text = text }
    }

    /// The filter field says whether it has the keyboard, as the view reports it.
    public func networkFilterFocus(_ focused: Bool) {
        guard focused != network.editingFilter else { return }
        network.editingFilter = focused
        if focused { focusTarget = "network.filter" } else if focusTarget == "network.filter" { focusTarget = nil }
    }
}
