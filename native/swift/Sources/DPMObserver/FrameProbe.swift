// Where a part of the page really is in the real window, reported through the state report so the
// packaged suite can assert the layout from actual geometry. It measures only; it changes no layout.
// The window's own registry is handed down the view tree; with none (no state file) a probe does nothing.

import AppKit
import DPMObserverCore
import SwiftUI

private struct LayoutRegistryKey: EnvironmentKey {
    static let defaultValue: LayoutRegistry? = nil
}

extension EnvironmentValues {
    /// The registry of the window this view is in, if its measurements are being reported.
    var layoutRegistry: LayoutRegistry? {
        get { self[LayoutRegistryKey.self] }
        set { self[LayoutRegistryKey.self] = newValue }
    }
}

struct FrameProbe: NSViewRepresentable {
    let name: String
    @Environment(\.layoutRegistry) private var registry

    func makeNSView(context: Context) -> NamedProbeView {
        let view = NamedProbeView()
        view.name = name
        view.registry = registry
        return view
    }

    func updateNSView(_ view: NamedProbeView, context: Context) {
        view.name = name
        view.registry = registry
    }
}

extension View {
    /// Report this view's frame, from the top left of the window's content view, under `name`.
    func reportFrame(_ name: String) -> some View { background(FrameProbe(name: name)) }
}

final class NamedProbeView: NSView {
    var name = ""
    weak var registry: LayoutRegistry?

    override func layout() {
        super.layout()
        publish()
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        publish()
    }

    private func publish() {
        guard let registry else { return }
        let name = self.name
        MainActor.assumeIsolated { registry.register(name) { [weak self] in self?.measure() } }
    }

    /// A rect of the content view's coordinate system as [x, y, width, height] from its top left.
    private static func fromTopLeft(_ rect: NSRect, in content: NSView) -> [Double] {
        let top = content.isFlipped ? rect.minY : content.bounds.height - rect.maxY
        return [rect.minX, top, rect.width, rect.height]
    }

    private func measure() -> LayoutRegistry.Measure? {
        guard let window, let content = window.contentView else { return nil }
        let mine = Self.fromTopLeft(convert(bounds, to: content), in: content)
        // The layout rect is in window coordinates; it is converted into the content view's own before
        // it is compared with anything measured in it.
        let usable = Self.fromTopLeft(content.convert(window.contentLayoutRect, from: nil), in: content)
        let inner = content.bounds
        let outer = window.frame
        return (mine, usable, [inner.minX, inner.minY, inner.width, inner.height], [outer.minX, outer.minY, outer.width, outer.height])
    }
}
