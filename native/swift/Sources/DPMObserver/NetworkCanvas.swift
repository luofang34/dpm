// The network's graph canvas and the inspector of the node under the cursor. Nodes, edges, float and
// criticality are the shared model's; the canvas computes only geometry, and the inspector shows a task's
// predecessors, successors and unmet gates as explain reports them.

import DPMObserverCore
import SwiftUI

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
                // While measuring, every draw pass says what it drew; the network workload pairs requests with these.
                if MeasureLog.shared != nil {
                    Measure.event("network_draw", detail: ["nodes": graph.nodes.count, "focus": state.focus.flatMap { graph.node($0)?.key } ?? "",
                                                           "pan_x": state.panX, "pan_y": state.panY])
                }
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
            NetworkRegion(request: model.focusTarget, returns: model.editing.focusReturns, cursor: focusWords, model: model, onKeyboard: { holds in
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
            // A link of the critical path is heavier as well as coloured; its words say so too.
            let color = edge.critical ? Palette.relationCritical : Color.primary.opacity(0.55)
            context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: edge.critical ? 2.2 : 1.1, dash: soft ? [5, 3] : []))
            var head = Path()
            head.move(to: end)
            head.addLine(to: CGPoint(x: end.x - 7, y: end.y - 4))
            head.addLine(to: CGPoint(x: end.x - 7, y: end.y + 4))
            head.closeSubpath()
            context.fill(head, with: .color(color))
        case .decisionBlock:
            // A decision that is no longer open gates nothing: a thin dashed line, and its words say so.
            let color = edge.released ? Palette.gate.opacity(0.45) : Palette.gate
            context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: edge.released ? 1 : 2.5, dash: edge.released ? [4, 3] : []))
            var bar = Path()
            bar.move(to: CGPoint(x: end.x - 3, y: end.y - 7))
            bar.addLine(to: CGPoint(x: end.x - 3, y: end.y + 7))
            context.stroke(bar, with: .color(color), lineWidth: edge.released ? 1.5 : 3)
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
        context.stroke(shape, with: .color(node.critical == true ? Palette.critical : .secondary), lineWidth: node.critical == true ? 2.5 : 1)
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
        guard let times = timesLine(node) else {
            context.draw(Text(node.key).font(.system(size: 10, weight: .semibold)), at: CGPoint(x: rect.midX, y: rect.minY + 13))
            context.draw(Text(title).font(.system(size: 9)), at: CGPoint(x: rect.midX, y: rect.minY + 29))
            return
        }
        // An activity-on-node box: key, title, then earliest start, latest start and total float.
        context.draw(Text(node.key).font(.system(size: 10, weight: .semibold)), at: CGPoint(x: rect.midX, y: rect.minY + 10))
        context.draw(Text(title).font(.system(size: 9)), at: CGPoint(x: rect.midX, y: rect.minY + 23))
        context.draw(Text(times).font(.system(size: 8).monospacedDigit()).foregroundColor(.secondary), at: CGPoint(x: rect.midX, y: rect.minY + 35))
    }

    /// "ES 9.8 h · LS 12.0 h · TF 2.2 h" for scheduled work, or "critical · ES 9.8 h · TF 0.0 h" on the critical
    /// path (where LS equals ES), so a critical node says so in words and not by its outline alone; nil when the
    /// projection gave no times.
    static func timesLine(_ node: NetworkNode) -> String? {
        guard let early = node.earliestStart, let late = node.latestStart, let float = node.totalFloat else { return nil }
        if node.critical == true { return "critical · ES \(hoursText(early)) · TF \(hoursText(float))" }
        return "ES \(hoursText(early)) · LS \(hoursText(late)) · TF \(hoursText(float))"
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
