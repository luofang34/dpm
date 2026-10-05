// The dependency network page: the filter and view controls, the complete text list of every node and
// edge (always present and first), then the graph canvas, whose native region is the network's keyboard
// stop, and the inspector of the node under the cursor. Everything shown is read from the shared model:
// nodes and edges from the snapshot, float and criticality from the schedule reading, predecessors,
// successors and unmet gates of the selected task from explain. The view computes only geometry.

import AppKit
import DPMObserverCore
import SwiftUI

struct NetworkView: View {
    @ObservedObject var model: ObserverModel
    @FocusState private var filterFocused: Bool

    var body: some View {
        let graph = model.networkGraph
        let nodes = model.networkNodes
        // The page takes the size the window offers, never one derived from its content: the reader has no ideal
        // size of its own (as the Gantt's body), so the complete text list scrolls in its own bounded region and the
        // graph gets a finite canvas. The plot is the primary area; the list and the inspector are bounded beside it.
        GeometryReader { proxy in
            let size = proxy.size
            let listWidth = min(380, max(230, size.width * 0.3))
            let inspectorHeight = min(190, max(96, size.height * 0.26))
            VStack(alignment: .leading, spacing: 0) {
                controls(graph, shown: nodes.count)
                    .reportFrame("network_controls")
                Divider()
                HStack(spacing: 0) {
                    NetworkList(model: model, graph: graph, nodes: nodes)
                        .frame(width: listWidth)
                        .frame(maxHeight: .infinity)
                        .reportFrame("network_list")
                    Divider()
                    VStack(spacing: 0) {
                        NetworkCanvas(model: model, graph: graph, nodes: nodes)
                            .frame(maxWidth: .infinity, maxHeight: .infinity)
                            .reportFrame("network_canvas")
                        Divider()
                        NetworkInspector(model: model, graph: graph)
                            .frame(height: inspectorHeight)
                            .reportFrame("network_inspector")
                    }
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
            }
            .frame(width: size.width, height: size.height, alignment: .topLeading)
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("network")
        .onAppear { model.reconcileNetworkFocus() }
        .onChange(of: graph.nodes.count) { model.reconcileNetworkFocus() }
        .onChange(of: model.focusTarget) {
            // The `/` request focuses the actual field; a node named while the field has the keyboard (Escape) is the
            // region's to take, and `filterFocused` follows that, as the Gantt's filter does.
            let node = model.focusTarget?.hasPrefix("network.node.") ?? false
            if model.focusTarget == "network.filter" { filterFocused = true } else if filterFocused, !node { filterFocused = false }
        }
    }

    private func controls(_ graph: NetworkGraph, shown: Int) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            if let note = basisNote {
                Badge(text: note, symbol: "exclamationmark.triangle", tint: .orange)
                    .accessibilityIdentifier("network.stale")
            }
            // Two short rows rather than one long one, so a narrow window keeps every control inside it.
            HStack(spacing: 6) {
                TextField(Self.prompt, text: Binding(get: { model.network.filter.text }, set: { model.setNetworkFilterText($0) }))
                    .textFieldStyle(.roundedBorder)
                    .frame(minWidth: 140, maxWidth: 280)
                    .focused($filterFocused)
                    .onChange(of: filterFocused) { _, focused in model.networkFilterFocus(focused) }
                    .accessibilityLabel("Network filter")
                    .accessibilityIdentifier("network.filter")
                    .reportFrame("network_filter")
                Button("Fit network") { model.performNetwork(.fit) }
                    .help("Fit the whole graph into the canvas (f): every node is inside it, at whatever scale that takes. Changes only the view.")
                    .accessibilityIdentifier("network.fit")
                Button("Locate selected") { model.performNetwork(.locate) }
                    .help("Put the cursor on the selected node and scroll it into view (l)")
                    .accessibilityIdentifier("network.locate")
                Button("Reset view") { model.performNetwork(.resetView) }
                    .help("Default zoom, scrolled to the cursor node (0)")
                    .accessibilityIdentifier("network.reset")
                Button { model.performNetwork(.zoomOut) } label: { Image(systemName: "minus.magnifyingglass") }
                    .help("Zoom out (-)").accessibilityLabel("Zoom out").accessibilityIdentifier("network.zoom-out")
                Text("\(Self.percent(model.network.scale)) %").font(.caption).monospacedDigit()
                    .accessibilityLabel("Zoom \(Self.percent(model.network.scale)) percent\(model.network.fitScale != nil ? ", fitted to the whole graph" : "")")
                Button { model.performNetwork(.zoomIn) } label: { Image(systemName: "plus.magnifyingglass") }
                    .help("Zoom in (=)").accessibilityLabel("Zoom in").accessibilityIdentifier("network.zoom-in")
                Spacer(minLength: 0)
            }
            HStack(spacing: 6) {
                ForEach(NetworkEdgeKind.allCases, id: \.self) { kind in
                    let on = model.network.filter.kinds.contains(kind)
                    Button { model.performNetwork(Self.toggle(kind)) } label: { Text("\(on ? "☑" : "☐") \(kind.cue) \(Self.short(kind))") }
                        .help("\(on ? "Hide" : "Show") \(kind.words) edges (\(Self.key(kind)))")
                        .accessibilityLabel("\(kind.words) edges")
                        .accessibilityValue(on ? "shown" : "hidden")
                        .accessibilityIdentifier("network.kind.\(kind.rawValue)")
                }
                Spacer(minLength: 0)
            }
            let counts = graph.counts
            let words = "Showing \(shown) of \(counts.nodes) nodes · \(model.network.filter.words). In the plan: \(counts.words). Hours are elapsed hours from the schedule query; no calendar date is supplied, so none is shown."
            // Two lines at most on the page; the complete sentence is the element's label and its tooltip.
            Text(words).font(.caption).foregroundStyle(.secondary).lineLimit(2).help(words)
                .accessibilityLabel(words)
                .accessibilityIdentifier("network.counts")
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 6)
    }

