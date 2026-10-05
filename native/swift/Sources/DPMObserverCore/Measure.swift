// The measurement instrument the accepted design record defines (FEAT-05 correction 2, section c.5):
// an append-only JSON Lines log of events, each stamped in the app process with the monotonic
// `CLOCK_UPTIME_RAW`, written off the main thread through a bounded buffer. It is inert unless the
// app was started with `--measure-log`.
//
// A latency or a frame is counted only for a generation that was DRAWN: the completing events
// (`first_draw`, `detail_drawn`, `view_op_drawn`, `commit_drawn`, `frame`) come only from a stamp that
// an app-owned draw pass emits, after reading the facts it drew and finding them equal to what was
// expected for that generation. Assigning a model value (`state_set`), evaluating a view body or a
// display-link callback (`tick`) completes nothing. `--measure-freeze` closes the gate of a named
// surface, so its draws paint nothing new and stamp nothing while everything else carries on: the
// negative control of the record.
//
// What a stamp proves, and does not: that the app's own drawing code ran for that generation with the
// logged content. It does not prove composition or presentation to the display, so every latency is a
// lower bound on the time until the change is visible.

import Foundation

public final class MeasureLog: @unchecked Sendable {
    /// The one log of this process, or nil when the app was not asked to measure.
    public nonisolated(unsafe) static var shared: MeasureLog?

    public let id: String
    private let lock = NSLock()
    private let queue = DispatchQueue(label: "dpm.observer.measure", qos: .utility)
    private let file: FileHandle
    private let frozen: Set<String>
    private let capacity: Int
    private var outstanding = 0
    private var dropped = 0
    private var ignored = 0
    private var closed = false
    private var expected: [String: [String: String]] = [:]
    /// What the generation's creator also recorded when it was created (for a frame: the requested distance and
    /// the independently expected offset), written into the event that completes it.
    private var expectedDetail: [String: [String: Any]] = [:]
    private var completed: Set<String> = []
    private var observers: [Int: @Sendable (String) -> Void] = [:]
    private var nextObserver = 0
    /// Test seam, nil in the app: called on the emitting thread with each event as it is emitted, before
    /// the event is queued for writing, so a test sees the order in which things happened.
    public var onEmit: (@Sendable (String, Any?, UInt64) -> Void)?

    private struct Line: @unchecked Sendable { let object: [String: Any] }

    /// Opens (creating, never appending to another run's) the log at `path` and writes its header.
    public init?(path: String, id: String, freeze: Set<String> = [], header: [String: Any], capacity: Int = 200_000) {
        guard FileManager.default.createFile(atPath: path, contents: nil), let handle = FileHandle(forWritingAtPath: path) else { return nil }
        file = handle
        self.id = id
        frozen = freeze
        self.capacity = capacity
        var first = header
        first["event"] = "log_opened"
        first["measure_id"] = id
        first["pid"] = Int(ProcessInfo.processInfo.processIdentifier)
        first["frozen"] = freeze.sorted()
        first["clock"] = "CLOCK_UPTIME_RAW"
        first["t_ns"] = Int(Self.now())
        write(Line(object: first))
    }

    /// The monotonic clock every event is stamped with, in nanoseconds.
    public static func now() -> UInt64 { clock_gettime_nsec_np(CLOCK_UPTIME_RAW) }

    private func write(_ line: Line) {
        guard JSONSerialization.isValidJSONObject(line.object), var data = try? JSONSerialization.data(withJSONObject: line.object, options: [.sortedKeys]) else { return }
        data.append(0x0A)
        file.write(data)
    }

    /// Record an event now. The timestamp is read here, on the caller's thread; encoding and writing
    /// happen on the log's own queue. A full buffer drops the event and counts it: a log with any
    /// dropped event is invalid, never silently short.
    public func emit(_ event: String, gen: Any? = nil, detail: [String: Any] = [:]) {
        let stamp = Self.now()
        onEmit?(event, gen, stamp)
        lock.lock()
        if closed || outstanding >= capacity {
            if !closed { dropped += 1 }
            lock.unlock()
            return
        }
        outstanding += 1
        lock.unlock()
        var object: [String: Any] = ["measure_id": id, "pid": Int(ProcessInfo.processInfo.processIdentifier), "event": event, "t_ns": Int(stamp), "detail": detail]
        if let gen = gen { object["gen"] = gen }
        let line = Line(object: object)
        queue.async { [self] in
            write(line)
            lock.lock()
            outstanding -= 1
            lock.unlock()
        }
    }

    /// Whether the named surface may draw: false while `--measure-freeze` closes its gate.
    public func gateOpen(_ surface: String) -> Bool { !frozen.contains(surface) }

