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
                        context.draw(Text(label.text).font(.caption2.monospacedDigit()), at: CGPoint(x: rect.midX, y: rect.midY))
                    }
                }
                if let terminals = graph.terminals { Self.drawTerminals(terminals, shown: Set(nodes.map(\.identity)), in: &context) }
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
                // The draft's links, over the nodes they join, until they are applied, removed or discarded.
                for link in model.networkDraftLinks { Self.drawDraft(link, in: &context) }
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

    static func label(_ edge: NetworkEdge) -> String? { edge.connectorLabel }

    /// Each edge kind has its own line and its own end mark, so none is told by colour alone: a solid line
    /// with an arrow (temporal; dashed when Soft), a heavy line ending in a bar (decision gate), a dotted line
    /// with no arrow (context, not blocking). The line follows the edge's route; every route enters its target
    /// horizontally from the left, at its own point of the side, so the end mark is its own too.
    static func drawEdge(_ edge: NetworkEdge, route: NetworkRoute, in context: inout GraphicsContext) {
        let start = CGPoint(x: route.start.x, y: route.start.y), end = CGPoint(x: route.end.x, y: route.end.y)
        var path = Path()
        path.move(to: start)
        for point in route.points.dropFirst() { path.addLine(to: CGPoint(x: point.x, y: point.y)) }
        switch edge.kind {
        case .temporal:
            let soft = edge.relation?.policy == "Soft"
            // A link of the critical path is heavier as well as coloured; its words say so too.
            let color = edge.critical ? Palette.relationCritical : Color.primary.opacity(0.55)
            context.stroke(path, with: .color(color), style: StrokeStyle(lineWidth: edge.critical ? 2.2 : 1.1, dash: soft ? [5, 3] : []))
            // The arrow points into the side it enters: right into a start, left into a finish.
            let back = route.entersFromLeft ? -7.0 : 7.0
            var head = Path()
            head.move(to: end)
            head.addLine(to: CGPoint(x: end.x + back, y: end.y - 4))
            head.addLine(to: CGPoint(x: end.x + back, y: end.y + 4))
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

    /// The Start and Finish markers and their thin connectors to the shown activities that have no temporal predecessor or
    /// successor. They are markers only; what they say is also in the text list and the summary line.
    static func drawTerminals(_ terminals: NetworkTerminals, shown: Set<String>, in context: inout GraphicsContext) {
        for (key, route) in terminals.routes {
            let id = key.hasPrefix("start>") ? String(key.dropFirst(6)) : String(key.dropLast(7))
            guard shown.contains(id) else { continue }
            var path = Path()
            path.move(to: CGPoint(x: route.start.x, y: route.start.y))
            for point in route.points.dropFirst() { path.addLine(to: CGPoint(x: point.x, y: point.y)) }
            context.stroke(path, with: .color(.secondary.opacity(0.7)), lineWidth: 0.8)
            let end = route.end
            var head = Path()
            head.move(to: CGPoint(x: end.x, y: end.y))
            head.addLine(to: CGPoint(x: end.x - 6, y: end.y - 3.5))
            head.addLine(to: CGPoint(x: end.x - 6, y: end.y + 3.5))
            head.closeSubpath()
            context.fill(head, with: .color(.secondary.opacity(0.7)))
        }
        let radius = NetworkLayout.terminalRadius
        for (point, word) in [(terminals.start, "Start"), (terminals.finish, "Finish")] {
            let circle = Path(ellipseIn: CGRect(x: point.x - radius, y: point.y - radius, width: radius * 2, height: radius * 2))
            context.fill(circle, with: .color(Color(nsColor: .controlBackgroundColor)))
            context.stroke(circle, with: .color(.secondary), lineWidth: 1.2)
            context.draw(Text(word).font(.caption2.weight(.semibold)), at: CGPoint(x: point.x, y: point.y))
        }
        context.draw(Text("0.0 h").font(.caption2.monospacedDigit()).foregroundColor(.secondary), at: CGPoint(x: terminals.start.x, y: terminals.start.y + radius + 8))
        // The finish and its percentiles, one per line under the marker so they fit the lane.
        let lines = terminals.finishWords.replacingOccurrences(of: "finish ", with: "").components(separatedBy: " · ")
        for (index, line) in lines.enumerated() {
            context.draw(Text(line).font(.caption2.monospacedDigit()).foregroundColor(.secondary), at: CGPoint(x: terminals.finish.x, y: terminals.finish.y + radius + 8 + Double(index) * 13))
        }
    }

    /// One drafted link: a dashed elbow connector with its arrowhead and a "draft" plate naming its kind.
    static func drawDraft(_ link: NetworkDraftLink, in context: inout GraphicsContext) {
        guard let tip = link.points.last, link.points.count > 1 else { return }
        var path = Path()
        path.addLines(link.points.map { CGPoint(x: $0.x, y: $0.y) })
        context.stroke(path, with: .color(Palette.relationEmphasis), style: StrokeStyle(lineWidth: 2, dash: [5, 3]))
        let before = link.points[link.points.count - 2]
        context.fill(GanttEditDrag.arrowhead(at: CGPoint(x: tip.x, y: tip.y), pointingRight: before.x < tip.x || (before.x == tip.x && link.kind.hasSuffix("Start"))),
                     with: .color(Palette.relationEmphasis))
        let middle = link.labelPoint
        let plate = Text("draft \(GanttRelation.abbreviation(link.kind))").font(.caption2.weight(.semibold)).foregroundColor(Palette.relationEmphasis)
        context.fill(Path(roundedRect: CGRect(x: middle.x - 26, y: middle.y - 7, width: 52, height: 14), cornerRadius: 3),
                     with: .color(Color(nsColor: .windowBackgroundColor).opacity(0.9)))
        context.draw(plate, at: CGPoint(x: middle.x, y: middle.y))
    }

    static func drawNode(_ node: NetworkNode, in rect: CGRect, focused: Bool, keyboard: Bool, context: inout GraphicsContext) {
        let outline: Color = node.critical == true ? Palette.critical : .secondary
        // Critical work is outlined in the critical colour and a little heavier, and says "critical" in its box.
        let weight = node.critical == true ? 1.6 : 1.0
        if focused { context.stroke(Path(rect.insetBy(dx: -4, dy: -4)), with: .color(keyboard ? .accentColor : .secondary), style: StrokeStyle(lineWidth: 2, dash: keyboard ? [] : [3, 2])) }
        switch node.kind {
        case .milestone:
            // A zero-duration event: a small diamond, with its key and time beside it.
            let size = 11.0, centre = CGPoint(x: rect.minX + size + 2, y: rect.midY)
            var diamond = Path()
            diamond.move(to: CGPoint(x: centre.x, y: centre.y - size))
            diamond.addLine(to: CGPoint(x: centre.x + size, y: centre.y))
            diamond.addLine(to: CGPoint(x: centre.x, y: centre.y + size))
            diamond.addLine(to: CGPoint(x: centre.x - size, y: centre.y))
            diamond.closeSubpath()
            context.fill(diamond, with: .color(Color(nsColor: .controlBackgroundColor)))
            context.stroke(diamond, with: .color(outline), lineWidth: weight)
            let at = node.earliestStart.map { " · at \(hoursText($0))" } ?? ""
            context.draw(Text(fitted(node.key + (node.critical == true ? " · critical" : ""), 30)).font(.caption.weight(.semibold)), at: CGPoint(x: centre.x + size + 6, y: rect.midY - 8), anchor: .leading)
            context.draw(Text(fitted("milestone\(at)", 32)).font(.caption2.monospacedDigit()).foregroundColor(.secondary), at: CGPoint(x: centre.x + size + 6, y: rect.midY + 8), anchor: .leading)
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
            context.fill(hexagon, with: .color(Color(nsColor: .controlBackgroundColor)))
            context.stroke(hexagon, with: .color(.secondary), lineWidth: 1)
            context.draw(Text(fitted(node.key, 30)).font(.caption.weight(.semibold)), at: CGPoint(x: rect.midX, y: rect.midY - 8))
            context.draw(Text(fitted(node.title, 32)).font(.caption2), at: CGPoint(x: rect.midX, y: rect.midY + 8))
        case .task, .package:
            let shape = Path(roundedRect: rect, cornerRadius: node.kind == .package ? 0 : 4)
            context.fill(shape, with: .color(Color(nsColor: .controlBackgroundColor)))
            context.stroke(shape, with: .color(outline), lineWidth: weight)
            if node.kind == .package { context.stroke(Path(rect.insetBy(dx: 3, dy: 3)), with: .color(.secondary), lineWidth: 1) }
            drawProgress(node, in: rect, context: &context)
            drawBox(node, in: rect, context: &context)
        }
    }

    /// A network diagram's progress marks, faint under the text: one diagonal once the work has started, two once it is
    /// verified. The status is also in words in the text list and the inspector.
    private static func drawProgress(_ node: NetworkNode, in rect: CGRect, context: inout GraphicsContext) {
        guard let marks = node.progressMarks, marks > 0 else { return }
        var lines = Path()
        lines.move(to: CGPoint(x: rect.minX + 2, y: rect.maxY - 2))
        lines.addLine(to: CGPoint(x: rect.maxX - 2, y: rect.minY + 2))
        if marks > 1 {
            lines.move(to: CGPoint(x: rect.minX + 2, y: rect.minY + 2))
            lines.addLine(to: CGPoint(x: rect.maxX - 2, y: rect.maxY - 2))
        }
        context.stroke(lines, with: .color(.secondary.opacity(0.4)), lineWidth: 1)
    }

    /// The activity-on-node box: earliest start, duration and earliest finish across the top, the key and title in the
    /// middle, latest start, total float and latest finish across the bottom. Without schedule times the box holds the
    /// key and title only.
    private static func drawBox(_ node: NetworkNode, in rect: CGRect, context: inout GraphicsContext) {
        let band = 18.0
        let name = node.key + (node.critical == true ? " · critical" : "")
        guard let cells = node.boxCells else {
            context.draw(Text(fitted(name, 34)).font(.caption.weight(.semibold)), at: CGPoint(x: rect.midX, y: rect.midY - 8))
            context.draw(Text(fitted(node.title, 36)).font(.caption2), at: CGPoint(x: rect.midX, y: rect.midY + 8))
            return
        }
        var rules = Path()
        for y in [rect.minY + band, rect.maxY - band] {
            rules.move(to: CGPoint(x: rect.minX, y: y))
            rules.addLine(to: CGPoint(x: rect.maxX, y: y))
        }
        for third in [1.0, 2.0] {
            let x = rect.minX + rect.width * third / 3
            rules.move(to: CGPoint(x: x, y: rect.minY))
            rules.addLine(to: CGPoint(x: x, y: rect.minY + band))
            rules.move(to: CGPoint(x: x, y: rect.maxY - band))
            rules.addLine(to: CGPoint(x: x, y: rect.maxY))
        }
        context.stroke(rules, with: .color(.secondary.opacity(0.35)), lineWidth: 0.5)
        for (index, cell) in cells.enumerated() {
            let column = Double(index % 3), top = index < 3
            let point = CGPoint(x: rect.minX + rect.width * (column + 0.5) / 3, y: top ? rect.minY + band / 2 : rect.maxY - band / 2)
            context.draw(Text(cell.label + " ").font(.caption2).foregroundColor(.secondary) + Text(cell.value).font(.caption2.monospacedDigit()), at: point)
        }
        context.draw(Text(fitted(name, 34)).font(.caption.weight(.semibold)), at: CGPoint(x: rect.midX, y: rect.minY + band + 8))
        context.draw(Text(fitted(node.title, 36)).font(.caption2), at: CGPoint(x: rect.midX, y: rect.minY + band + 21))
    }

    /// The text cut to `room` characters with an ellipsis; the full text is in the hover, the inspector and the list.
    static func fitted(_ text: String, _ room: Int) -> String {
        text.count > room ? String(text.prefix(room - 1)) + "…" : text
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
                        Prose(text: "Predecessors: \(names(report.predecessors)) · Successors: \(names(report.successors))", font: .caption)
                        Prose(text: report.unmet.isEmpty ? "Nothing is holding it back." : "Waiting: it " + report.unmet.map(\.words).joined(separator: "; it "), font: .caption)
                        if let reason = report.blockReason { Prose(text: "Blocked: \(reason)", font: .caption) }
                    } else if !node.isDecision {
                        Prose(text: "Select this node (Space) to read what explain reports: its predecessors, successors and unmet gates.", font: .caption)
                    }
                    Button("Open in Detail") { model.performNetwork(.openDetail) }
                        .help("Open the node in Detail (Return or ⌘D); closing Detail returns here")
                        .accessibilityIdentifier("network.openDetail")
                    DisclosureGroup("Keys and pointer") {
                        Prose(text: "Keys: ↑ ↓ move, Space selects, Return opens Detail; p, s, g, b walk to predecessors, successors, decision gates and blockers; 1 2 3 show or hide each edge kind; = and - zoom; f fits, l locates the selected node, 0 resets; Shift-arrows pan; / filters. Pointer: a click selects a node, a double click opens Detail, a drag on the background or a scroll pans, a pinch zooms; nothing moves a plan item. While editing, a drag from a task or milestone proposes a link from the half pressed (start or finish) to the half it is dropped on, reviewed before it applies.", font: .caption2)
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
