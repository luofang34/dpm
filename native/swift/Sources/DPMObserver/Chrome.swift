// Shared pieces of the interface: status text paired with symbols (never colour alone), the status
// bar that always says what is shown and how current it is, and wrapped, scrollable text.

import DPMObserverCore
import SwiftUI

extension ObserverModel.Page {
    var symbol: String {
        switch self {
        case .now: return "bolt.circle"
        case .live: return "waveform.path.ecg"
        case .review: return "checkmark.seal"
        case .detail: return "doc.text.magnifyingglass"
        case .gantt: return "chart.bar.xaxis"
        case .network: return "point.3.connected.trianglepath.dotted"
        }
    }

    var shortcut: KeyEquivalent {
        switch self {
        case .now: return "1"
        case .live: return "2"
        case .review: return "3"
        case .detail: return "4"
        case .gantt: return "5"
        case .network: return "6"
        }
    }

    var hint: String {
        switch self {
        case .now: return "What to do now and why"
        case .live: return "Runs and their public activity"
        case .review: return "Work awaiting independent verification"
        case .detail: return "One task or run in full"
        case .gantt: return "The plan on a timeline: hierarchy, bars, float and relations"
        case .network: return "The dependency network: tasks, milestones and decision gates with every edge, and its full text list"
        }
    }
}

enum Layout {
    static let margin: CGFloat = 16
    static let rowSpacing: CGFloat = 4
}

func displayTime(_ instant: ObserverInstant?) -> String {
    guard let instant = instant else { return "unknown" }
    return instant.date.formatted(date: .abbreviated, time: .standard)
}

func age(_ date: Date, now: Date) -> String {
    let seconds = max(0, Int(now.timeIntervalSince(date)))
    if seconds < 2 { return "just now" }
    if seconds < 120 { return "\(seconds) s ago" }
    return "\(seconds / 60) min ago"
}

extension View {
    /// One accessibility element that says `label` and, when there is one, its `value` in full, so
    /// the whole text reaches assistive technology and not only the first caption of a group.
    func spoken(_ label: String, value: String? = nil) -> some View {
        accessibilityElement(children: .ignore)
            .accessibilityLabel(label)
            .accessibilityValue(value ?? "")
    }
}

/// A word with a symbol and a tint: the tint repeats what the text already says.
struct Badge: View {
    let text: String
    let symbol: String
    var tint: Color = .secondary

    var body: some View {
        Label(text, systemImage: symbol)
            .font(.caption.weight(.medium))
            .padding(.horizontal, 6)
            .padding(.vertical, 2)
            .background(tint.opacity(0.15), in: RoundedRectangle(cornerRadius: 5))
            .foregroundStyle(tint)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(text)
    }
}

struct EmptyState: View {
    let symbol: String
    let title: String
    let message: String

    var body: some View {
        VStack(spacing: 8) {
            Image(systemName: symbol).font(.largeTitle).foregroundStyle(.secondary).accessibilityHidden(true)
            Text(title).font(.headline)
            Text(message).foregroundStyle(.secondary).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: 420)
        .padding(Layout.margin * 2)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .accessibilityElement(children: .combine)
    }
}

/// Text that wraps to the width it is given and can be selected: nothing is cut for space.
struct Prose: View {
    let text: String
    var font: Font = .body

    var body: some View {
        Text(text)
            .font(font)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity, alignment: .leading)
            .textSelection(.enabled)
    }
}

// MARK: - Source, connection and freshness

struct StatusBar: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        let snapshot = model.snapshot
        VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 8) {
                sourceBadge(snapshot)
                connectionBadge(snapshot.connection)
                if snapshot.freshness.current, snapshot.connection.isLive {
                    Badge(text: "Current", symbol: "checkmark.circle", tint: .green)
                } else if snapshot.connection.isLive {
                    Badge(text: staleWords(snapshot.freshness), symbol: "exclamationmark.triangle", tint: .orange)
                }
                if let words = Self.identityWords(snapshot) {
                    // The revision stays in view; the workspace and lineage identities are in its tooltip, its
                    // accessibility label and the Observation basis details.
                    Text("Revision \(snapshot.revision.map { String($0) } ?? "unknown")")
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .help(words)
                        .accessibilityLabel(words)
                        .accessibilityIdentifier("status.revision")
                }
                Spacer()
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(freshnessLine(snapshot, now: context.date))
                        .font(.caption)
                        .foregroundStyle(.secondary)
                        .accessibilityLabel(freshnessLine(snapshot, now: context.date))
                }
            }
            ConnectionBanner(model: model)
        }
        .padding(.horizontal, Layout.margin)
        .padding(.vertical, 8)
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("status.bar")
    }

    static func identityWords(_ snapshot: ObserverSnapshot) -> String? {
        snapshot.identity.map { "Workspace \($0.workspaceId) · lineage \($0.lineageId ?? "none (preview)") · revision \(snapshot.revision.map { String($0) } ?? "unknown")" }
    }

    private func sourceBadge(_ snapshot: ObserverSnapshot) -> some View {
        switch snapshot.identity?.source {
        case "preview"?: return Badge(text: "Read-only preview", symbol: "eye", tint: .blue)
        case "live"?: return Badge(text: "Live workspace", symbol: "externaldrive.connected.to.line.below", tint: .green)
        case "archive"?: return Badge(text: "Archive (read-only)", symbol: "archivebox", tint: .brown)
        case let other?: return Badge(text: other, symbol: "questionmark.circle")
        case nil: return Badge(text: "No workspace", symbol: "tray")
        }
    }

    private func connectionBadge(_ state: ConnectionState) -> some View {
        switch state {
        case .idle, .closed: return Badge(text: "Not open", symbol: "circle.dashed")
        case .opening: return Badge(text: "Opening", symbol: "hourglass", tint: .blue)
        case .connected: return Badge(text: "Connected", symbol: "bolt.horizontal.circle", tint: .green)
        case .reconnecting(let attempt, _): return Badge(text: "Reconnecting (attempt \(attempt))", symbol: "arrow.triangle.2.circlepath", tint: .orange)
        case .sourceChanged: return Badge(text: "Source changed", symbol: "arrow.triangle.branch", tint: .red)
        case .failed: return Badge(text: "Failed", symbol: "xmark.octagon", tint: .red)
        }
    }

    private func staleWords(_ freshness: Freshness) -> String {
        var reasons: [String] = []
        if freshness.projectStale { reasons.append("project changed") }
        if freshness.runsStale { reasons.append("runs changed") }
        if freshness.clockDue { reasons.append("time moved on") }
        if freshness.lastError != nil { reasons.append("last read failed") }
        return "Refreshing: " + (reasons.isEmpty ? "waiting" : reasons.joined(separator: ", "))
    }

    private func freshnessLine(_ snapshot: ObserverSnapshot, now: Date) -> String {
        guard let received = snapshot.freshness.receivedAt else { return "Nothing read yet" }
        let evaluated = snapshot.freshness.evaluatedAt.map { " · evaluated \(displayTime($0))" } ?? ""
        return "Updated \(age(received, now: now))\(evaluated)"
    }
}

