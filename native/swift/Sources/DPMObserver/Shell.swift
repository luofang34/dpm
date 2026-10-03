// The window: the status bar that always says what is shown and how current it is, a sidebar of
// the four views, and the view itself. Before a workspace is chosen it shows how to choose one.

import DPMObserverCore
import SwiftUI

struct RootView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        VStack(spacing: 0) {
            if model.hasWorkspace {
                StatusBar(model: model)
                Divider()
                NavigationSplitView {
                    Sidebar(model: model)
                        .navigationSplitViewColumnWidth(min: 150, ideal: 170, max: 220)
                } detail: {
                    PageView(model: model)
                }
            } else {
                WelcomeView(model: model)
            }
        }
        .toolbar { toolbar }
    }

    @ToolbarContentBuilder
    private var toolbar: some ToolbarContent {
        ToolbarItemGroup {
            Button { model.chooseWorkspace() } label: { Label("Open…", systemImage: "folder") }
                .help("Open a DPM project or database (⌘O). Nothing is created or changed.")
            Button { model.reload() } label: { Label("Read Again", systemImage: "arrow.clockwise") }
                .help("Read everything again, or open a lost source again (⌘R). Never changes the project.")
                .disabled(!model.hasWorkspace)
        }
    }
}

struct Sidebar: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        VStack(spacing: 0) {
            pages
            Divider()
            InspectionView(model: model)
        }
    }

    private var pages: some View {
        List(selection: Binding<ObserverModel.Page?>(get: { model.page }, set: { if let page = $0 { model.show(page) } })) {
            ForEach(ObserverModel.Page.allCases) { page in
                Label(page.rawValue, systemImage: page.symbol)
                    .tag(page)
                    .help("\(page.hint) (⌘\(String(describing: page.shortcut.character)))")
                    .accessibilityLabel("\(page.rawValue): \(page.hint)")
                    .accessibilityIdentifier("page.\(page.rawValue.lowercased())")
            }
        }
        .accessibilityIdentifier("sidebar")
    }
}

/// What each displayed view was anchored to and where each feed stands: for telling differently
/// anchored answers apart. Technical, so it stays closed until asked for.
struct InspectionView: View {
    @ObservedObject var model: ObserverModel
    @State private var open = false

    var body: some View {
        let inspection = model.snapshot.inspection
        DisclosureGroup("Observation basis", isExpanded: $open) {
            ScrollView {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Feeds followed").font(.caption.weight(.semibold))
                    ForEach(inspection.cursors, id: \.self) { Prose(text: $0, font: .caption2).spoken($0) }
                    Text("Views on display").font(.caption.weight(.semibold))
                    ForEach(inspection.views) { view in
                        let text = "\(view.slot): evaluated \(displayTime(view.evaluatedAt))\(view.refreshAt.map { ", changes by itself at \(displayTime($0))" } ?? "")\(view.project.map { ". Project: \($0)" } ?? "")\(view.runs.map { ". Runs: \($0)" } ?? "")"
                        Prose(text: text, font: .caption2).spoken(view.slot, value: String(text.dropFirst(view.slot.count + 2)))
                    }
                }
            }
            .frame(maxHeight: 220)
        }
        .padding(8)
        .font(.caption)
        .accessibilityIdentifier("inspection")
    }
}

struct PageView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        switch model.page {
        case .now: NowView(model: model)
        case .live: LiveView(model: model)
        case .review: ReviewView(model: model)
        case .detail: DetailView(model: model)
        }
    }
}

struct WelcomeView: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        VStack(spacing: 12) {
            Image(systemName: "waveform.path.ecg.rectangle").font(.system(size: 44)).foregroundStyle(.secondary).accessibilityHidden(true)
            Text("DPM Observer").font(.largeTitle.weight(.semibold))
            Text("Watch current work, the runs executing it, and what awaits independent review. This app only reads: it never changes a project, never creates one and never verifies work.")
                .multilineTextAlignment(.center).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            if let problem = model.setupError {
                Badge(text: problem, symbol: "xmark.octagon", tint: .red)
            }
            if case .failed(let message) = model.snapshot.connection {
                Badge(text: message, symbol: "xmark.octagon", tint: .red)
            }
            Button("Open Project or Database…") { model.chooseWorkspace() }
                .keyboardShortcut("o", modifiers: .command)
                .buttonStyle(.borderedProminent)
                .controlSize(.large)
                .disabled(model.engine == nil)
                .accessibilityIdentifier("welcome.open")
            Text("Choose a project directory that contains .dpm/project.toml (this repository's read-only preview is one), or a DPM store file.")
                .font(.callout).foregroundStyle(.secondary).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: 520)
        .padding(Layout.margin * 2)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("welcome")
    }
}