    static let prompt = "Filter nodes by key, title, status or kind"

    static func percent(_ scale: Double) -> String {
        let value = scale * 100
        return value >= 10 ? "\(Int(value.rounded()))" : String(format: "%.1f", value)
    }

    /// The stale or disconnected state, in words, beside the last reading that stays shown.
    private var basisNote: String? {
        let snapshot = model.snapshot
        switch snapshot.connection {
        case .connected:
            if snapshot.gantt == nil { return "The schedule reading is not installed yet: float and criticality are not shown until it is." }
            return snapshot.freshness.current ? nil : "Not current: a newer reading is owed. What is shown is the last reading."
        case .reconnecting(let attempt, _): return "Disconnected (attempt \(attempt)): the network shown is the last reading and is not current."
        case .sourceChanged: return "Source changed: the network shown is the previous source's last reading."
        default: return "Not connected: the network shown is the last reading, if any."
        }
    }

    static func toggle(_ kind: NetworkEdgeKind) -> NetworkIntent {
        switch kind {
        case .temporal: return .toggleTemporal
        case .decisionBlock: return .toggleBlocks
        case .context: return .toggleContext
        }
    }

    static func short(_ kind: NetworkEdgeKind) -> String {
        switch kind {
        case .temporal: return "Temporal"
        case .decisionBlock: return "Decision gates"
        case .context: return "Related (not blocking)"
        }
    }

    static func key(_ kind: NetworkEdgeKind) -> String {
        switch kind {
        case .temporal: return "1"
        case .decisionBlock: return "2"
        case .context: return "3"
        }
    }
}

// MARK: - The text alternative (its lines are `NetworkText`, in the core, the same projection the canvas draws)

