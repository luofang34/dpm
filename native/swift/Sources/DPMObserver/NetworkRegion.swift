// The network's keyboard stop: the native view over the graph that is a real first responder of its
// window, so whether the network holds the keyboard is what the window says, not a flag of the model.
// It also takes the pointer over the graph and draws the link being dragged while editing.

import AppKit
import DPMObserverCore
import SwiftUI

/// The native view over the graph: the network's keyboard stop, a real first responder of its window.
/// Whether the network holds the keyboard is whether this view is its window's actual first responder.
struct NetworkRegion: NSViewRepresentable {
    @Environment(\.hostObservation) private var host
    let request: String?
    /// Changes each time the review sheet closes; the region then takes the keyboard back.
    let returns: Int
    let cursor: String?
    let model: ObserverModel
    let onKeyboard: (Bool) -> Void

    func makeNSView(context: Context) -> NetworkRegionView {
        let view = NetworkRegionView()
        view.onKeyboard = onKeyboard
        view.cursor = cursor
        view.request = request
        view.returns = returns
        view.model = model
        view.host = host
        return view
    }

    func updateNSView(_ view: NetworkRegionView, context: Context) {
        view.onKeyboard = onKeyboard
        view.cursor = cursor
        view.model = model
        view.host = host
        if view.returns != returns {
            view.returns = returns
            view.take("review_closed")
        }
        // The ONE handoff path: a new node named as the element to focus (a walk, a move, a return from Detail, an
        // ordinary Escape in the filter) makes the region take the keyboard here, and nowhere else.
        guard view.request != request else { return }
        view.request = request
        if let request, request.hasPrefix("network.node."), !RegionFocus.otherTextEdits(in: view.window) { view.take("node_requested") }
    }
}

final class NetworkRegionView: NSView {
    var returns = 0
    var onKeyboard: ((Bool) -> Void)?
    var request: String?
    var cursor: String? {
        didSet { if cursor != oldValue, holdsKeyboard { NSAccessibility.post(element: self, notification: .valueChanged) } }
    }

    @MainActor private static let regions = NSHashTable<NetworkRegionView>.weakObjects()

    /// The network region in `window`, if that window shows the network.
    @MainActor static func region(in window: NSWindow?) -> NetworkRegionView? {
        guard let window else { return nil }
        return regions.allObjects.first { $0.window === window }
    }

    /// Whether the network takes this key event: only while its region in the event's window is that window's
    /// first responder, or, for Escape alone, while its filter is being typed in. An input method's marked text
    /// keeps every key, Escape included.
    @MainActor static func takes(_ event: NSEvent, input: KeyInput, model: ObserverModel) -> Bool {
        guard let window = event.window else { return false }
        if let editor = window.firstResponder as? NSTextView {
            if editor.hasMarkedText() { return false }
            // Only an ordinary Escape, and only while the window's ACTUAL first responder is the field editor of this
            // window's network filter field: the model's flag alone is not trusted.
            guard input.characters == "\u{1B}", !input.command, let region = region(in: window), filterField(of: editor) != nil else { return false }
            if !model.network.editingFilter { model.networkFilterFocus(true) }
            region.note("filter.escape", ["editor_is_filter": true])
            // The model names the cursor node; the region's update takes the keyboard for that request (one path).
            return model.handleNetworkKey(input) != .ignored
        }
        guard let region = region(in: window), region.holdsKeyboard else { return false }
        return model.handleNetworkKey(input) != .ignored
    }

    /// How `editor` is known to be the field editor of the network filter in its own window; nil if it is not.
    @MainActor static func filterField(of editor: NSTextView) -> String? {
        guard editor.isFieldEditor, let field = editor.delegate as? NSTextField, field.window === editor.window else { return nil }
        if field.accessibilityIdentifier() == "network.filter" || (field.accessibilityAttributeValue(.identifier) as? String) == "network.filter" { return "identifier" }
        return field.placeholderString == NetworkView.prompt ? "prompt" : nil
    }

    weak var model: ObserverModel?
    weak var host: HostObservation?
    private var events: [[String: Any]] = []
    private var sequence = 0
    private static let limit = 40
    private var dragOrigin: NSPoint?
    private var dragged = false
    /// The node a press landed on; it is selected on release, and only when the press did not become a drag.
    private var pressed: (identity: String, clicks: Int)?
    /// Where the press landed, in canvas coordinates: a link drawn while editing starts there.
    private var pressPoint: (x: Double, y: Double)?
    private var tracking: NSTrackingArea?

