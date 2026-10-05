// The one keyboard route into a long Detail: a single focusable view behind the Detail content, at a
// defined place in the window's Tab order, that scrolls the Detail's own scroll view with the arrow keys,
// Page Up, Page Down, Home, End and Space. Text keeps its own selection; Tab, Escape and ⌘[ are not
// taken here. When the window's state is reported, the view also samples itself (first responder, Tab
// position, scroll offset) and the accessibility elements below the Detail's scroll view; with no
// reporter it samples nothing and registers nothing.

import AppKit
import DPMObserverCore
import SwiftUI

private struct HostObservationKey: EnvironmentKey {
    static let defaultValue: HostObservation? = nil
}

extension EnvironmentValues {
    /// The host observation of the window this view is in, if its views are being reported.
    var hostObservation: HostObservation? {
        get { self[HostObservationKey.self] }
        set { self[HostObservationKey.self] = newValue }
    }
}

struct DetailScrollHost: NSViewRepresentable {
    @Environment(\.hostObservation) private var host

    func makeNSView(context: Context) -> DetailFocusView {
        let view = DetailFocusView()
        view.host = host
        return view
    }

    func updateNSView(_ view: DetailFocusView, context: Context) {
        view.host = host
        view.noteUpdate()
    }
}

final class DetailFocusView: NSView {
    weak var host: HostObservation?
    /// The host events this view reports on (scrolling, the content's size, the window becoming or ceasing to
    /// be key, the app becoming or ceasing to be active), for the window it is in now; none with no reporter.
    private var observers: [NSObjectProtocol] = []