struct ConnectionBanner: View {
    @ObservedObject var model: ObserverModel

    var body: some View {
        switch model.snapshot.connection {
        case .reconnecting(let attempt, let cause):
            banner("arrow.triangle.2.circlepath", "The connection to the application was lost (attempt \(attempt)). What is shown is as old as the loss and is not current. \(cause)", tint: .orange, action: nil)
        case .sourceChanged(let message):
            banner("arrow.triangle.branch", "The project locator now selects another source, so nothing new is shown. What is displayed is the previous source. \(message)", tint: .red, action: ("Open the source again", { model.reload() }))
        case .failed(let message):
            banner("xmark.octagon", "The workspace could not be opened or followed: \(message)", tint: .red, action: ("Try again", { model.reload() }))
        default:
            if let error = model.snapshot.freshness.lastError {
                banner("exclamationmark.triangle", "The last read failed and will be retried: \(error)", tint: .orange, action: nil)
            }
        }
    }

    private func banner(_ symbol: String, _ text: String, tint: Color, action: (String, () -> Void)?) -> some View {
        HStack(alignment: .top, spacing: 8) {
            Image(systemName: symbol).foregroundStyle(tint).accessibilityHidden(true)
            Text(text).font(.callout).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
            Spacer(minLength: 0)
            if let action = action { Button(action.0, action: action.1) }
        }
        .padding(8)
        .background(tint.opacity(0.12), in: RoundedRectangle(cornerRadius: 6))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("connection.banner")
    }
}

// MARK: - Detail text

/// The sections of a detail as wrapped, selectable text. Complete content, never truncated.
struct SectionsView: View {
    let sections: [DetailSection]

    var body: some View {
        VStack(alignment: .leading, spacing: Layout.margin) {
            ForEach(sections) { section in
                VStack(alignment: .leading, spacing: Layout.rowSpacing) {
                    Text(section.title).font(.headline).accessibilityAddTraits(.isHeader)
                    ForEach(section.rows) { row in
                        if let label = row.label {
                            // The caption and the body are two real text elements, read one after the other,
                            // each exposing its own complete text through the standard accessibility attributes.
                            VStack(alignment: .leading, spacing: 1) {
                                Text(label).font(.caption).foregroundStyle(.secondary)
                                Prose(text: row.text)
                            }
                        } else {
                            Prose(text: row.text)
                        }
                    }
                }
            }
        }
    }
}

/// The selected subject's detail, or what to do to get one.
struct SubjectPane: View {
    @ObservedObject var model: ObserverModel
    var only: Set<String>?

    var body: some View {
        if let detail = model.snapshot.detail {
            ScrollView {
                VStack(alignment: .leading, spacing: Layout.margin) {
                    VStack(alignment: .leading, spacing: 2) {
                        Text(detail.title).font(.title2.weight(.semibold)).fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                        if !detail.subtitle.isEmpty { Text(detail.subtitle).foregroundStyle(.secondary) }
                    }
                    .spoken(detail.title, value: detail.subtitle)
                    .accessibilityAddTraits(.isHeader)
                    if detail.loading { ProgressView("Reading from the application…") }
                    if let error = detail.error {
                        Badge(text: error, symbol: "exclamationmark.triangle", tint: .orange)
                    }
                    SectionsView(sections: only.map { names in detail.sections.filter { names.contains($0.title) } } ?? detail.sections)
                }
                .padding(Layout.margin)
            }
            .accessibilityIdentifier("detail.scroll")
        } else {
            EmptyState(symbol: "hand.point.up.left", title: "Nothing selected", message: "Select a task or a run in the list. Reading never changes the project.")
        }
    }
}
