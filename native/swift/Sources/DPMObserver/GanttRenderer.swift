// The app renders its own Gantt to PNG files, only when started with `--render-to DIR`. Each file is
// app-rendered, scripted and synthetic: SwiftUI's `ImageRenderer` draws the same `GanttFrame` the
// window draws, from the model as it stands, at a fixed size. It writes only PNGs, only under DIR, and
// never touches the store, the project or the selection of a person: the scenarios use the same view
// operations and selection a key press would, which change no stored data.
//
// `--render-when` says when: `loaded` (the schedule is read and current: the hierarchy expanded and
// collapsed, a nested task selected with its Detail), `disconnected` (the helper was lost and the last
// reading is marked), or `stale` (the source changed or a change is not yet read).

import AppKit
import DPMObserverCore
import SwiftUI

@MainActor
final class GanttRenderer {
    static let shared = GanttRenderer()

    private weak var model: ObserverModel?
    private var directory: URL?
    private var prefix = "gantt"
    private var finished = false

    static let size = CGSize(width: 1360, height: 820)

    func start(model: ObserverModel, options: LaunchOptions) {
        guard let directory = options.renderTo else { return }
        self.model = model
        self.directory = directory
        prefix = options.renderPrefix
        let when = options.renderWhen
        Task { @MainActor in
            let deadline = Date().addingTimeInterval(120)
            while !self.finished, Date() < deadline {
                if self.ready(when) {
                    self.finished = true
                    await self.render(when)
                }
                try? await Task.sleep(nanoseconds: 200_000_000)
            }
        }
    }

    private func ready(_ when: String) -> Bool {
        guard let model = model, model.page == .gantt, model.snapshot.gantt != nil else { return false }
        let snapshot = model.snapshot
        switch when {
        case "disconnected":
            if case .reconnecting = snapshot.connection { return true }
            return false
        case "stale":
            if case .sourceChanged = snapshot.connection { return true }
            return !snapshot.freshness.current && snapshot.connection == .connected
        default:
            return snapshot.connection == .connected && snapshot.freshness.current
        }
    }

    private func render(_ when: String) async {
        guard let model = model else { return }
        if when != "loaded" {
            write(name: "\(prefix)-\(when).png", detail: false)
            return
        }
        model.perform(.expandAll)
        write(name: "\(prefix)-expanded.png", detail: false)
        model.perform(.collapseAll)
        write(name: "\(prefix)-collapsed.png", detail: false)
        model.perform(.expandAll)
        // A nested task, selected as a key press would, with its Detail read from the application.
        guard let nested = model.outline.first(where: { $0.row.parent != nil && !$0.row.isPackage }) else { return }
        model.focusRow(nested.id)
        let selected = Subject.work(nested.id)
        let end = Date().addingTimeInterval(20)
        while Date() < end {
            if let detail = model.snapshot.detail, detail.subject == selected, !detail.loading { break }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        write(name: "\(prefix)-selected-nested-detail.png", detail: true)
    }

    /// Draw the Gantt, with the shared Detail beside it when asked, and write it as a PNG.
    private func write(name: String, detail: Bool) {
        guard let model = model, let directory = directory else { return }
        var frame = GanttFrame(model: model, keyboard: true, stamps: false)
        frame.state.viewportRows = Int((Self.size.height - 260) / GanttLayout.rowHeight)
        let content = RenderedPage(frame: frame, detail: detail ? model.snapshot.detail : nil)
            .frame(width: Self.size.width, height: Self.size.height)
            .background(Color(nsColor: .windowBackgroundColor))
        let renderer = ImageRenderer(content: content)
        renderer.scale = 2
        guard let image = renderer.nsImage, let tiff = image.tiffRepresentation, let bitmap = NSBitmapImageRep(data: tiff),
              let png = bitmap.representation(using: .png, properties: [:]) else {
            Measure.event("render_failed", detail: ["name": name])
            return
        }
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
            try png.write(to: directory.appendingPathComponent(name), options: .atomic)
            Measure.event("rendered", detail: ["name": name, "bytes": png.count])
        } catch {
            Measure.event("render_failed", detail: ["name": name, "error": "\(error)"])
        }
    }
}

/// What the window shows on the Gantt page, as one still picture: the marks, the strip, the rows and the
/// timeline, and Detail beside them when a selection's Detail is part of the scenario.
struct RenderedPage: View {
    let frame: GanttFrame
    let detail: SubjectDetail?

    var body: some View {
        HStack(spacing: 0) {
            VStack(spacing: 0) {
                Text("app-rendered, scripted, synthetic · not an operator observation · \(frame.outline.count) of \(frame.schedule?.rows.count ?? 0) rows · \(frame.state.filter.words)")
                    .font(.caption)
                    .padding(6)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if let mark = frame.marked {
                    Badge(text: mark, symbol: "exclamationmark.triangle", tint: .orange).padding(.horizontal, Layout.margin).frame(maxWidth: .infinity, alignment: .leading)
                }
                GanttStrip(frame: frame, scrolling: false)
                Divider()
                let height = Double(frame.state.viewportRows) * GanttLayout.rowHeight + GanttGeometry.axis
                let geometry = GanttGeometry(frame, height: height)
                HStack(spacing: 0) {
                    GanttLabels(frame: frame, geometry: geometry, model: nil).frame(width: GanttGeometry.labelWidth)
                    Divider()
                    Canvas { context, size in GanttDrawing.draw(&context, size: size, frame: frame) }
                }
                .frame(height: height)
                Spacer(minLength: 0)
            }
            if let detail = detail {
                Divider()
                VStack(alignment: .leading, spacing: Layout.margin) {
                    Text(detail.title).font(.title2.weight(.semibold)).fixedSize(horizontal: false, vertical: true)
                    if !detail.subtitle.isEmpty { Text(detail.subtitle).foregroundStyle(.secondary) }
                    SectionsView(sections: Array(detail.sections.prefix(6)))
                    Spacer(minLength: 0)
                }
                .padding(Layout.margin)
                .frame(width: 440, alignment: .topLeading)
            }
        }
    }
}