    override var acceptsFirstResponder: Bool { true }
    override var isFlipped: Bool { true }
    override var focusRingMaskBounds: NSRect { visibleRect }
    override func drawFocusRingMask() { NSBezierPath(rect: visibleRect).fill() }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Detail content" }
    override func accessibilityHelp() -> String? { "Scrolls the Detail. Arrow keys, Page Up, Page Down, Home and End move through it." }
    override func accessibilityIdentifier() -> String { "detail.content" }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        if accepted { noteFocusRingMaskChanged(); noteChange() }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        if accepted { noteFocusRingMaskChanged(); noteChange() }
        return accepted
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        super.mouseDown(with: event)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        // Leaving with the keyboard (Detail closed): back on the Gantt, whose region enters this window before this view
        // leaves it, the keyboard goes straight to that region; otherwise the window takes it back, as it held it before
        // this view existed, and a Gantt region that enters later takes it from the window.
        if newWindow == nil, let window, window.firstResponder === self {
            if let region = MainActor.assumeIsolated({ GanttKeyboard.shared.region(in: window) }) {
                region.take("detail_handoff")
            } else if let region = MainActor.assumeIsolated({ NetworkRegionView.region(in: window) }) {
                // Back on the network the same way: its region, already in the window, takes the keyboard.
                region.take("detail_handoff")
            } else {
                window.makeFirstResponder(nil)
            }
        }
        super.viewWillMove(toWindow: newWindow)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stopObserving()
        // The window rebuilds its key view loop for the views it has now, so this one is in it on every visit.
        window?.recalculateKeyViewLoop()
        guard let window, let host else { return }
        MainActor.assumeIsolated { host.register("detail") { [weak self] in self?.sample() } }
        let center = NotificationCenter.default
        func observe(_ name: Notification.Name, _ object: Any?) {
            observers.append(center.addObserver(forName: name, object: object, queue: .main) { [weak self] _ in
                MainActor.assumeIsolated { self?.noteChange() }
            })
        }
        if let scroll = enclosingScrollView {
            scroll.contentView.postsBoundsChangedNotifications = true
            observe(NSView.boundsDidChangeNotification, scroll.contentView)
            if let document = scroll.documentView {
                document.postsFrameChangedNotifications = true
                observe(NSView.frameDidChangeNotification, document)
            }
        }
        observe(NSWindow.didBecomeKeyNotification, window)
        observe(NSWindow.didResignKeyNotification, window)
        observe(NSApplication.didBecomeActiveNotification, NSApp)
        observe(NSApplication.didResignActiveNotification, NSApp)
    }

    override func layout() {
        super.layout()
        noteChange()
    }

    deinit {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }

    private func stopObserving() {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers = []
    }

    func noteChange() {
        guard let host else { return }
        MainActor.assumeIsolated { host.noteChange() }
    }

    /// SwiftUI updated the Detail's content: report now, and again on the next turn of the main run loop, so a
    /// later sample can read the hosted text and its accessibility elements once they are the new ones.
    func noteUpdate() {
        guard host != nil else { return }
        noteChange()
        RunLoop.main.perform { [weak self] in MainActor.assumeIsolated { self?.noteChange() } }
    }

    // MARK: Keys

    private static let lineStep: CGFloat = 40

    override func keyDown(with event: NSEvent) {
        let held = event.modifierFlags.intersection([.command, .option, .control])
        guard held.isEmpty, let scroll = enclosingScrollView, let target = destination(for: event, in: scroll) else {
            super.keyDown(with: event)
            return
        }
        move(scroll, toOffset: target)
    }

    /// Where a key sends the scroll position, as an offset from the top of the content; nil for a key that is not ours.
    private func destination(for event: NSEvent, in scroll: NSScrollView) -> CGFloat? {
        let clip = scroll.contentView
        let current = Self.offset(of: scroll)
        let page = max(Self.lineStep, clip.bounds.height - Self.lineStep)
        switch event.keyCode {
        case 125: return current + Self.lineStep
        case 126: return current - Self.lineStep
        case 121: return current + page
        case 116: return current - page
        case 115: return 0
        case 119: return .greatestFiniteMagnitude
        case 49: return event.modifierFlags.contains(.shift) ? current - page : current + page
        default: return nil
        }
    }

    private static func offset(of scroll: NSScrollView) -> CGFloat {
        guard let document = scroll.documentView else { return 0 }
        let clip = scroll.contentView.bounds
        return document.isFlipped ? clip.minY : document.frame.height - clip.maxY
    }

    private func move(_ scroll: NSScrollView, toOffset wanted: CGFloat) {
        guard let document = scroll.documentView else { return }
        let clip = scroll.contentView
        let largest = max(0, document.frame.height - clip.bounds.height)
        let offset = min(max(0, wanted), largest)
        let y = document.isFlipped ? offset : document.frame.height - clip.bounds.height - offset
        clip.scroll(to: NSPoint(x: clip.bounds.minX, y: y))
        scroll.reflectScrolledClipView(clip)
    }

    // MARK: What the real host reports

    private func sample() -> [String: Any]? {
        guard let window else { return nil }
        let responder = window.firstResponder
        var focused: [String: Any] = ["class": responder.map { String(describing: type(of: $0)) } ?? "none", "is_detail_content": responder === self]
        if let view = responder as? NSView { focused["identifier"] = view.accessibilityIdentifier() }
        var scrolled: Any = ["error": "no enclosing scroll view"]
        var tree = (nodes: [[String: Any]](), truncated: false, visited: 0)
        if let scroll = enclosingScrollView, let document = scroll.documentView {
            let visible = scroll.contentView.bounds.height
            scrolled = ["offset": Double(Self.offset(of: scroll)), "content_height": Double(document.frame.height), "visible_height": Double(visible),
                        "max_offset": Double(max(0, document.frame.height - visible))]
            tree = Self.elements(below: scroll)
        }
        return [
            "key_window": window.isKeyWindow, "first_responder": focused, "scroll": scrolled, "tab_steps": tabSteps(in: window).map { $0 as Any } ?? NSNull(),
            "ax": tree.nodes, "ax_truncated": tree.truncated, "ax_visited": tree.visited,
            "key_view_loop": keyViewLoop(in: window), "can_become_key_view": canBecomeKeyView, "autorecalculates_key_view_loop": window.autorecalculatesKeyViewLoop,
        ]
    }

    /// The window's key view loop as it is walked from the first responder, for the report.
    private func keyViewLoop(in window: NSWindow) -> [String] {
        let start = (window.firstResponder as? NSView) ?? window.contentView
        var out: [String] = []
        var view = start
        while let current = view, out.count < 60 {
            out.append("\(type(of: current)) \(current.accessibilityIdentifier())")
            guard let next = current.nextValidKeyView, next !== start else { break }
            view = next
        }
        return out
    }

    /// How many Tab presses from the present first responder reach this view, following the window's own
    /// key view loop; nil if it is not in the loop.
    private func tabSteps(in window: NSWindow) -> Int? {
        let start = (window.firstResponder as? NSView) ?? window.contentView
        guard var view = start else { return nil }
        if view === self { return 0 }
        for step in 1...120 {
            guard let next = view.nextValidKeyView else { return nil }
            if next === self { return step }
            if next === start { return nil }
            view = next
        }
        return nil
    }

    /// The accessibility elements below the Detail's scroll view, as the standard NSAccessibility attributes
    /// give them (AXTitle, AXDescription, AXValue, AXRole, AXIdentifier), read from the real objects.
    private static func elements(below root: NSScrollView) -> (nodes: [[String: Any]], truncated: Bool, visited: Int) {
        var nodes: [[String: Any]] = []
        var seen = Set<ObjectIdentifier>()
        var visited = 0
        var truncated = false
        func walk(_ node: Any, depth: Int) {
            guard let object = node as? NSObject, seen.insert(ObjectIdentifier(object)).inserted else { return }
            visited += 1
            if visited > 4000 || depth > 40 { truncated = true; return }
            let element = node as? NSAccessibilityProtocol
            func attribute(_ name: NSAccessibility.Attribute) -> String? {
                guard let found = object.accessibilityAttributeValue(name) else { return nil }
                if let text = found as? String { return text }
                return found is NSNull ? nil : "\(found)"
            }
            let identifier = attribute(.identifier) ?? element?.accessibilityIdentifier() ?? ""
            let title = attribute(.title) ?? element?.accessibilityTitle() ?? ""
            let label = attribute(.description) ?? element?.accessibilityLabel() ?? ""
            let value = attribute(.value) ?? element?.accessibilityValue().map { "\($0)" } ?? ""
            let role = attribute(.role) ?? element?.accessibilityRole()?.rawValue ?? ""
            if !(title.isEmpty && label.isEmpty && value.isEmpty && identifier.isEmpty) {
                nodes.append(["depth": depth, "role": role, "title": title, "label": label, "value": value, "identifier": identifier])
            }
            var children: [Any] = element?.accessibilityChildren() ?? []
            if let listed = object.accessibilityAttributeValue(.children) as? [Any] { children += listed }
            for child in children { walk(child, depth: depth + 1) }
        }
        walk(root, depth: 0)
        return (nodes, truncated, visited)
    }
}
