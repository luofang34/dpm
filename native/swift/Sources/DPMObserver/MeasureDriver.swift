// The scripted driver of the measurement record, active only with `--measure-log` and a
// `--measure-script`. It makes the calls the record names (a selection, a view operation, a scroll
// request on each display tick) and waits for each generation's completion, which only an app-owned
// draw pass can give; it never reads a completion from the model or from a callback.
//
// It bypasses the pointer and the keyboard on purpose: the keyboard path is judged by the suite and by
// a person, not timed. A display-link callback (`tick`) is a clock and a pacing source here, never a
// frame: a frame is a `frame` stamp from the scrolled surface.
//
// Waiting is by notification: a wait ends when the model changes or the log completes a generation and the
// condition then holds, or when its bound runs out. The bound, the length of a scroll window and the pause
// between operations are timing by design; none of them is a poll for completion.

import AppKit
import Combine
import DPMObserverCore
import QuartzCore

final class DisplayTicker: NSObject {
    var onTick: ((CADisplayLink) -> Void)?

    @objc func tick(_ link: CADisplayLink) { onTick?(link) }
}

/// One wait for a condition: it ends once, when the condition holds after a notification or when the bound runs out.
@MainActor
private final class Waiter {
    private var continuation: CheckedContinuation<Bool, Never>?
    private var subscription: AnyCancellable?
    private var token: Int?
    private var bound: Task<Void, Never>?
    private let condition: @MainActor () -> Bool

    init(_ condition: @escaping @MainActor () -> Bool) { self.condition = condition }

    func start(model: ObserverModel, log: MeasureLog?, seconds: Double, _ continuation: CheckedContinuation<Bool, Never>) {
        self.continuation = continuation
        // The model announces a change before it is made, so the condition is read on the next turn.
        // The waiter is held by what it subscribed to until it finishes, which lets all of them go.
        subscription = model.objectWillChange.sink { _ in Task { @MainActor in self.check() } }
        token = log?.notifyOnComplete { _ in Task { @MainActor in self.check() } }
        bound = Task { @MainActor in
            try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
            self.finish(self.condition())
        }
        check()
    }

    func check() {
        if continuation != nil, condition() { finish(true) }
    }

    private func finish(_ held: Bool) {
        guard let waiting = continuation else { return }
        continuation = nil
        subscription?.cancel()
        subscription = nil
        bound?.cancel()
        bound = nil
        if let token = token { MeasureLog.shared?.stopNotifying(token) }
        token = nil
        waiting.resume(returning: held)
    }
}

@MainActor
final class MeasureDriver {
    static let shared = MeasureDriver()

    private weak var model: ObserverModel?
    private var ticker: DisplayTicker?
    private var link: CADisplayLink?
    private var activity: NSObjectProtocol?
    private var axis: GanttDrive.Axis?
    /// The scroll window in force, numbered across the process: a window's number is never reused, so a
    /// request of one window cannot be taken for a request of another.
    private var window = 0
    /// The request's number within the window, from 1.
    private var sequence = 0
    private var unavailable = false
    /// The display link's own record: when it was made, how many ticks arrived and when the first did.
    private var started: UInt64?
    private var ticksSeen = 0
    private var firstTickMs: Double?

