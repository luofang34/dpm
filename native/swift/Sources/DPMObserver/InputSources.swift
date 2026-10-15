// Which processes delivered the window's presses, keys and scrolls, reported only while the window's state is. A
// scripted route posts its own events; one from any other process (another automation's clicks, a person's keyboard)
// lands on the same window, so the route's verdict must know it was disturbed rather than judge what it caused.

import AppKit
import DPMObserverCore

@MainActor final class InputSources {
    /// Counts keyed "source pid/tag/event type": the pid the window server stamps, the tag a scripted poster writes into
    /// the event's user data, and the NSEvent type.
    private var counts: [String: Int] = [:]
    private var monitor: Any?

    func attach(host: HostObservation) {
        guard monitor == nil else { return }
        let kinds: NSEvent.EventTypeMask = [.leftMouseDown, .rightMouseDown, .otherMouseDown, .keyDown, .scrollWheel]
        monitor = NSEvent.addLocalMonitorForEvents(matching: kinds) { [weak self, weak host] event in
            // The window server stamps an event with the process that posted it (0 for a device's own, and for an event
            // posted to the HID tap); a scripted poster also tags its events with its pid in the user data.
            let cg = event.cgEvent
            let source = "\(cg?.getIntegerValueField(.eventSourceUnixProcessID) ?? -1)/\(cg?.getIntegerValueField(.eventSourceUserData) ?? 0)/\(event.type.rawValue)"
            MainActor.assumeIsolated {
                self?.counts[source, default: 0] &+= 1
                host?.noteChange()
            }
            return event
        }
        host.register("input_sources") { [weak self] in
            guard let self else { return nil }
            return MainActor.assumeIsolated {
                ["by_source": self.counts]
            }
        }
    }
}