struct NetworkList: View {
    @ObservedObject var model: ObserverModel
    let graph: NetworkGraph
    let nodes: [NetworkNode]

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 6) {
                    Text("Text list of the network (\(nodes.count) nodes)").font(.headline).accessibilityAddTraits(.isHeader)
                    let shown = model.networkShown
                    ForEach(nodes) { node in
                        let lines = NetworkText.lines(node, graph: graph, kinds: model.network.filter.kinds, shown: shown)
                        let focused = model.network.focus == node.identity
                        Button { model.focusNode(node.identity, selecting: true) } label: {
                            // A compact row to browse by (key, kind, status, title and how many relations of each kind);
                            // the cursor node's row is expanded to every line, complete and wrapped. Every row's
                            // accessibility label is all of its lines, whichever is shown.
                            VStack(alignment: .leading, spacing: 2) {
                                if focused {
                                    ForEach(Array(lines.enumerated()), id: \.offset) { index, line in
                                        Text(line).font(index == 0 ? .callout.weight(.semibold) : .caption)
                                            .fixedSize(horizontal: false, vertical: true)
                                            .frame(maxWidth: .infinity, alignment: .leading)
                                    }
                                } else {
                                    Text(lines[0]).font(.callout).lineLimit(2)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                    Text(Self.summary(lines)).font(.caption2).foregroundStyle(.secondary).lineLimit(1)
                                        .frame(maxWidth: .infinity, alignment: .leading)
                                }
                            }
                            .padding(.horizontal, 6)
                            .padding(.vertical, focused ? 6 : 3)
                            .overlay(RoundedRectangle(cornerRadius: 4).stroke(focused ? Color.accentColor : Color.secondary.opacity(0.3), lineWidth: focused ? 2 : 1))
                        }
                        .buttonStyle(.plain)
                        .id(node.identity)
                        .onAppear { reveal.appeared(node.identity, proxy) }
                        .onDisappear { reveal.realized.remove(node.identity) }
                        .accessibilityElement(children: .ignore)
                        .accessibilityLabel(lines.joined(separator: ". "))
                        .accessibilityAddTraits(focused ? [.isButton, .isSelected] : .isButton)
                        .accessibilityIdentifier(ObserverModel.element(forNode: node.key))
                    }
                }
                .padding(8)
            }
            // On entry the cursor node (the shared selection's node) is revealed at the top once the list is laid out; an
            // explicit Locate reveals it again, even on the unchanged cursor node. A cursor move centres its row.
            // Nothing else scrolls the list, so a person's own scroll stays where they put it.
            .onAppear {
                reveal.served = model.network.listReveal
                // One turn later the list has its size: the entry reveal is made (or, if the row it asked for has not
                // been laid out yet, asked again, which lays it out).
                DispatchQueue.main.async {
                    if let focus = model.network.focus, !reveal.entered || reveal.pending == focus { reveal.request(focus, proxy) }
                }
            }
            .onChange(of: model.network.focus) { _, focus in
                guard let focus else { return }
                if !reveal.entered || reveal.served != model.network.listReveal {
                    reveal.served = model.network.listReveal
                    reveal.request(focus, proxy)
                } else {
                    reveal.pending = nil
                    proxy.scrollTo(focus, anchor: .center)
                }
            }
            .onChange(of: model.network.listReveal) { _, request in
                guard reveal.served != request, let focus = model.network.focus else { return }
                reveal.served = request
                reveal.request(focus, proxy)
            }
        }
        .accessibilityIdentifier("network.list")
    }

    @State private var reveal = NetworkListReveal()

    /// How many lines of each relation a compact row holds, in words: the lines themselves are the row's label.
    static func summary(_ lines: [String]) -> String {
        let kinds: [(String, String, String)] = [("Predecessor:", "predecessor", "predecessors"), ("Successor:", "successor", "successors"),
                                                 ("Gated by:", "gated by a decision", "gated by decisions"), ("Gates:", "gate", "gates"),
                                                 ("Context:", "context link (not blocking)", "context links (not blocking)")]
        var parts: [String] = []
        for (prefix, one, many) in kinds {
            let count = lines.filter { $0.hasPrefix(prefix) }.count
            if count > 0 { parts.append(count == 1 ? "1 \(one)" : "\(count) \(many)") }
        }
        if lines.contains(where: { $0.hasPrefix("Blocked") }) { parts.append("blocked") }
        return parts.isEmpty ? "no relations shown" : parts.joined(separator: " · ")
    }
}

/// The text list's reveal of the cursor node's row: view state only, never the selection or the plan. A lazy list
/// places a row it has not laid out yet by an estimate, so a reveal asked for such a row is kept pending and made
/// again, once, when that row actually appears (is laid out); a row already laid out is revealed after the
/// current update. The row goes to the top, so a card taller than the list shows its title and scrolls for the rest.
/// Not observed: changing it redraws nothing.
@MainActor final class NetworkListReveal {
    /// Whether the entry reveal has been made.
    var entered = false
    /// The last `NetworkViewState.listReveal` request answered.
    var served = 0
    /// The row whose reveal waits for its layout.
    var pending: String?
    /// The rows the lazy list has laid out now.
    var realized: Set<String> = []

    func request(_ id: String, _ proxy: ScrollViewProxy) {
        entered = true
        if realized.contains(id) {
            pending = nil
            DispatchQueue.main.async { proxy.scrollTo(id, anchor: .top) }
        } else {
            pending = id
            proxy.scrollTo(id, anchor: .top)
        }
    }

    func appeared(_ id: String, _ proxy: ScrollViewProxy) {
        realized.insert(id)
        guard pending == id else { return }
        pending = nil
        DispatchQueue.main.async { proxy.scrollTo(id, anchor: .top) }
    }
}

// MARK: - The graph

struct NetworkCanvas: View {
    @ObservedObject var model: ObserverModel
    let graph: NetworkGraph
    let nodes: [NetworkNode]
    @State private var keyboard = false

