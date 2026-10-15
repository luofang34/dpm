// The network page's text alternative: every node and edge of the graph as a list, always present and
// first, so the whole network reads without the canvas. Its lines are `NetworkText`, the core projection the
// canvas draws too, and revealing the cursor's row is view state only, never the selection or the plan.

import DPMObserverCore
import SwiftUI

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
                            // The cursor row is set apart by a fill, its expanded lines and its heavier first line, never by
                            // a box: no row carries an outline, so the list reads as one surface, as the Gantt's names do.
                            .background(focused ? Color.accentColor.opacity(0.16) : Color.clear, in: RoundedRectangle(cornerRadius: 5))
                            .contentShape(Rectangle())
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