    /// One bounded receipt of what this region really received and did, with its window's actual first responder.
    func note(_ event: String, _ facts: [String: Any] = [:]) {
        guard let host else { return }
        sequence &+= 1
        var entry = facts
        entry["seq"] = sequence
        entry["event"] = event
        entry["uptime"] = ProcessInfo.processInfo.systemUptime
        let responder = window?.firstResponder
        let editor = responder as? NSTextView
        let isFilter = editor.map { found in MainActor.assumeIsolated { Self.filterField(of: found) != nil } } ?? false
        entry["first_responder"] = ["class": responder.map { String(describing: type(of: $0)) } ?? "none", "is_region": responder === self,
                                    "is_filter_editor": isFilter] as [String: Any]
        events.append(entry)
        if events.count > Self.limit { events.removeFirst(events.count - Self.limit) }
        MainActor.assumeIsolated { host.noteChange() }
    }

    private func traced() -> [String: Any]? {
        guard let window else { return nil }
        return ["window": window.windowNumber, "holds_keyboard": holdsKeyboard, "bounds": [bounds.width, bounds.height], "events": events, "sequence": sequence, "limit": Self.limit]
    }

    /// A point of this view as canvas coordinates from the top left.
    private func canvasPoint(_ event: NSEvent) -> (x: Double, y: Double) {
        let p = convert(event.locationInWindow, from: nil)
        return (p.x, isFlipped ? p.y : bounds.height - p.y)
    }

    /// The end of `node` a point over it names: its left half is the start, its right half the finish.
    @MainActor private func end(of node: NetworkNode, at point: (x: Double, y: Double)) -> PlanEnd {
        guard let model else { return .finish }
        return NetworkLayout.end(of: node, in: model.networkGraph, at: point, pan: (model.network.panX, model.network.panY), scale: model.network.scale)
    }

    @MainActor private func node(at point: (x: Double, y: Double)) -> NetworkNode? {
        guard let model else { return nil }
        let state = model.network
        return NetworkLayout.hit(model.networkGraph, shown: model.networkNodes, at: point, pan: (state.panX, state.panY), scale: state.scale)
    }

    /// The window's actual field editor while it edits this window's network filter, read from the real text view on
    /// each sample: first responder, key window, string, marked text and the ranges. Nil when the filter is not being
    /// edited in this window; it reports what the editor holds and never a flag of its own.
    private func filterEditor() -> [String: Any]? {
        guard let window else { return nil }
        guard let editor = window.firstResponder as? NSTextView, editor.window === window,
              let by = MainActor.assumeIsolated({ Self.filterField(of: editor) }) else {
            return ["window": window.windowNumber, "current": NSNull()]
        }
        func range(_ value: NSRange) -> Any { value.location == NSNotFound ? NSNull() : [value.location, value.length] }
        let text = editor.string
        return ["window": window.windowNumber, "current": ["uptime": ProcessInfo.processInfo.systemUptime, "key_window": window.isKeyWindow, "identified_by": by,
                                                           "first_responder": true, "has_marked_text": editor.hasMarkedText(),
                                                           "marked_range": range(editor.markedRange()), "selected_range": range(editor.selectedRange()),
                                                           "string": String(text.prefix(256)), "length": (text as NSString).length] as [String: Any]]
    }

    var holdsKeyboard: Bool { window?.firstResponder === self }

