// The Gantt's keyboard region: one AppKit first responder behind the rows and the timeline, and the measure of
// the viewport the window gives it.

import AppKit
import DPMObserverCore
import SwiftUI

// MARK: - The keyboard region, and the viewport as the window reports it

/// The native view behind the rows and the timeline: the Gantt's one keyboard stop, a real first responder of
/// its window, and the measure of the viewport. Whether the Gantt holds the keyboard is whether this view is its
/// window's actual first responder; nothing else says so.
struct GanttRegion: NSViewRepresentable {
    @Environment(\.hostObservation) private var host
    /// The model's requests for a row to have the keyboard, counted; a new one makes the region take it.
    let request: Int
    /// The cursor row in words, the region's accessibility value.
    let cursor: String?
    let pointer: GanttPointerLog
    let onKeyboard: (Bool) -> Void
    let onResize: (CGSize) -> Void

    func makeNSView(context: Context) -> GanttRegionView {
        let view = GanttRegionView()
        view.request = request
        update(view)
        return view
    }

    func updateNSView(_ view: GanttRegionView, context: Context) {
        update(view)
        guard view.request != request else { return }
        view.request = request
        view.take("row_requested")
    }

    private func update(_ view: GanttRegionView) {
        view.host = host
        if view.pointer !== pointer {
            view.pointer = pointer
            view.registerPointer()
        }
        view.onKeyboard = onKeyboard
        view.onResize = onResize
        view.cursor = cursor
    }
}

final class GanttRegionView: NSView {
    weak var host: HostObservation?
    var onKeyboard: ((Bool) -> Void)?
    var onResize: ((CGSize) -> Void)?
    var request = 0
    var cursor: String? {
        didSet { if cursor != oldValue, holdsKeyboard { NSAccessibility.post(element: self, notification: .valueChanged) } }
    }

    /// Whether this view is now the actual first responder of the window it is in.
    var holdsKeyboard: Bool { window?.firstResponder === self }

    override var acceptsFirstResponder: Bool { true }
    override var focusRingMaskBounds: NSRect { bounds }
    override func drawFocusRingMask() { NSBezierPath(rect: bounds).fill() }

    override func isAccessibilityElement() -> Bool { true }
    override func accessibilityRole() -> NSAccessibility.Role? { .group }
    override func accessibilityLabel() -> String? { "Gantt" }
    override func accessibilityValue() -> Any? { cursor ?? "" }
    override func accessibilityHelp() -> String? { "Arrow keys move the cursor row, Space selects it, Return opens it in Detail. The rows beside it carry their full text." }
    override func accessibilityIdentifier() -> String { "gantt.region" }

    override func layout() {
        super.layout()
        onResize?(bounds.size)
    }

    override func becomeFirstResponder() -> Bool {
        let accepted = super.becomeFirstResponder()
        note("become", ["accepted": accepted])
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    override func resignFirstResponder() -> Bool {
        let accepted = super.resignFirstResponder()
        note("resign", ["accepted": accepted])
        if accepted { noteFocusRingMaskChanged(); tell() }
        return accepted
    }

    /// Once this turn is done, the page is told whether the region holds the keyboard then, as its window says.
    private func tell() {
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.note("settled", ["holds_keyboard": self.holdsKeyboard])
            self.onKeyboard?(self.holdsKeyboard)
        }
    }

    /// One request that this region become its window's first responder, unless it already is. From the filter's editor
    /// this one call is the whole handoff: the window ends the editor's editing (the field releases the keyboard,
    /// `filter.end`) and then gives the keyboard to this region, and the field's focus state follows.
    func take(_ reason: String) {
        guard let window, window.firstResponder !== self else { return }
        let fromFilter = host != nil && (window.firstResponder as? NSTextView).flatMap { Self.filterField(of: $0) } != nil
        let accepted = window.makeFirstResponder(self)
        note("take." + reason, ["accepted": accepted, "from_filter_editor": fromFilter])
    }