    var body: some View {
        let state = model.network
        let visibleEdges = model.networkVisibleEdges
        let scale = state.scale
        let hovered = model.networkHover
        let focusWords = model.networkFocusedNode.map { node in
            ([NetworkText.lines(node, graph: graph, kinds: state.filter.kinds, shown: model.networkShown).joined(separator: ". ")] + [state.walked, state.locateNote].compactMap { $0 }).joined(separator: ". ")
        }
        ZStack {
            Canvas { context, size in
                context.translateBy(x: -state.panX, y: -state.panY)
                context.scaleBy(x: scale, y: scale)
                let visible = CGRect(x: state.panX / scale, y: state.panY / scale, width: size.width / scale, height: size.height / scale).insetBy(dx: -NetworkLayout.nodeWidth, dy: -NetworkLayout.nodeHeight)
                var candidates: [NetworkLabels.Candidate] = []
                // Each edge is drawn, culled and labelled on its one route (`NetworkRouting`).
                for edge in visibleEdges {
                    guard let route = graph.routes[edge.id] else { continue }
                    let b = route.bounds
                    guard CGRect(x: b.x, y: b.y, width: b.width, height: b.height).intersects(visible) else { continue }
                    Self.drawEdge(edge, route: route, in: &context)
                    if let text = Self.label(edge) {
                        let segment = route.labelSegment
                        candidates.append(NetworkLabels.Candidate(edge: edge.id, text: text, from: segment.from, to: segment.to,
                                                                  priority: edge.from == state.focus || edge.to == state.focus))
                    }
                }
                // The edge labels where they touch no node and no other label (the cursor node's first); below a readable
                // scale none is drawn. Every relation stays complete in the text list and the inspector.
                if scale >= NetworkCanvas.labelScale {
                    let boxes = nodes.compactMap { node -> (x: Double, y: Double, width: Double, height: Double)? in
                        guard let at = graph.place[node.identity] else { return nil }
                        return NetworkLayout.rect(at)
                    }
                    for label in NetworkLabels.place(candidates, nodes: boxes).placed {
                        let rect = CGRect(x: label.x, y: label.y, width: label.width, height: label.height)
                        guard rect.intersects(visible) else { continue }
                        context.fill(Path(roundedRect: rect, cornerRadius: 3), with: .color(Color(nsColor: .windowBackgroundColor).opacity(0.85)))
                        context.draw(Text(label.text).font(.system(size: 9)), at: CGPoint(x: rect.midX, y: rect.midY))
                    }
                }
                for node in nodes {
                    guard let at = graph.place[node.identity] else { continue }
                    let origin = NetworkLayout.origin(column: at.column, row: at.row)
                    let rect = CGRect(x: origin.x, y: origin.y, width: NetworkLayout.nodeWidth, height: NetworkLayout.nodeHeight)
                    guard rect.intersects(visible) else { continue }
                    Self.drawNode(node, in: rect, focused: node.identity == state.focus, keyboard: keyboard, context: &context)
                    if node.identity == hovered?.identity, node.identity != state.focus {
                        context.stroke(Path(roundedRect: rect.insetBy(dx: -2, dy: -2), cornerRadius: 6), with: .color(.accentColor.opacity(0.6)), lineWidth: 1.5)
                    }
                }
            }
            if let note = state.locateNote {
                VStack {
                    Badge(text: note, symbol: "line.3.horizontal.decrease.circle", tint: .orange)
                        .padding(8)
                        .accessibilityIdentifier("network.locate-note")
                    Spacer()
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .allowsHitTesting(false)
            }
            NetworkRegion(request: model.focusTarget, cursor: focusWords, model: model, onKeyboard: { holds in
                keyboard = holds
                // What the region's actual first-responder state says, for the state file (a sync aid, not proof of focus).
                model.reportFocus(holds ? model.focusTarget : (model.network.editingFilter ? "network.filter" : nil))
            })
            // The hover inspection: the full key and title of the node under the pointer, for as long as it is there.
            if let hovered {
                VStack {
                    Spacer()
                    Text("\(hovered.key), \(hovered.kindText): \(hovered.title)")
                        .font(.caption).lineLimit(3)
                        .padding(6)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                        .padding(8)
                        .allowsHitTesting(false)
                        .accessibilityIdentifier("network.hover")
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
        .clipped()
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("network.canvas")
    }

    /// Edge labels are drawn only from this scale up; below it they could not be read.
    static let labelScale = 0.55

    /// The short label of a temporal edge: its relation, its lead or lag, and Soft; nil for the other kinds (their line
    /// and end mark say what they are).
    static func label(_ edge: NetworkEdge) -> String? {
        guard edge.kind == .temporal, let relation = edge.relation else { return nil }
        return "\(relation.abbreviation)\(relation.lagHours == 0 ? "" : " " + GanttRelation.lagWords(relation.lagHours, basis: relation.lagBasis))\(relation.policy == "Soft" ? " Soft" : "")"
    }

    /// Each edge kind has its own line and its own end mark, so none is told by colour alone: a solid line
    /// with an arrow (temporal; dashed when Soft), a heavy line ending in a bar (decision gate), a dotted line
    /// with no arrow (context, not blocking). The line follows the edge's route; every route enters its target
    /// horizontally from the left, at its own point of the side, so the end mark is its own too.
    static func drawEdge(_ edge: NetworkEdge, route: NetworkRoute, in context: inout GraphicsContext) {
        let start = CGPoint(x: route.start.x, y: route.start.y), end = CGPoint(x: route.end.x, y: route.end.y)
        var path = Path()
        path.move(to: start)
        if route.detour {
            for point in route.points.dropFirst() { path.addLine(to: CGPoint(x: point.x, y: point.y)) }
        } else {
            path.addCurve(to: end, control1: CGPoint(x: start.x + NetworkRoute.reach, y: start.y), control2: CGPoint(x: end.x - NetworkRoute.reach, y: end.y))
        }
        switch edge.kind {
        case .temporal:
            let soft = edge.relation?.policy == "Soft"
            context.stroke(path, with: .color(.primary.opacity(0.7)), style: StrokeStyle(lineWidth: 1.2, dash: soft ? [5, 3] : []))
            var head = Path()
            head.move(to: end)
            head.addLine(to: CGPoint(x: end.x - 7, y: end.y - 4))
            head.addLine(to: CGPoint(x: end.x - 7, y: end.y + 4))
            head.closeSubpath()
            context.fill(head, with: .color(.primary.opacity(0.7)))
        case .decisionBlock:
            context.stroke(path, with: .color(.red.opacity(0.8)), style: StrokeStyle(lineWidth: 2.5))
            var bar = Path()
            bar.move(to: CGPoint(x: end.x - 3, y: end.y - 7))
            bar.addLine(to: CGPoint(x: end.x - 3, y: end.y + 7))
            context.stroke(bar, with: .color(.red.opacity(0.8)), lineWidth: 3)
        case .context:
            context.stroke(path, with: .color(.secondary), style: StrokeStyle(lineWidth: 1, dash: [1, 3]))
        }
    }

    static func drawNode(_ node: NetworkNode, in rect: CGRect, focused: Bool, keyboard: Bool, context: inout GraphicsContext) {
        let shape: Path
        switch node.kind {
        case .milestone:
            var diamond = Path()
            diamond.move(to: CGPoint(x: rect.midX, y: rect.minY))
            diamond.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            diamond.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
            diamond.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
            diamond.closeSubpath()
            shape = diamond
        case .decision:
            var hexagon = Path()
            let inset = 14.0
            hexagon.move(to: CGPoint(x: rect.minX + inset, y: rect.minY))
            hexagon.addLine(to: CGPoint(x: rect.maxX - inset, y: rect.minY))
            hexagon.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            hexagon.addLine(to: CGPoint(x: rect.maxX - inset, y: rect.maxY))
            hexagon.addLine(to: CGPoint(x: rect.minX + inset, y: rect.maxY))
            hexagon.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
            hexagon.closeSubpath()
            shape = hexagon
        case .task, .package:
            shape = Path(roundedRect: rect, cornerRadius: node.kind == .package ? 0 : 5)
        }
        context.fill(shape, with: .color(Color(nsColor: .controlBackgroundColor)))
        context.stroke(shape, with: .color(node.critical == true ? .orange : .secondary), lineWidth: node.critical == true ? 2.5 : 1)
        if node.kind == .package { context.stroke(Path(rect.insetBy(dx: 3, dy: 3)), with: .color(.secondary), lineWidth: 1) }
        if focused { context.stroke(Path(rect.insetBy(dx: -4, dy: -4)), with: .color(keyboard ? .accentColor : .secondary), style: StrokeStyle(lineWidth: 2, dash: keyboard ? [] : [3, 2])) }
        if node.kind == .milestone || node.kind == .decision {
            // Inside the diamond or hexagon the text is kept to the shape's interior and clipped to it, so no outline
            // crosses it; the full key and title are in the hover, the inspector, the list and the region's value.
            var inner = context
            inner.clip(to: shape)
            let room = node.kind == .milestone ? 20 : 26
            let key = node.key.count > room ? String(node.key.prefix(room - 1)) + "…" : node.key
            let title = node.title.count > room - 4 ? String(node.title.prefix(room - 5)) + "…" : node.title
            inner.draw(Text(key).font(.system(size: 9, weight: .semibold)), at: CGPoint(x: rect.midX, y: rect.midY - 6))
            inner.draw(Text(title).font(.system(size: 8)), at: CGPoint(x: rect.midX, y: rect.midY + 7))
            return
        }
        let title = node.title.count > 26 ? String(node.title.prefix(25)) + "…" : node.title
        context.draw(Text(node.key).font(.system(size: 10, weight: .semibold)), at: CGPoint(x: rect.midX, y: rect.minY + 13))
        context.draw(Text(title).font(.system(size: 9)), at: CGPoint(x: rect.midX, y: rect.minY + 29))
    }
}

/// The selected node's relations as the application reports them, and the way to Detail.
struct NetworkInspector: View {
    @ObservedObject var model: ObserverModel
    let graph: NetworkGraph

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 4) {
                if let node = model.networkFocusedNode {
                    Text("\(node.key): \(node.title)").font(.headline).fixedSize(horizontal: false, vertical: true)
                    if let walked = model.network.walked { Prose(text: walked, font: .caption).id(walked).accessibilityIdentifier("network.walked") }
                    if let note = model.network.locateNote { Prose(text: note, font: .caption).accessibilityIdentifier("network.locate-note-text") }
                    if let report = model.networkReport {
                        Prose(text: "As explain reports it — predecessors: \(names(report.predecessors)); successors: \(names(report.successors)).", font: .caption)
                        Prose(text: report.unmet.isEmpty ? "Unmet gates: none reported." : "Unmet gates: " + report.unmet.map(\.words).joined(separator: "; "), font: .caption)
                        if let reason = report.blockReason { Prose(text: "Blocked: \(reason)", font: .caption) }
                    } else if !node.isDecision {
                        Prose(text: "Select this node (Space) to read what explain reports: its predecessors, successors and unmet gates.", font: .caption)
                    }
                    Button("Open in Detail") { model.performNetwork(.openDetail) }
                        .help("Open the node in Detail (Return or ⌘D); closing Detail returns here")
                        .accessibilityIdentifier("network.openDetail")
                    DisclosureGroup("Keys and pointer") {
                        Prose(text: "Keys: ↑ ↓ move, Space selects, Return opens Detail; p, s, g, b walk to predecessors, successors, decision gates and blockers; 1 2 3 show or hide each edge kind; = and - zoom; f fits, l locates the selected node, 0 resets; Shift-arrows pan; / filters. Pointer: a click selects a node, a double click opens Detail, a drag on the background or a scroll pans, a pinch zooms; nothing moves a plan item.", font: .caption2)
                    }
                    .font(.caption)
                    .accessibilityIdentifier("network.keys")
                } else {
                    Prose(text: "No node under the cursor. Use the arrow keys in the graph, or choose a node in the text list.", font: .caption)
                }
            }
            .padding(8)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityIdentifier("network.inspector")
    }

    private func names(_ ids: [String]) -> String { ids.isEmpty ? "none" : ids.map { graph.name($0) }.joined(separator: ", ") }
}

// MARK: - The keyboard region

/// The native view over the graph: the network's keyboard stop, a real first responder of its window.
/// Whether the network holds the keyboard is whether this view is its window's actual first responder.
struct NetworkRegion: NSViewRepresentable {
    @Environment(\.hostObservation) private var host
    let request: String?
    let cursor: String?
    let model: ObserverModel
    let onKeyboard: (Bool) -> Void

    func makeNSView(context: Context) -> NetworkRegionView {
        let view = NetworkRegionView()
        view.onKeyboard = onKeyboard
        view.cursor = cursor
        view.request = request
        view.model = model
        view.host = host
        return view
    }

    func updateNSView(_ view: NetworkRegionView, context: Context) {
        view.onKeyboard = onKeyboard
        view.cursor = cursor
        view.model = model
        view.host = host
        // The ONE handoff path: a new node named as the element to focus (a walk, a move, a return from Detail, an
        // ordinary Escape in the filter) makes the region take the keyboard here, and nowhere else.
        guard view.request != request else { return }
        view.request = request
        if let request, request.hasPrefix("network.node.") { view.take("node_requested") }
    }
}

final class NetworkRegionView: NSView {
    var onKeyboard: ((Bool) -> Void)?
    var request: String?
    var cursor: String? {
        didSet { if cursor != oldValue, holdsKeyboard { NSAccessibility.post(element: self, notification: .valueChanged) } }
    }

    @MainActor private static let regions = NSHashTable<NetworkRegionView>.weakObjects()

    /// The network region in `window`, if that window shows the network.
    @MainActor static func region(in window: NSWindow?) -> NetworkRegionView? {
        guard let window else { return nil }
        return regions.allObjects.first { $0.window === window }
    }

    /// Whether the network takes this key event: only while its region in the event's window is that window's
    /// first responder, or, for Escape alone, while its filter is being typed in. An input method's marked text
    /// keeps every key, Escape included.
    @MainActor static func takes(_ event: NSEvent, input: KeyInput, model: ObserverModel) -> Bool {
        guard let window = event.window else { return false }
        if let editor = window.firstResponder as? NSTextView {
            if editor.hasMarkedText() { return false }
            // Only an ordinary Escape, and only while the window's ACTUAL first responder is the field editor of this
            // window's network filter field: the model's flag alone is not trusted.
            guard input.characters == "\u{1B}", !input.command, let region = region(in: window), filterField(of: editor) != nil else { return false }
            if !model.network.editingFilter { model.networkFilterFocus(true) }
            region.note("filter.escape", ["editor_is_filter": true])
            // The model names the cursor node; the region's update takes the keyboard for that request (one path).
            return model.handleNetworkKey(input) != .ignored
        }
        guard let region = region(in: window), region.holdsKeyboard else { return false }
        return model.handleNetworkKey(input) != .ignored
    }

    /// How `editor` is known to be the field editor of the network filter in its own window; nil if it is not.
    @MainActor static func filterField(of editor: NSTextView) -> String? {
        guard editor.isFieldEditor, let field = editor.delegate as? NSTextField, field.window === editor.window else { return nil }
        if field.accessibilityIdentifier() == "network.filter" || (field.accessibilityAttributeValue(.identifier) as? String) == "network.filter" { return "identifier" }
        return field.placeholderString == NetworkView.prompt ? "prompt" : nil
    }

    weak var model: ObserverModel?
    weak var host: HostObservation?
    private var events: [[String: Any]] = []
    private var sequence = 0
    private static let limit = 40
    private var dragOrigin: NSPoint?
    private var dragged = false
    private var tracking: NSTrackingArea?

    /// One bounded receipt of what this region really received and did, with its window's actual first responder.
    func note(_ event: String, _ facts: [String: Any] = [:]) {
        guard let host else { return }
        sequence &+= 1
        var entry = facts
        entry["seq"] = sequence
        entry["event"] = event
        entry["uptime"] = ProcessInfo.processInfo.systemUptime
        let responder = window?.firstResponder
        let editor = responder as? NSTextView
        let isFilter = editor.map { found in MainActor.assumeIsolated { Self.filterField(of: found) != nil } } ?? false
        entry["first_responder"] = ["class": responder.map { String(describing: type(of: $0)) } ?? "none", "is_region": responder === self,
                                    "is_filter_editor": isFilter] as [String: Any]
        events.append(entry)
        if events.count > Self.limit { events.removeFirst(events.count - Self.limit) }
        MainActor.assumeIsolated { host.noteChange() }
    }

    private func traced() -> [String: Any]? {
        guard let window else { return nil }
        return ["window": window.windowNumber, "holds_keyboard": holdsKeyboard, "bounds": [bounds.width, bounds.height], "events": events, "sequence": sequence, "limit": Self.limit]
    }

    /// A point of this view as canvas coordinates from the top left.
    private func canvasPoint(_ event: NSEvent) -> (x: Double, y: Double) {
        let p = convert(event.locationInWindow, from: nil)
        return (p.x, isFlipped ? p.y : bounds.height - p.y)
    }

    @MainActor private func node(at point: (x: Double, y: Double)) -> NetworkNode? {
        guard let model else { return nil }
        let state = model.network
        return NetworkLayout.hit(model.networkGraph, shown: model.networkNodes, at: point, pan: (state.panX, state.panY), scale: state.scale)
    }

    /// The window's actual field editor while it edits this window's network filter, read from the real text view on
    /// each sample: first responder, key window, string, marked text and the ranges. Nil when the filter is not being
    /// edited in this window; it reports what the editor holds and never a flag of its own.
    private func filterEditor() -> [String: Any]? {
        guard let window else { return nil }
        guard let editor = window.firstResponder as? NSTextView, editor.window === window,
              let by = MainActor.assumeIsolated({ Self.filterField(of: editor) }) else {
            return ["window": window.windowNumber, "current": NSNull()]
        }
        func range(_ value: NSRange) -> Any { value.location == NSNotFound ? NSNull() : [value.location, value.length] }
        let text = editor.string
        return ["window": window.windowNumber, "current": ["uptime": ProcessInfo.processInfo.systemUptime, "key_window": window.isKeyWindow, "identified_by": by,
                                                           "first_responder": true, "has_marked_text": editor.hasMarkedText(),
                                                           "marked_range": range(editor.markedRange()), "selected_range": range(editor.selectedRange()),
                                                           "string": String(text.prefix(256)), "length": (text as NSString).length] as [String: Any]]
    }

    var holdsKeyboard: Bool { window?.firstResponder === self }

    override var acceptsFirstResponder: Bool { true }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { NSBezierPath(rect: bounds).fill() }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Dependency network graph" }
    override func accessibilityValue() -> Any? { cursor ?? "no node under the cursor" }
    override func accessibilityHelp() -> String? {
        "Arrow keys move the cursor node, Space selects it, Return opens it in Detail; p, s, g and b walk to its predecessors, successors, decision gates and blockers. The text list beside the graph carries every node and edge in full."
    }
    override func accessibilityIdentifier() -> String { "network.region" }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    private func tell() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.note("settled", ["holds_keyboard": self.holdsKeyboard])
            self.onKeyboard?(self.holdsKeyboard)
        }
    }