    override var acceptsFirstResponder: Bool { true }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { NSBezierPath(rect: bounds).fill() }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Dependency network graph" }
    override func accessibilityValue() -> Any? { cursor ?? "no node under the cursor" }
    override func accessibilityHelp() -> String? {
        "Arrow keys move the cursor node, Space selects it, Return opens it in Detail; p, s, g and b walk to its predecessors, successors, decision gates and blockers. The text list beside the graph carries every node and edge in full."
    }
    override func accessibilityIdentifier() -> String { "network.region" }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    private func tell() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.note("settled", ["holds_keyboard": self.holdsKeyboard])
            self.onKeyboard?(self.holdsKeyboard)
        }
    }

    func take(_ reason: String) {
        guard let window, window.firstResponder !== self else { return }
        let accepted = window.makeFirstResponder(self)
        note("take." + reason, ["accepted": accepted])
    }

    // MARK: Pointer: a click selects the node under it, a double click opens Detail, a drag on the background or a
    // scroll pans, a pinch zooms. None of it moves or changes a plan item; all of it is view state and selection.

    override func acceptsFirstMouse(for event: NSEvent?) -> Bool { true }

    override func mouseDown(with event: NSEvent) {
        take("pointer")
        let point = canvasPoint(event)
        dragOrigin = event.locationInWindow
        dragged = false
        pressPoint = point
        MainActor.assumeIsolated {
            let hit = node(at: point)
            pressed = hit.map { ($0.identity, event.clickCount) }
            note("mouse_down", ["point": [point.x, point.y], "clicks": event.clickCount, "hit": hit?.key ?? NSNull()])
        }
    }

    override func mouseDragged(with event: NSEvent) {
        guard let origin = dragOrigin else { return }
        let now = event.locationInWindow
        let dx = now.x - origin.x, dy = now.y - origin.y
        guard dragged || abs(dx) + abs(dy) > 3 else { return }
        dragged = true
        dragOrigin = now
        let point = canvasPoint(event)
        MainActor.assumeIsolated {
            // While editing, a drag from a task or milestone draws a link from the half of the node pressed (its start or
            // finish, as on a Gantt bar) instead of panning; nothing moves. A work package's span comes from its tasks.
            if let model, model.editing.enabled, let press = pressed, let start = pressPoint, let node = model.networkGraph.node(press.identity), !node.isDecision {
                if node.kind == .package {
                    model.setEditNotice("A work package's span comes from its tasks: link one of its tasks instead of \(node.key).")
                    return
                }
                let current = model.network.linking.flatMap { $0.from == press.identity ? $0.fromEnd : nil }
                let fromEnd = current ?? end(of: node, at: start)
                model.setNetworkLink(NetworkLinkDrag(from: press.identity, fromEnd: fromEnd, origin: NetworkPoint(x: start.x, y: start.y), point: NetworkPoint(x: point.x, y: point.y)))
                return
            }
            // The content follows the pointer: the pan grows as the pointer moves left or up (window y grows upwards).
            model?.panNetwork(dx: -dx, dy: dy)
        }
    }

    override func mouseUp(with event: NSEvent) {
        let point = canvasPoint(event)
        MainActor.assumeIsolated {
            if let model, let link = model.network.linking {
                model.setNetworkLink(nil)
                if let target = node(at: point), target.identity != link.from, !target.isDecision {
                    if target.kind == .package {
                        model.setEditNotice("A link joins tasks and milestones; \(target.key) is a work package.")
                        note("edit_link_refused", ["from": model.editName(link.from), "to": target.key])
                    } else {
                        let toEnd = end(of: target, at: point)
                        model.edit(.link(id: UUID().uuidString.lowercased(), from: link.from, fromEnd: link.fromEnd, to: target.identity, toEnd: toEnd, lagHours: 0))
                        note("edit_link", ["from": model.editName(link.from), "to": target.key, "kind": PlanEdit.kind(from: link.fromEnd, to: toEnd)])
                    }
                }
            } else if dragged {
                note("drag_end", ["pan": model.map { [$0.network.panX, $0.network.panY] } ?? []])
            } else if let press = pressed, let model {
                // A click, not a drag: only now does it select, so panning from a node leaves the selection alone.
                model.focusNode(press.identity, selecting: true)
                if press.clicks >= 2 { model.performNetwork(.openDetail) }
                note("click", ["selected": press.identity, "opened_detail": press.clicks >= 2])
            }
        }
        pressed = nil
        pressPoint = nil
        dragOrigin = nil
        dragged = false
    }

    override func scrollWheel(with event: NSEvent) {
        let scale = event.hasPreciseScrollingDeltas ? 1.0 : 10.0
        MainActor.assumeIsolated {
            model?.panNetwork(dx: -event.scrollingDeltaX * scale, dy: -event.scrollingDeltaY * scale)
            note("scroll", ["delta": [event.scrollingDeltaX, event.scrollingDeltaY], "precise": event.hasPreciseScrollingDeltas,
                            "pan": model.map { [$0.network.panX, $0.network.panY] } ?? []])
        }
    }

    override func magnify(with event: NSEvent) {
        guard abs(event.magnification) > 0.02 else { return }
        MainActor.assumeIsolated { model?.performNetwork(event.magnification > 0 ? .zoomIn : .zoomOut) }
    }

    override func updateTrackingAreas() {
        super.updateTrackingAreas()
        if let tracking { removeTrackingArea(tracking) }
        let area = NSTrackingArea(rect: bounds, options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect], owner: self)
        addTrackingArea(area)
        tracking = area
    }

    override func mouseMoved(with event: NSEvent) {
        let point = canvasPoint(event)
        MainActor.assumeIsolated {
            let before = model?.networkHover?.key
            model?.hoverNetwork(at: NetworkPoint(x: point.x, y: point.y))
            let now = model?.networkHover?.key
            if now != before { note("hover", ["point": [point.x, point.y], "hit": now ?? NSNull()]) }
        }
    }

    override func mouseExited(with event: NSEvent) {
        MainActor.assumeIsolated { model?.hoverNetwork(at: nil) }
    }

    override func layout() {
        super.layout()
        let size = bounds.size
        MainActor.assumeIsolated {
            model?.setNetworkViewport(width: size.width, height: size.height)
            host?.noteChange()
        }
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        if newWindow == nil, let window, window.firstResponder === self { window.makeFirstResponder(nil) }
        super.viewWillMove(toWindow: newWindow)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        guard let window else {
            MainActor.assumeIsolated { Self.regions.remove(self) }
            return
        }
        MainActor.assumeIsolated {
            Self.regions.add(self)
            host?.register("network_focus") { [weak self] in self?.traced() }
            host?.register("network_filter_editor") { [weak self] in self?.filterEditor() }
        }
        note("attached")
        window.recalculateKeyViewLoop()
        let responder = window.firstResponder
        // On entry the model names the cursor node (from the shared selection): the region takes the keyboard, from the
        // window or from the previous page's region, never from a text editor being typed in.
        let named = request?.hasPrefix("network.node.") ?? false
        if responder == nil || responder === window || responder === window.contentView || (named && !(responder is NSText)) { take("arrival") }
    }
}