    func start(model: ObserverModel, options: LaunchOptions) {
        guard let log = MeasureLog.shared else { return }
        self.model = model
        if let screen = NSScreen.main {
            let ticker = DisplayTicker()
            ticker.onTick = { [weak self] link in
                MainActor.assumeIsolated { self?.tick(link) }
            }
            let link = screen.displayLink(target: ticker, selector: #selector(DisplayTicker.tick(_:)))
            link.add(to: .main, forMode: .common)
            self.ticker = ticker
            self.link = link
            // A measurement runs for a long time and its clock is the display's: the display is not to idle to sleep,
            // nor the process to be napped, while it runs. Said in the log, with what the link was made against.
            activity = ProcessInfo.processInfo.beginActivity(options: [.idleDisplaySleepDisabled, .userInitiated, .latencyCritical], reason: "DPM Observer measurement")
            log.emit("display_link", detail: ["screens": NSScreen.screens.count, "maximum_fps": screen.maximumFramesPerSecond, "paused": link.isPaused, "activity_held": activity != nil])
            started = DispatchTime.now().uptimeNanoseconds
            reportDisplayState(reason: "start")
            watchForTicks()
        } else {
            log.emit("no_display", detail: ["reason": "NSScreen.main is nil, so no display link exists"])
        }
        guard let script = options.measureScript else { return }
        Task { @MainActor in await self.run(script, options) }
    }

    /// A display-link callback: logged as a clock sample and used to pace a scroll request. It completes nothing.
    /// Real state of the display link and of the window, as observed now and never inferred: said once at start and
    /// again while no tick has arrived. The link is created from `NSScreen.main`, a screen-level link, not from a
    /// view or window; that is what this source does, and it is stated here as a fact of the wiring.
    private func reportDisplayState(reason: String) {
        guard let log = MeasureLog.shared else { return }
        func screenId(_ screen: NSScreen?) -> Any {
            guard let screen = screen else { return NSNull() }
            let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber
            return ["display_id": number?.intValue ?? -1, "name": screen.localizedName, "maximum_fps": screen.maximumFramesPerSecond]
        }
        let windows: [[String: Any]] = NSApp.windows.map { window in
            ["visible": window.isVisible, "key": window.isKeyWindow, "main": window.isMainWindow, "miniaturized": window.isMiniaturized,
             "occlusion_visible": window.occlusionState.contains(.visible), "occlusion_state": window.occlusionState.rawValue, "screen": screenId(window.screen)]
        }
        let elapsed = started.map { Double(DispatchTime.now().uptimeNanoseconds &- $0) / 1e6 } ?? -1
        log.emit("display_state", detail: [
            "reason": reason, "elapsed_ms": elapsed, "ticks_seen": ticksSeen, "first_tick_ms": firstTickMs.map { $0 as Any } ?? NSNull(),
            "link_bound_to": "NSScreen.main (screen-level link added to the main run loop in common modes; no view or window)",
            "link_paused": link?.isPaused ?? NSNull(), "main_screen": screenId(NSScreen.main), "screens": NSScreen.screens.count,
            "app_active": NSApp.isActive, "windows": windows,
        ])
    }

    /// While no tick has arrived the state is recorded again, a few times and then stopped; a tick ends it. It makes no tick.
    private func watchForTicks() {
        Task { @MainActor in
            for seconds in [2.0, 3.0, 5.0, 10.0] {
                try? await Task.sleep(nanoseconds: UInt64(seconds * 1_000_000_000))
                if ticksSeen > 0 { return }
                reportDisplayState(reason: "no_tick_yet")
            }
        }
    }

    private func tick(_ link: CADisplayLink) {
        ticksSeen += 1
        if ticksSeen == 1 {
            firstTickMs = started.map { Double(DispatchTime.now().uptimeNanoseconds &- $0) / 1e6 }
            reportDisplayState(reason: "first_tick")
        }
        Measure.event("tick", detail: ["link_timestamp": link.timestamp, "target_timestamp": link.targetTimestamp])
        guard let axis = axis, let model = model, let log = MeasureLog.shared else { return }
        let metrics = model.scrollMetrics(axis)
        let range = metrics.extent - metrics.viewport
        let drive = GanttDrive(axis: axis, window: window, sequence: sequence &+ 1)
        // The content is finite: the trajectory wraps over its scroll range, worked out here, apart from the
        // draw. With no range there is nothing to scroll, and that is said, never counted as frames.
        guard let expected = ScrollPlan.expectedOffset(distance: drive.logicalDistance, range: range) else {
            if !unavailable {
                unavailable = true
                log.emit("scroll_unavailable", gen: drive.generation, detail: ["axis": axis.rawValue, "window_id": window, "extent": metrics.extent, "viewport": metrics.viewport])
            }
            return
        }
        sequence = drive.sequence
        let facts = [
            "axis": axis.rawValue, "window_id": String(window), "seq": String(drive.sequence),
            "logical_distance": Measure.format(drive.logicalDistance), "extent": Measure.format(metrics.extent), "viewport": Measure.format(metrics.viewport),
            "offset": Measure.format(expected),
        ]
        let record: [String: Any] = [
            "axis": axis.rawValue, "window_id": window, "seq": drive.sequence, "logical_distance": drive.logicalDistance,
            "expected_effective_offset": expected, "extent": metrics.extent, "viewport": metrics.viewport,
        ]
        // The request is made, and what a draw must show for it is recorded, before the model is told.
        log.expect(drive.key, facts: facts, record: record)
        log.emit("scroll_request", gen: drive.generation, detail: record)
        model.driveScroll(drive)
    }

    // MARK: Scripts

    private func run(_ script: String, _ options: LaunchOptions) async {
        guard let model = model else { return }
        // The memory budget samples its baseline once the plan is loaded and before the session starts.
        if options.measureDelay > 0 { try? await Task.sleep(nanoseconds: UInt64(options.measureDelay * 1_000_000_000)) }
        switch script {
        case "first-draw":
            break
        case "selections":
            await ready(.detail)
            await selections(options.measureKeys)
        case "viewops":
            await ready(.gantt)
            await viewOperations(rounds: 5)
        case "scroll":
            await ready(.gantt)
            await scroll(seconds: options.measureSeconds)
        case "scroll-repeat":
            // Two successive windows on each axis in one process, to show identities are not reused.
            await ready(.gantt)
            await scroll(seconds: options.measureSeconds)
            await scroll(seconds: options.measureSeconds)
        case "session":
            await session(options)
        default:
            Measure.event("driver_error", detail: ["script": script, "reason": "unknown script"])
        }
        model.driveScroll(nil)
        Measure.event("driver_done", detail: ["script": script])
    }

    /// Wait for a connected, current workspace and, on the Gantt, for its first completed draw.
    private func ready(_ page: ObserverModel.Page) async {
        guard let model = model, let log = MeasureLog.shared else { return }
        _ = await until(60) { model.snapshot.connection == .connected && model.snapshot.freshness.current && !model.snapshot.inventory.items.isEmpty }
        if model.page != page { model.show(page) }
        if page == .gantt {
            _ = await until(30) { model.snapshot.gantt != nil && (log.isComplete("first") || !log.gateOpen("gantt")) }
        }
        // A pause by design: the plan settles before the first generation is timed.
        try? await Task.sleep(nanoseconds: 1_000_000_000)
        Measure.event("driver_ready", detail: ["page": page.rawValue])
    }

    /// Serial selections, each waiting for its own Detail draw.
    private func selections(_ keys: [String]) async {
        guard let model = model, let log = MeasureLog.shared else { return }
        let timeout = log.gateOpen("detail") ? 5.0 : 0.5
        for key in keys {
            guard let identity = model.snapshot.inventory.items.first(where: { $0.key == key })?.identity else {
                Measure.event("driver_error", detail: ["reason": "no task \(key)"])
                continue
            }
            model.select(.work(identity))
            let generation = model.selectionCount
            if await until(timeout, { log.isComplete("select:\(generation)") }) == false {
                Measure.event("generation_timeout", gen: generation, detail: ["kind": "selection", "subject": key])
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
    }

    /// Serial view operations of every kind, each waiting for its own draw.
    private func viewOperations(rounds: Int) async {
        guard let model = model, let log = MeasureLog.shared else { return }
        let timeout = log.gateOpen("gantt") ? 5.0 : 0.5
        // A block holds two calls of each of the five kinds, so five blocks are ten calls per kind: the first of a
        // kind is discarded and nine are kept. Each block goes both ways (zoom in and out, pan right and left, the
        // critical filter on and cleared), and the filter is on only while zoom and pan run, never across a
        // collapse or an expand, whose row counts are the plan's.
        let operations: [GanttIntent] = [.collapseAll, .expandAll, .toggleCritical, .zoomIn, .panRight, .clearFilter, .collapseAll, .expandAll, .zoomOut, .panLeft]
        for _ in 0..<rounds {
            for intent in operations {
                model.perform(intent)
                let sequence = model.gantt.sequence
                if await until(timeout, { log.isComplete("view_op:\(sequence)") }) == false {
                    Measure.event("generation_timeout", gen: sequence, detail: ["kind": "view_op", "op": intent.rawValue])
                }
                try? await Task.sleep(nanoseconds: 50_000_000)
            }
        }
    }

    /// Scroll vertically, then horizontally. Each window runs the measured seconds after a half second
    /// that the record discards; the harness cuts the window on the `frame` events' own clock. Every window
    /// has its own number and its requests are numbered from 1 within it.
    private func scroll(seconds: Double) async {
        guard let model = model else { return }
        for next in [GanttDrive.Axis.vertical, .horizontal] {
            window &+= 1
            sequence = 0
            unavailable = false
            Measure.event("scroll_window_start", detail: ["axis": next.rawValue, "window_id": window, "seconds": seconds])
            axis = next
            // The measured window's length is the measurement, not a wait for completion.
            try? await Task.sleep(nanoseconds: UInt64((seconds + 0.5) * 1_000_000_000))
            axis = nil
            Measure.event("scroll_window_end", detail: ["axis": next.rawValue, "window_id": window, "requests": sequence])
            model.driveScroll(nil)
            try? await Task.sleep(nanoseconds: 500_000_000)
        }
    }

    /// The five-minute session of the memory budget: the view operations, a scroll and the selections, in turn.
    private func session(_ options: LaunchOptions) async {
        let end = Date().addingTimeInterval(options.measureSeconds)
        while Date() < end {
            await ready(.gantt)
            await viewOperations(rounds: 1)
            await scroll(seconds: 2)
            await ready(.detail)
            await selections(options.measureKeys)
        }
    }

    /// Wait until a condition holds, or the bound is out. Returns whether it held.
    private func until(_ seconds: Double, _ condition: @escaping @MainActor () -> Bool) async -> Bool {
        if condition() { return true }
        guard let model = model else { return false }
        let waiter = Waiter(condition)
        return await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            waiter.start(model: model, log: MeasureLog.shared, seconds: seconds, continuation)
        }
    }
}
