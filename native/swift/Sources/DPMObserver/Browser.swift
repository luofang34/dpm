// Finding any task or decision by key, title, status or owner, so no work is out of reach because
// it is not ranked, has no run or is held back by a decision. Rendering is bounded: at most
// `shown` matches are drawn, and the footer says how many more there are. Selection is by
// persistent identity and every row is reachable with the keyboard.

import DPMObserverCore
import SwiftUI

struct WorkBrowser: View {
    @ObservedObject var model: ObserverModel
    @State private var query = ""
    @State private var status = "Any status"
    @FocusState private var searching: Bool

    /// Most rows drawn at once; the rest are reached by narrowing the search.
    static let shown = 100

    var body: some View {
        let inventory = model.snapshot.inventory
        let statuses = ["Any status"] + inventory.statuses
        let result = inventory.search(query, status: status == "Any status" ? nil : status, limit: Self.shown)
        let found = result.items
        let decisions = inventory.decisions(matching: query)
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 6) {
                TextField("Find a task or decision", text: $query)
                    .textFieldStyle(.roundedBorder)
                    .focused($searching)
                    .accessibilityLabel("Find a task or decision")
                    .accessibilityHint("Matches key, title, status and owner")
                    .accessibilityIdentifier("browser.search")
                Picker("Status", selection: $status) {
                    ForEach(statuses, id: \.self) { Text($0).tag($0) }
                }
                .accessibilityIdentifier("browser.status")
            }
            .padding(Layout.margin / 2)
            Divider()
            List(selection: model.selectionBinding) {
                Section("Tasks (\(found.count) of \(result.total))") {
                    ForEach(found) { item in
                        BrowserRow(item: item).tag(Subject.work(item.identity))
                    }
                }
                if !decisions.isEmpty {
                    Section("Decisions (\(decisions.count))") {
                        ForEach(decisions.prefix(Self.shown)) { decision in
                            VStack(alignment: .leading, spacing: 1) {
                                Text(decision.key).font(.system(.callout, design: .monospaced))
                                Text("\(decision.status) — \(decision.question)").font(.caption).foregroundStyle(.secondary).lineLimit(2)
                            }
                            .spoken("Decision \(decision.key): \(decision.status)", value: decision.question)
                            .tag(Subject.decision(decision.identity))
                        }
                    }
                }
            }
            .accessibilityIdentifier("browser.list")
            if result.total > found.count {
                Text("Showing the first \(found.count) of \(result.total) tasks. Narrow the search to reach the rest.")
                    .font(.caption).foregroundStyle(.secondary).padding(6).fixedSize(horizontal: false, vertical: true)
            }
        }
        // A request made on the same update that created this view is waiting, not missed.
        .onAppear { takeFocusRequest() }
        .onChange(of: model.searchRequest) { takeFocusRequest() }
        .onChange(of: searching) { model.searchFocus(searching) }
    }

    private func takeFocusRequest() {
        guard model.consumeSearchFocus() else { return }
        DispatchQueue.main.async { searching = true }
    }
}

struct BrowserRow: View {
    let item: WorkItem

    var body: some View {
        VStack(alignment: .leading, spacing: 1) {
            HStack(spacing: 6) {
                Text(item.key).font(.system(.callout, design: .monospaced))
                Text(item.status).font(.caption).foregroundStyle(.secondary)
            }
            Text(item.title).font(.callout).fixedSize(horizontal: false, vertical: true)
        }
        .spoken("\(item.key): \(item.title)", value: "\(item.status). Owner \(item.owner ?? "none").")
        .accessibilityIdentifier("browser.row.\(item.key)")
    }
}