/// The model the pointer's scroll pans; set once at launch, as the key monitor's is.
@MainActor enum NetworkPointer {
    static weak var model: ObserverModel?
}

/// The link being drawn on the network while editing, with what a drop does.
struct NetworkLinkView: View {
    let link: NetworkLinkDrag
    @ObservedObject var model: ObserverModel

    var body: some View {
        let target = self.target
        ZStack(alignment: .topLeading) {
            Canvas { context, _ in
                guard let start = canvas(model.networkEnd(link.from, link.fromEnd)) else { return }
                let tip = target.flatMap { canvas(model.networkEnd($0.node.identity, $0.end)) } ?? NetworkPoint(x: link.point.x, y: link.point.y)
                let points = ElbowRoute.points(from: start, outward: ElbowRoute.outward(link.fromEnd), to: tip, inward: target.map { ElbowRoute.inward($0.end) } ?? 0)
                var path = Path()
                path.addLines(points.map { CGPoint(x: $0.x, y: $0.y) })
                context.stroke(path, with: .color(Palette.relationEmphasis), style: StrokeStyle(lineWidth: 2, dash: target == nil ? [5, 3] : []))
                context.fill(Path(ellipseIn: CGRect(x: start.x - 4, y: start.y - 4, width: 8, height: 8)), with: .color(Palette.relationEmphasis))
                if let end = target?.end {
                    context.fill(GanttEditDrag.arrowhead(at: CGPoint(x: tip.x, y: tip.y), pointingRight: end == .start), with: .color(Palette.relationEmphasis))
                }
            }
            Text(hint(target)).font(.caption).padding(5)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 5))
                .offset(x: link.point.x + 12, y: link.point.y + 12)
        }
        .allowsHitTesting(false)
    }

    /// The task or milestone under the pointer and the end of it a drop would join.
    private var target: (node: NetworkNode, end: PlanEnd)? {
        let state = model.network, at = (x: link.point.x, y: link.point.y)
        guard let node = NetworkLayout.hit(model.networkGraph, shown: model.networkNodes, at: at, pan: (state.panX, state.panY), scale: state.scale),
              node.identity != link.from, !node.isDecision, node.kind != .package else { return nil }
        return (node, NetworkLayout.end(of: node, in: model.networkGraph, at: at, pan: (state.panX, state.panY), scale: state.scale))
    }

    /// A graph point where the canvas draws it.
    private func canvas(_ point: NetworkPoint?) -> NetworkPoint? {
        point.map { NetworkPoint(x: $0.x * model.network.scale - model.network.panX, y: $0.y * model.network.scale - model.network.panY) }
    }

    private func hint(_ target: (node: NetworkNode, end: PlanEnd)?) -> String {
        guard let target else {
            return "Link from \(model.editName(link.from))'s \(link.fromEnd.rawValue): drop on a task or milestone, its left half for its start, its right half for its finish"
        }
        return "\(GanttRelation.abbreviation(PlanEdit.kind(from: link.fromEnd, to: target.end))) \(model.editName(link.from)) → \(target.node.key)"
    }
}
