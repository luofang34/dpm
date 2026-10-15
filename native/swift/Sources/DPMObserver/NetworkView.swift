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
                            .overlay { if let link = model.network.linking { NetworkLinkView(link: link, model: model) } }
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
        .inspector(isPresented: Binding(get: { model.editing.enabled }, set: { model.setEditing($0) })) { EditPanel(model: model) }
        .sheet(isPresented: Binding(get: { model.editing.reviewing }, set: { if !$0 { model.closeReview() } }), onDismiss: { model.reviewDismissed() }) {
            ReviewSheet(model: model)
        }
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
                EditToggle(model: model)
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
            // One short line on the page; the complete sentence is the element's label and its tooltip.
            Text("\(shown) of \(counts.nodes) nodes · \(model.network.filter.words) · elapsed hours, no dates").font(.caption).foregroundStyle(.secondary).lineLimit(1).help(words)
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
