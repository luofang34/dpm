// The window: its title names the page shown and its subtitle the revision and how current it is; a bar
// under the toolbar appears only when something is not normal (a read-only source, a lost connection, a
// failed read). A sidebar of the six views in two groups, and the view itself. Before a workspace is
// chosen it shows how to choose one.

import DPMObserverCore
import SwiftUI

struct RootView: View {
    @ObservedObject var model: ObserverModel
    /// The subtitle's clock: the age it states is rewritten once a second, and nothing else is.
    @State private var now = Date()
    /// One timer for the process: a publisher made per view value would restart whenever the window is rebuilt.
    private static let clock = Timer.publish(every: 1, on: .main, in: .common).autoconnect()

    var body: some View {
        VStack(spacing: 0) {
            if model.hasWorkspace {
                StatusBar(model: model).reportFrame("status")
                if StatusBar.hasNews(model.snapshot) { Divider() }
                NavigationSplitView {
                    Sidebar(model: model)
                        .navigationSplitViewColumnWidth(min: 150, ideal: 170, max: 220)
                } detail: {
                    PageView(model: model)
                        .reportFrame("page")
                        .navigationTitle(model.page.rawValue)
                        .navigationSubtitle(Text(StatusBar.subtitle(model.snapshot, now: now)).monospacedDigit())
                }
            } else {
                WelcomeView(model: model)
            }
        }
        .reportFrame("root")
        .toolbar { toolbar }
        // During a scripted measurement the age is not ticked, so nothing but what is measured redraws the window.
        .onReceive(Self.clock) { if MeasureLog.shared == nil { now = $0 } }
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

    /// The pages in two groups: the plan as it stands, and the work being executed and reviewed. The order inside a
    /// group, and every page's ⌘ shortcut, are unchanged.
    static let plan: [ObserverModel.Page] = [.now, .gantt, .network, .detail]
    static let execution: [ObserverModel.Page] = [.live, .review]

    var body: some View {
        VStack(spacing: 0) {
            pages
            Divider()
            RevisionLine(model: model)
            InspectionView(model: model)
        }
        .reportFrame("sidebar")
    }

    private var pages: some View {
        List(selection: Binding<ObserverModel.Page?>(get: { model.page }, set: { if let page = $0 { model.show(page) } })) {
            Section("Plan") {
                ForEach(Self.plan) { page in row(page) }
            }
            Section("Execution") {
                ForEach(Self.execution) { page in row(page) }
            }
        }
        .accessibilityLabel("Pages")
        .accessibilityIdentifier("sidebar")
    }

    /// One page: its symbol and name, the count awaiting review on Review, and a quiet working indicator on Live while
    /// a run reports working. Each is also said in words, in the tooltip and to assistive technology.
    private func row(_ page: ObserverModel.Page) -> some View {
        let working = page == .live ? model.snapshot.runs.filter { $0.status == "working" }.count : 0
        let waiting = page == .review ? (model.snapshot.status?.awaitingVerification ?? 0) : 0
        let note = working > 0 ? " · \(working) working" : (waiting > 0 ? " · \(waiting) awaiting review" : "")
        return HStack(spacing: 6) {
            Label {
                Text(page.rawValue)
            } icon: {
                Image(systemName: page.symbol).symbolRenderingMode(.hierarchical)
            }
            if working > 0 {
                Spacer(minLength: 4)
                QuietDots()
            }
        }
        .badge(waiting)
        .tag(page)
        .help("\(page.hint)\(note) (⌘\(String(describing: page.shortcut.character)))")
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(page.rawValue): \(page.hint)")
        .accessibilityValue(note.isEmpty ? "" : String(note.dropFirst(3)))
        .accessibilityIdentifier("page.\(page.rawValue.lowercased())")
    }
}

/// The revision on display, with the workspace and lineage identities in its tooltip and its accessibility label: the
/// window subtitle says the same revision, and this keeps it as an element of its own.
struct RevisionLine: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        if let words = StatusBar.identityWords(snapshot) {
            Text("Revision \(snapshot.revision.map { String($0) } ?? "unknown")")
                .font(.caption)
                .monospacedDigit()
                .foregroundStyle(.secondary)
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(.horizontal, 8)
                .padding(.top, 6)
                .help(words)
                .accessibilityLabel(words)
                .accessibilityIdentifier("status.revision")
        }
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
                    if let words = StatusBar.identityWords(model.snapshot) {
                        Text("Source").font(.caption.weight(.semibold))
                        Prose(text: words, font: .caption2).textSelection(.enabled).spoken("Source", value: words)
                    }
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
        case .gantt: GanttView(model: model)
        case .network: NetworkView(model: model)
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