    override func mouseDown(with event: NSEvent) {
        take("pointer")
        super.mouseDown(with: event)
    }

    override func viewWillMove(toWindow newWindow: NSWindow?) {
        // Leaving with the keyboard (the page closed): the window takes it back, as it held it before this view existed.
        if newWindow == nil, let window, window.firstResponder === self { window.makeFirstResponder(nil) }
        super.viewWillMove(toWindow: newWindow)
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        stopObserving()
        guard let window else {
            MainActor.assumeIsolated { GanttKeyboard.shared.detach(self) }
            return
        }
        MainActor.assumeIsolated { GanttKeyboard.shared.attach(self) }
        // The window rebuilds its key view loop for the views it has now, so Tab and Shift-Tab reach this one on every visit.
        window.recalculateKeyViewLoop()
        startTrace()
        note("attached")
        // Arriving on the page while no control holds the keyboard (the window or its root view does), the region takes
        // it, and the cursor row is the element in it. Back from Detail, the Detail content may still hold it here: it hands
        // it to this region as it leaves the window.
        let responder = window.firstResponder
        if responder == nil || responder === window || responder === window.contentView { take("arrival") }
    }

    // MARK: What the real host reports

    /// The region's own focus events, each with its own window's actual first responder; bounded, and kept only while the
    /// window's state is reported.
    private var events: [[String: Any]] = []
    private var sequence = 0
    private static let limit = 48
    /// The filter's editor in this window, as last seen editing the filter, and its readings: bounded, and kept only while
    /// the window's state is reported.
    private weak var editor: NSTextView?
    private var readings: [[String: Any]] = []
    private var readingSequence = 0
    private var observers: [NSObjectProtocol] = []

    /// This body's pointer log, owned by the body and reported for this region's window.
    var pointer: GanttPointerLog?

    /// The pointer log reports through this region's host for this region's window, from the moment both exist; the
    /// launch creates its one host once, so a new window (not a replaced host) is what registers again. A log whose region
    /// left its window reports nothing (its sampler is removed). Host replacement is not a supported lifecycle here.
    func registerPointer() {
        guard let pointer, let host, window != nil else { return }
        MainActor.assumeIsolated {
            pointer.attach(host: host, region: self)
            host.register("gantt_pointer") { [weak pointer] in pointer?.traced() }
        }
    }