    func take(_ reason: String) {
        guard let window, window.firstResponder !== self else { return }
        let accepted = window.makeFirstResponder(self)
        note("take." + reason, ["accepted": accepted])
    }

    // MARK: Pointer: a click selects the node under it, a double click opens Detail, a drag on the background or a
    // scroll pans, a pinch zooms. None of it moves or changes a plan item; all of it is view state and selection.

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        take("pointer")
        let point = canvasPoint(event)
        dragOrigin = event.locationInWindow
        dragged = false
        MainActor.assumeIsolated {
            guard let model else { return }
            let hit = node(at: point)
            if let hit {
                model.focusNode(hit.identity, selecting: true)
                if event.clickCount >= 2 { model.performNetwork(.openDetail) }
            }
            note("mouse_down", ["point": [point.x, point.y], "clicks": event.clickCount, "hit": hit?.key ?? NSNull(),
                                "selected": hit != nil, "opened_detail": hit != nil && event.clickCount >= 2])
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let origin = dragOrigin else { return }
        let now = event.locationInWindow
        let dx = now.x - origin.x, dy = now.y - origin.y
        guard dragged || abs(dx) + abs(dy) > 3 else { return }
        dragged = true
        dragOrigin = now
        // The content follows the pointer: the pan grows as the pointer moves left or up (window y grows upwards).
        MainActor.assumeIsolated { model?.panNetwork(dx: -dx, dy: dy) }
    }

