// Where the window's key events meet the model. Every key press and scroll gesture the Gantt (or the
// Detail it opened) should see arrives here as the real `NSEvent`, is reduced by `KeyInput(event:)` and
// applied by `ObserverModel.handleKey`: the one handler, which the qualification suite drives with
// events it builds. This file decides only whether the Gantt owns the event; it holds no key logic.
// Ownership is read from the event's own window: the Gantt owns a key there only while its region in that
// window is the window's actual first responder (or the Gantt's filter field in that window is being typed in, and
// its editor holds no input method's marked text).

import AppKit
import DPMObserverCore

@MainActor
final class GanttKeyboard {
    static let shared = GanttKeyboard()

    private weak var model: ObserverModel?
    private var monitor: Any?
    /// The Gantt regions now in a window, held weakly; each is found only through the window it is in.
    private let regions = NSHashTable<GanttRegionView>.weakObjects()

    func attach(_ region: GanttRegionView) { regions.add(region) }
    func detach(_ region: GanttRegionView) { regions.remove(region) }

    /// The Gantt region in `window`, if that window shows the Gantt; never one of another window.
    func region(in window: NSWindow?) -> GanttRegionView? {
        guard let window else { return nil }
        return regions.allObjects.first { $0.window === window }
    }

    func install(_ model: ObserverModel) {
        guard monitor == nil else { return }
        self.model = model
        monitor = NSEvent.addLocalMonitorForEvents(matching: [.keyDown, .scrollWheel]) { [weak self] event in
            guard let self = self else { return event }
            // The monitor runs on the main thread; the event is only handed across the isolation boundary,
            // and what comes back is whether the Gantt took it.
            nonisolated(unsafe) let held = event
            let taken = MainActor.assumeIsolated { self.takes(held) }
            return taken ? nil : event
        }
    }

    /// Whether the Gantt (or the Detail it opened) took this event.
    private func takes(_ event: NSEvent) -> Bool {
        guard let model = model else { return false }
        if event.type == .scrollWheel { return scroll(event, model) }
        guard let input = KeyInput(event: event) else { return false }
        switch model.page {
        case .gantt:
            // The region of this event's window holds the keyboard, or the filter field beside it is being typed in,
            // which keeps its own keys: the model says so, and takes only Escape. While the filter's actual editor (that
            // window's first responder) holds an input method's marked text, every key is the text input system's: an
            // Escape then cancels the composition, and the field keeps the keyboard.
            guard let region = region(in: event.window), region.holdsKeyboard || model.gantt.editingFilter else { return false }
            if let editor = event.window?.firstResponder as? NSTextView, editor.hasMarkedText(), GanttRegionView.filterField(of: editor) != nil { return false }
        case .detail:
            // Detail's own search field keeps its keys; Back is reached by Escape or ⌘[ elsewhere.
            guard model.canReturn, !(event.window?.firstResponder is NSText) else { return false }
        default:
            return false
        }
        return model.handleKey(input) != .ignored
    }

    /// A scroll over the rows and the timeline pans the Gantt, as the shift-arrow keys do.
    private func scroll(_ event: NSEvent, _ model: ObserverModel) -> Bool {
        guard model.page == .gantt, let region = region(in: event.window) else { return false }
        let point = region.convert(event.locationInWindow, from: nil)
        guard region.bounds.contains(point) else { return false }
        let scale = event.hasPreciseScrollingDeltas ? 1.0 : 10.0
        model.pan(dx: -event.scrollingDeltaX * scale, dy: -event.scrollingDeltaY * scale)
        return true
    }
}