    private func startTrace() {
        guard let host, let window else { return }
        MainActor.assumeIsolated {
            host.register("gantt_focus") { [weak self] in self?.traced() }
            host.register("gantt_filter_editor") { [weak self] in self?.editorTraced() }
        }
        registerPointer()
        // The filter's own editor in this window: when the field begins and ends editing with it (the end is the field
        // releasing the keyboard), and each change of its text or selection (typing, an input method's marked text and its
        // commit). Delivered on the posting turn, so each is in order with the region's own events.
        for name in [NSControl.textDidBeginEditingNotification, NSControl.textDidEndEditingNotification, NSText.didChangeNotification, NSTextView.didChangeSelectionNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: nil) { [weak self, weak window] notification in
                let field = notification.object as? NSTextField
                guard let editor = field == nil ? notification.object as? NSTextView : notification.userInfo?["NSFieldEditor"] as? NSTextView,
                      let window, (field?.window ?? editor.window) === window else { return }
                MainActor.assumeIsolated { self?.editorChanged(name, editor, field) }
            })
        }
    }

    private func stopObserving() {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers = []
    }

    deinit {
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
    }

    private func note(_ event: String, _ facts: [String: Any] = [:]) {
        guard let host else { return }
        sequence &+= 1
        var entry = facts
        entry["seq"] = sequence
        entry["event"] = event
        entry["time"] = Date().timeIntervalSince1970
        entry["uptime"] = ProcessInfo.processInfo.systemUptime
        entry["window"] = window.map { ["number": $0.windowNumber, "key": $0.isKeyWindow] as [String: Any] } ?? NSNull()
        entry["first_responder"] = window.map { Self.responder(in: $0, region: self) } ?? NSNull()
        events.append(entry)
        if events.count > Self.limit { events.removeFirst(events.count - Self.limit) }
        MainActor.assumeIsolated { host.noteChange() }
    }

    private static func responder(in window: NSWindow, region: GanttRegionView) -> [String: Any] {
        guard let responder = window.firstResponder else { return ["class": NSNull()] }
        let identifier = (responder as? NSView)?.accessibilityIdentifier() ?? ""
        return ["class": String(describing: type(of: responder)), "identifier": identifier.isEmpty ? NSNull() : identifier,
                "is_region": responder === region, "is_window": responder === window, "is_content_view": responder === window.contentView]
    }

    private func traced() -> [String: Any]? {
        guard let window else { return nil }
        return ["window": window.windowNumber, "holds_keyboard": holdsKeyboard, "events": events, "sequence": sequence, "limit": Self.limit]
    }

    /// How `field` is known to be this page's filter (its accessibility identifier, else its prompt); nil if it is not.
    static func filterIdentity(_ field: NSTextField) -> String? {
        if field.accessibilityIdentifier() == "gantt.filter" || (field.accessibilityAttributeValue(.identifier) as? String) == "gantt.filter" { return "identifier" }
        return field.placeholderString == GanttToolbar.prompt ? "prompt" : nil
    }

    /// How the field `editor` is editing is known to be this page's filter; nil if it is editing no field, or another.
    static func filterField(of editor: NSTextView) -> String? {
        guard editor.isFieldEditor, let field = editor.delegate as? NSTextField, field.window === editor.window else { return nil }
        return filterIdentity(field)
    }

    /// One notification of a text editor of this window (with the field, for a field's begin and end): kept only if the
    /// editor is the filter's.
    private func editorChanged(_ name: Notification.Name, _ editor: NSTextView, _ field: NSTextField?) {
        guard host != nil, let window, let by = field.map({ Self.filterIdentity($0) }) ?? Self.filterField(of: editor) else { return }
        self.editor = editor
        if name == NSControl.textDidBeginEditingNotification { note("filter.begin") }
        if name == NSControl.textDidEndEditingNotification { note("filter.end") }
        readingSequence &+= 1
        var entry = Self.reading(editor, by, in: window)
        entry["seq"] = readingSequence
        entry["cause"] = name.rawValue
        readings.append(entry)
        if readings.count > Self.limit { readings.removeFirst(readings.count - Self.limit) }
        MainActor.assumeIsolated { host?.noteChange() }
    }

    /// What the filter's editor itself holds now, read from the real text view: never from the model's filter text.
    private static func reading(_ editor: NSTextView, _ by: String, in window: NSWindow) -> [String: Any] {
        func range(_ value: NSRange) -> Any { value.location == NSNotFound ? NSNull() : [value.location, value.length] }
        let text = editor.string
        return ["time": Date().timeIntervalSince1970, "uptime": ProcessInfo.processInfo.systemUptime, "window": window.windowNumber, "key_window": window.isKeyWindow,
                "identified_by": by, "first_responder": window.firstResponder === editor, "has_marked_text": editor.hasMarkedText(),
                "marked_range": range(editor.markedRange()), "selected_range": range(editor.selectedRange()),
                "string": String(text.prefix(256)), "length": (text as NSString).length]
    }

    private func editorTraced() -> [String: Any]? {
        guard let window else { return nil }
        // The window's actual first responder if it is the filter's editor (a field given the keyboard posts nothing until it
        // is edited), else the editor last seen editing the filter; null when neither edits the filter in this window now.
        var current: Any = NSNull()
        if let editor = (window.firstResponder as? NSTextView) ?? editor, editor.window === window, let by = Self.filterField(of: editor) {
            current = Self.reading(editor, by, in: window)
        }
        return ["window": window.windowNumber, "current": current, "readings": readings, "sequence": readingSequence, "limit": Self.limit]
    }
}