    override func mouseUp(with event: NSEvent) {
        if dragged { MainActor.assumeIsolated { note("drag_end", ["pan": model.map { [$0.network.panX, $0.network.panY] } ?? []]) } }
        dragOrigin = nil
        dragged = false
    }

    override func scrollWheel(with event: NSEvent) {
        let scale = event.hasPreciseScrollingDeltas ? 1.0 : 10.0
        MainActor.assumeIsolated {
            model?.panNetwork(dx: -event.scrollingDeltaX * scale, dy: -event.scrollingDeltaY * scale)
            note("scroll", ["delta": [event.scrollingDeltaX, event.scrollingDeltaY], "precise": event.hasPreciseScrollingDeltas,
                            "pan": model.map { [$0.network.panX, $0.network.panY] } ?? []])
        }
    }

    override func magnify(with event: NSEvent) {
        guard abs(event.magnification) > 0.02 else { return }
        MainActor.assumeIsolated { model?.performNetwork(event.magnification > 0 ? .zoomIn : .zoomOut) }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseMoved(with event: NSEvent) {
        let point = canvasPoint(event)
        MainActor.assumeIsolated {
            let before = model?.networkHover?.key
            model?.hoverNetwork(at: NetworkPoint(x: point.x, y: point.y))
            let now = model?.networkHover?.key
            if now != before { note("hover", ["point": [point.x, point.y], "hit": now ?? NSNull()]) }
        }
    }

    override func mouseExited(with event: NSEvent) {
        MainActor.assumeIsolated { model?.hoverNetwork(at: nil) }
    }

    override func layout() {
        super.layout()
        let size = bounds.size
        MainActor.assumeIsolated {
            model?.setNetworkViewport(width: size.width, height: size.height)
            host?.noteChange()
        }
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil, let window, window.firstResponder === self { window.makeFirstResponder(nil) }
        super.viewWillMove(toWindow: newWindow)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else {
            MainActor.assumeIsolated { Self.regions.remove(self) }
            return
        }
        MainActor.assumeIsolated {
            Self.regions.add(self)
            host?.register("network_focus") { [weak self] in self?.traced() }
            host?.register("network_filter_editor") { [weak self] in self?.filterEditor() }
        }
        note("attached")
        window.recalculateKeyViewLoop()
        let responder = window.firstResponder
        // On entry the model names the cursor node (from the shared selection): the region takes the keyboard, from the
        // window or from the previous page's region, never from a text editor being typed in.
        let named = request?.hasPrefix("network.node.") ?? false
        if responder == nil || responder === window || responder === window.contentView || (named && !(responder is NSText)) { take("arrival") }
    }
}

/// The model the pointer's scroll pans; set once at launch, as the key monitor's is.
@MainActor enum NetworkPointer {
    static weak var model: ObserverModel?
}