    /// Whether a generation was completed by a draw pass.
    public func isComplete(_ key: String) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return completed.contains(key)
    }

    /// What a generation must show when it is drawn, set where the generation is created. `record` is
    /// carried into the event that completes it (or refuses it), so the log holds the expected values
    /// beside the drawn ones.
    public func expect(_ key: String, facts: [String: String], record: [String: Any] = [:]) {
        lock.lock()
        expected[key] = facts
        expectedDetail[key] = record
        lock.unlock()
    }

    /// Be told, on the completing thread, each time a generation is completed by a draw pass: the way a
    /// waiter learns of a completion without polling for it. Returns the token to stop with.
    public func notifyOnComplete(_ handler: @escaping @Sendable (String) -> Void) -> Int {
        lock.lock()
        defer { lock.unlock() }
        nextObserver &+= 1
        observers[nextObserver] = handler
        return nextObserver
    }

    public func stopNotifying(_ token: Int) {
        lock.lock()
        observers[token] = nil
        lock.unlock()
    }

    /// A draw pass reports what it drew. The completing event is emitted only when the gate is open
    /// and every expected fact equals the one read at draw time, and only for the first such pass of the
    /// generation. A pass that disagrees completes nothing and is counted in `ignored_stamps`.
    /// Returns whether this pass completed the generation.
    @discardableResult
    public func stamp(_ event: String, key: String, gen: Any, facts: [String: String], surface: String, detail: [String: Any] = [:], mismatch: String? = nil) -> Bool {
        guard gateOpen(surface) else { return false }
        lock.lock()
        let wanted = expected[key]
        let recorded = expectedDetail[key] ?? [:]
        let done = completed.contains(key)
        lock.unlock()
        guard let wanted = wanted, !done else { return false }
        // A pass that drew other content than the generation expects (the placeholder of a detail still
        // loading, an older state) completes nothing; it is counted, and the log says how many. A
        // surface may name an event for it, where a disagreement makes the run invalid.
        for name in wanted.keys.sorted() where facts[name] != wanted[name] {
            lock.lock()
            ignored += 1
            lock.unlock()
            if let mismatch = mismatch {
                var seen = recorded
                seen["fact"] = name
                seen["expected"] = wanted[name]
                seen["drawn"] = facts[name] ?? "absent"
                seen["expected_facts"] = wanted
                seen["drawn_facts"] = facts
                for (name, value) in detail { seen[name] = value }
                emit(mismatch, gen: gen, detail: seen)
            }
            return false
        }
        lock.lock()
        completed.insert(key)
        let waiting = Array(observers.values)
        lock.unlock()
        // The facts as text first, then what the generation recorded, then the numbers the draw measured:
        // a name that appears in more than one keeps its number.
        var drawn: [String: Any] = [:]
        for (name, value) in facts { drawn[name] = value }
        for (name, value) in recorded { drawn[name] = value }
        for (name, value) in detail { drawn[name] = value }
        emit(event, gen: gen, detail: drawn)
        for handler in waiting { handler(key) }
        return true
    }

    /// End the log: what was handed over is written first, then how many events were dropped.
    public func close() {
        lock.lock()
        let already = closed
        closed = true
        let lost = dropped
        let unmatched = ignored
        lock.unlock()
        guard !already else { return }
        queue.sync { [self] in
            write(Line(object: ["event": "log_closed", "dropped": lost, "ignored_stamps": unmatched, "measure_id": id, "t_ns": Int(Self.now())]))
            try? file.close()
        }
    }
}

/// Events of the shared model and its views, each a no-op unless a log is open.
public enum Measure {
    public static func event(_ name: String, gen: Any? = nil, detail: [String: Any] = [:]) {
        MeasureLog.shared?.emit(name, gen: gen, detail: detail)
    }

    public static func gateOpen(_ surface: String) -> Bool { MeasureLog.shared?.gateOpen(surface) ?? true }

    /// A coordinate as the log writes it, the same text on the expecting side and the drawing side.
    public static func format(_ value: Double) -> String { String(format: "%.3f", value) }

    /// A deliberate fault of the measurement, nil in the app unless `--measure-displace` asks for it: points
    /// added to the offset the Gantt draws a scripted scroll at. The draw stamp reads the offset it actually
    /// used, so the fault must show as a coordinate mismatch: the negative control of BF5.
    public nonisolated(unsafe) static var coordinateFault = 0.0
}

/// What a draw of the Gantt's view state shows, as text. The expecting side (the model, after the
/// operation) and the drawing side (the geometry the draw used) build it with this one function from what
/// each of them holds, so the two are comparable fact by fact.
public enum GanttFingerprint {
    public static func facts(zoom: Int, offsetX: Double, offsetY: Double, filter: GanttFilter, collapsed: Int) -> [String: String] {
        ["zoom": String(zoom), "pan_x": Measure.format(offsetX), "pan_y": Measure.format(offsetY), "filter": filter.words, "collapsed": String(collapsed)]
    }
}
