// Detail: one task, decision or run in full, beside a search over every task and decision.
// Titles, objectives, acceptance, requirements, decisions, dependencies, risks and references are
// wrapped, selectable text in a scrolling column that assistive technology reads in full.

import DPMObserverCore
import SwiftUI

struct DetailView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        HSplitView {
            WorkBrowser(model: model).frame(minWidth: 280, idealWidth: 340, maxWidth: 480)
            DetailContent(model: model).frame(minWidth: 420)
        }
    }
}

struct DetailContent: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        ScrollView {
            VStack(alignment: .leading, spacing: Layout.margin) {
                if let detail = snapshot.detail {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(detail.title).font(.title.weight(.semibold)).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                        if !detail.subtitle.isEmpty { Text(detail.subtitle).foregroundStyle(.secondary).textSelection(.enabled) }
                    }
                    .spoken(detail.title, value: detail.subtitle)
                    .accessibilityAddTraits(.isHeader)
                    if detail.loading { ProgressView("Reading from the application…") }
                    if let error = detail.error { Badge(text: error, symbol: "exclamationmark.triangle", tint: .orange) }
                    SectionsView(sections: detail.sections)
                    switch detail.subject {
                    case .work(let identity):
                        RelatedView(model: model, identity: identity)
                    case .decision(let id):
                        if let info = snapshot.inventory.decisionByIdentity[id] { GatedWork(model: model, decision: info) }
                    case .run(let id):
                        if let window = snapshot.window, window.run == id {
                            Text("Public activity (\(window.entries.count) shown of \(window.recorded))").font(.headline).accessibilityAddTraits(.isHeader)
                            if let gap = window.gap { Badge(text: gap, symbol: "exclamationmark.triangle", tint: .orange) }
                            if let incomplete = window.incomplete { Badge(text: incomplete, symbol: "exclamationmark.triangle", tint: .orange) }
                            ForEach(window.entries.reversed()) { entry in ActivityRow(entry: entry) }
                        }
                    }
                } else {
                    EmptyState(symbol: "doc.text.magnifyingglass", title: "Nothing selected", message: "Search for any task or decision on the left, or select one in Now, Live or Review. Reading never changes the project.")
                }
            }
            .padding(Layout.margin)
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .accessibilityIdentifier("detail.scroll")
    }
}

/// The work a decision gates, each openable by the keyboard.
struct GatedWork: View {
    @ObservedObject var model: ObserverModel
    let decision: DecisionInfo

    var body: some View {
        let inventory = model.snapshot.inventory
        VStack(alignment: .leading, spacing: Layout.rowSpacing) {
            Text("Open the work it holds back").font(.headline).accessibilityAddTraits(.isHeader)
            if decision.blocks.isEmpty { Text("This decision gates no work.").foregroundStyle(.secondary) }
            ForEach(decision.blocks.prefix(60), id: \.self) { identity in
                if let item = inventory.byIdentity[identity] {
                    Button { model.select(.work(identity)) } label: {
                        Text("\(item.key) — \(item.title) (\(item.status))").fixedSize(horizontal: false, vertical: true).multilineTextAlignment(.leading)
                    }
                    .buttonStyle(.link)
                    .accessibilityLabel("Open \(item.key)")
                    .accessibilityValue("\(item.title), \(item.status)")
                }
            }
            if decision.blocks.count > 60 { Text("\(decision.blocks.count - 60) more are found with the search.").font(.caption).foregroundStyle(.secondary) }
        }
        .accessibilityIdentifier("decision.gated")
    }
}
