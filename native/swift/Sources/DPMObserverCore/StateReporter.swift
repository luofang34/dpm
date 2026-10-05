// A machine-readable record of what the running app is showing, written only when the app is
// started with `--state-file`. It is how the packaged scenario sees the real application: the same
// snapshot the views render, the connection and freshness it carries, and how long the main thread
// was ever kept from its run loop. It is inert otherwise, and it reads nothing from a project.
//
// Writing is bounded: at most one write is in flight and at most one newer snapshot waits behind it,
// replaced by every later one, however slow the disk or the sink. A failed write is counted and its
// reason is carried in the next state written, never dropped.

import Combine
import Foundation

/// A value, or JSON null.
func orNull<T>(_ value: T?) -> Any { value.map { $0 as Any } ?? NSNull() }

/// Where one window put its parts, as that window's own probes measured them. It belongs to the window
/// whose content holds the probes (and to the one reporter that follows it); nothing is shared between
/// windows, and a window with no registry has no probes at work. Every rect is [x, y, width, height]:
/// `frames` and `usableContent` are in points from the top left of the window's content view,
/// `contentBounds` is that view's own bounds, and `windowFrame` is the outer `NSWindow.frame` on the
/// screen (origin at the screen's bottom left). Nothing here changes what is drawn.
@MainActor
public final class LayoutRegistry {
    /// One measurement of a part, with the window facts it was taken against.
    public typealias Measure = (frame: [Double], usableContent: [Double], contentBounds: [Double], windowFrame: [Double])
    private var probes: [String: () -> Measure?] = [:]
    private var last: [String: [Double]] = [:]
    private var revision = 0
    public var onChange: (() -> Void)?

    public init() {}

    /// A part says how to measure itself now; the measurement is taken when a state is written, so it is
    /// never older than the write. A change since the last report asks for a write.
    public func register(_ name: String, measure: @escaping () -> Measure?) {
        probes[name] = measure
        guard let now = measure() else { return }
        let key = now.frame + now.usableContent + now.contentBounds + now.windowFrame
        guard last[name] != key else { return }
        last[name] = key
        revision &+= 1
        onChange?()
    }

    /// Something the report carries beside the frames changed (the viewport's row count): write again.
    public func noteChange() {
        revision &+= 1
        onChange?()
    }

    /// A part that is gone no longer reports.
    public func remove(_ name: String) {
        probes[name] = nil
        last[name] = nil
    }

    func described() -> [String: Any] {
        var frames: [String: Any] = [:]
        var sample: Measure?
        for (name, measure) in probes {
            guard let now = measure() else { remove(name); continue }
            frames[name] = now.frame
            sample = now
        }
        return [
            "window_frame": orNull(sample?.windowFrame), "content_bounds": orNull(sample?.contentBounds), "usable_content": orNull(sample?.usableContent),
            "frames": frames, "revision": revision, "measured_at_ms": Int(Date().timeIntervalSince1970 * 1000),
        ]
    }
}

/// What the real hosted views of one window report about themselves (the actual first responder, the
/// accessibility attributes of an element, a scroll offset), sampled by the view layer when a state is
/// written. A part registers a sampler holding only a weak reference to its view; with no registry there
/// is no registration and no sampling. A value that cannot be read from the host is reported as null or
/// as an explicit error by its sampler, never inferred from the model.
@MainActor
public final class HostObservation {
    public typealias Sample = () -> [String: Any]?
    private var samplers: [String: Sample] = [:]
    public var onChange: (() -> Void)?

    public init() {}

    public func register(_ name: String, sample: @escaping Sample) {
        samplers[name] = sample
        onChange?()
    }

    /// Something a sampler would report changed (focus, scroll offset, content): write again.
    public func noteChange() { onChange?() }

    public func remove(_ name: String) { samplers[name] = nil }

    func described() -> [String: Any] {
        var out: [String: Any] = [:]
        for (name, sample) in samplers {
            guard let now = sample() else { remove(name); continue }
            out[name] = now
        }
        return out
    }
}

@MainActor
public final class StateReporter {
    /// Where an encoded state goes. The default writes the file atomically; a test supplies its own.
    public typealias Sink = @Sendable (Data, URL) throws -> Void
    /// How a described state becomes bytes. It runs on the writer queue, never on the main actor.
    public typealias Encoder = @Sendable ([String: Any]) throws -> Data

    /// A state that cannot be written as JSON.
    struct Unencodable: Error, CustomStringConvertible {
        var description: String { "the state is not valid JSON" }
    }

    /// A described state travelling to the writer queue; it is not touched again after it is handed over.
    private struct Handover: @unchecked Sendable { let state: [String: Any] }

    private let file: URL
    private let sink: Sink
    private let encode: Encoder
    private weak var model: ObserverModel?
    private var layout: LayoutRegistry?
    private var host: HostObservation?
    private var lastWrite = Date.distantPast
    private var trailing = false
    private var inFlight = false
    private var waiting: [String: Any]?
    private let writer = DispatchQueue(label: "dpm.observer.state", qos: .utility)
    private let monitor = DispatchSource.makeTimerSource(queue: .main)
    private var lastTick = DispatchTime.now()
    private var maxGap = 0.0
    private var windowGap = 0.0
    /// The longest gap since the first connected snapshot was installed: the steady state, after
    /// the window and the first read are over.
    private var steady = false
    private var steadyGap = 0.0
    private var ticks = 0
    private let started = Date()
    /// Writes that were handed to the sink, and how many of them failed.
    public private(set) var writes = 0
    public private(set) var failures = 0
    public private(set) var lastFailure: String?

    public init(file: URL, sink: Sink? = nil, encode: Encoder? = nil) {
        self.file = file
        self.sink = sink ?? { data, url in try data.write(to: url, options: .atomic) }
        self.encode = encode ?? { state in
            guard JSONSerialization.isValidJSONObject(state) else { throw Unencodable() }
            return try JSONSerialization.data(withJSONObject: state, options: [.sortedKeys])
        }
    }

    /// Start following `model`. The main-thread monitor ticks every few milliseconds; a long gap
    /// between ticks is the main thread having been blocked.
    public func attach(_ model: ObserverModel, layout: LayoutRegistry? = nil, host: HostObservation? = nil) {
        self.model = model
        self.layout = layout
        self.host = host
        model.onInstalled = { [weak self] in self?.changed() }
        layout?.onChange = { [weak self] in self?.changed() }
        host?.onChange = { [weak self] in self?.changed() }
        monitor.schedule(deadline: .now(), repeating: .milliseconds(5))
        monitor.setEventHandler { [weak self] in
            MainActor.assumeIsolated { self?.tick() }
        }
        monitor.resume()
        changed()
    }

    private func tick() {
        let now = DispatchTime.now()
        let gap = Double(now.uptimeNanoseconds - lastTick.uptimeNanoseconds) / 1_000_000
        lastTick = now
        ticks += 1
        maxGap = max(maxGap, gap)
        windowGap = max(windowGap, gap)
        if steady { steadyGap = max(steadyGap, gap) }
    }

    /// A snapshot was installed; write soon, at most every 50 ms, with one trailing write.
    public func changed() {
        let since = Date().timeIntervalSince(lastWrite)
        if since >= 0.05 {
            write()
        } else if !trailing {
            trailing = true
            Task { @MainActor [weak self] in
                try? await Task.sleep(nanoseconds: UInt64((0.05 - since) * 1_000_000_000))
                self?.trailing = false
                self?.write()
            }
        }
    }

    /// Describe the model now and hand it to the writer: at once if none is in flight, otherwise as
    /// the one waiting state, replacing whatever waited before it.
    public func write() {
        guard let model = model else { return }
        lastWrite = Date()
        if !steady, model.snapshot.connection == .connected {
            steady = true
            steadyGap = 0
        }
        let state = Self.describe(model, started: started, gaps: (maxGap, windowGap, steady ? steadyGap : nil), ticks: ticks, layout: layout, host: host)
        windowGap = 0
        if inFlight {
            waiting = state
        } else {
            start(state)
        }
    }

    private func start(_ described: [String: Any]) {
        // The write counters are read when the write starts, so a state that waited behind a failed
        // write carries that failure.
        var state = described
        state["state_writes"] = writes + 1
        state["state_write_failures"] = failures
        state["last_write_error"] = orNull(lastFailure)
        inFlight = true
        writes += 1
        let handover = Handover(state: state)
        let file = self.file, sink = self.sink, encode = self.encode
        // Encoding and writing both happen off the main actor, and either failing is reported the same
        // way: counted, kept, and carried in the next state that is written.
        writer.async { [weak self] in
            var failure: String?
            do { try sink(try encode(handover.state), file) } catch { failure = "\(error)" }
            Task { @MainActor in self?.finished(failure) }
        }
    }

    private func finished(_ failure: String?) {
        inFlight = false
        if let failure = failure {
            failures += 1
            lastFailure = failure
        }
        if let next = waiting {
            waiting = nil
            start(next)
        }
    }

    /// What the Gantt shows, read from the model the view draws: how many rows, which is selected, where
    /// the keyboard cursor is, and the focus the view itself reports (as `search_focused` is).
    static func gantt(_ model: ObserverModel, layout: LayoutRegistry?) -> [String: Any] {
        let rows = model.outline
        var selected: Any = NSNull()
        if case .work(let identity)? = model.selection { selected = model.snapshot.inventory.key(of: identity) ?? identity }
        let view = model.gantt
        let cursor = model.focusedRow?.row.key
        return [
            "loaded": model.snapshot.gantt != nil,
            "total_rows": model.snapshot.gantt?.rows.count ?? 0,
            "rows": rows.count,
            "selected_key": selected,
            "cursor_key": orNull(cursor),
            "focus_target": orNull(model.focusTarget),
            "focus_reported": orNull(model.focusReported),
            "detail_return": orNull(model.detailReturn?.element),
            "collapsed": view.collapsed.count,
            "filter": ["text": view.filter.text, "status": orNull(view.filter.status), "critical_only": view.filter.criticalOnly, "editing": view.editingFilter] as [String: Any],
            "zoom_level": view.zoomLevel,
            "pan": [view.panX, view.panY],
            "scale": view.scale,
            "fitted": view.fitScale != nil,
            // The whole timeline as drawn now, in timeline points (pan applied), and the timeline's measured width.
            "timeline_extent": orNull(model.snapshot.gantt.map { [-view.panX, GanttLayout.timelineWidth($0, scale: view.scale)] }),
            "viewport_width": view.viewportWidth,
            "locate_note": orNull(view.locateNote),
            "view_sequence": view.sequence,
            "revision": orNull(model.snapshot.gantt?.revision.map { Int($0) }),
            "relations": model.snapshot.inventory.relations.count,
            "keys": rows.prefix(60).map { $0.row.key },
            "viewport_rows": view.viewportRows,
            "row_height": GanttLayout.rowHeight,
            "layout": orNull(layout?.described()),
        ]
    }

    /// What the network shows, read from the model the view draws; computed only while it is the page shown.
    static func network(_ model: ObserverModel) -> [String: Any] {
        guard model.page == .network else { return ["shown": false] }
        let graph = model.networkGraph
        let counts = graph.counts
        let view = model.network
        return [
            "shown": true,
            "nodes": counts.nodes, "temporal": counts.temporal, "decision_blocks": counts.decisionBlocks, "context_links": counts.contextLinks,
            "listed_nodes": model.networkNodes.count,
            "visible_edges": model.networkVisibleEdges.count,
            "cursor_key": orNull(model.networkFocusedNode?.key),
            "focus_target": orNull(model.focusTarget),
            "focus_reported": orNull(model.focusReported),
            "detail_return": orNull(model.detailReturn?.element),
            "walked": orNull(view.walked),
            "explain_read": model.networkReport != nil,
            "filter": ["text": view.filter.text, "kinds": view.filter.kinds.map(\.rawValue).sorted(), "editing": view.editingFilter] as [String: Any],
            "zoom_level": view.zoomLevel,
            "pan": [view.panX, view.panY],
            "viewport": [view.viewportWidth, view.viewportHeight],
            "max_pan": { let most = NetworkLayout.maxPan(graph, scale: view.scale, viewport: (view.viewportWidth, view.viewportHeight)); return [most.x, most.y] }(),
            "scale": view.scale,
            "fitted": view.fitScale != nil,
            // The whole graph's extent as drawn now, from the canvas's top left (pan applied): Fit is whole when this lies
            // inside [0, 0, viewport].
            "extent_drawn": { () -> [Double] in let size = NetworkLayout.extent(graph); return [-view.panX, -view.panY, size.width * view.scale, size.height * view.scale] }(),
            "locate_note": orNull(view.locateNote),
            "hover_key": orNull(model.networkHover?.key),
            "hover_point": orNull(view.hoverPoint.map { [$0.x, $0.y] }),
            // Where each shown node is drawn now, in canvas points from the canvas's top left, for those inside the canvas
            // (bounded): the geometry a pointer aims at. The pixels themselves are the suite's to read.
            "drawn": { () -> [[String: Any]] in
                let scale = view.scale
                var out: [[String: Any]] = []
                for node in model.networkNodes {
                    guard out.count < 60, let place = graph.place[node.identity] else { continue }
                    let r = NetworkLayout.rect(place)
                    let x = r.x * scale - view.panX, y = r.y * scale - view.panY, w = r.width * scale, h = r.height * scale
                    guard view.viewportWidth > 0, x + w > 0, y + h > 0, x < view.viewportWidth, y < view.viewportHeight else { continue }
                    out.append(["key": node.key, "kind": node.kindText, "rect": [x, y, w, h]])
                }
                return out
            }(),
            "view_sequence": view.sequence,
        ]
    }

    static func describe(_ model: ObserverModel, started: Date, gaps: (max: Double, window: Double, steady: Double?), ticks: Int, layout: LayoutRegistry? = nil, host: HostObservation? = nil) -> [String: Any] {
        let snapshot = model.snapshot
        let millis = { (date: Date) in Int(date.timeIntervalSince1970 * 1000) }
        var connection = "idle"
        var detail: String?
        switch snapshot.connection {
        case .idle: connection = "idle"
        case .opening: connection = "opening"
        case .connected: connection = "connected"
        case .reconnecting(let attempt, let cause): connection = "reconnecting"; detail = "attempt \(attempt): \(cause)"
        case .sourceChanged(let message): connection = "source_changed"; detail = message
        case .failed(let message): connection = "failed"; detail = message
        case .closed: connection = "closed"
        }
        var selection: Any = NSNull()
        switch model.selection {
        case .work(let identity)?: selection = ["kind": "work", "id": identity, "key": snapshot.inventory.key(of: identity) ?? ""]
        case .run(let id)?: selection = ["kind": "run", "id": id]
        case .decision(let id)?: selection = ["kind": "decision", "id": id, "key": snapshot.inventory.decisionByIdentity[id]?.key ?? ""]
        case nil: break
        }
        let freshness = snapshot.freshness
        return [
            "pid": Int(ProcessInfo.processInfo.processIdentifier),
            "wrote_at_ms": millis(Date()),
            "installed_at_ms": millis(model.installedAt),
            "started_at_ms": millis(started),
            "page": model.page.rawValue.lowercased(),
            "search_focused": model.searchFocused,
            "helper_pid": orNull(snapshot.helperProcess.map { Int($0) }),
            "gantt": gantt(model, layout: layout),
            "network": network(model),
            "host": orNull(host?.described()),
            "selection": selection,
            "setup_error": orNull(model.setupError),
            "connection": connection,
            "connection_detail": orNull(detail),
            "source": orNull(snapshot.identity?.source),
            "workspace": orNull(snapshot.identity?.workspaceId),
            "lineage": orNull(snapshot.identity?.lineageId),
            "revision": orNull(snapshot.revision.map { Int($0) }),
            "evaluated_at": orNull(snapshot.freshness.evaluatedAt?.description),
            "generation": snapshot.generation,
            "sequence": Int(snapshot.sequence),
            "freshness": [
                "current": freshness.current, "project_stale": freshness.projectStale, "runs_stale": freshness.runsStale,
                "clock_due": freshness.clockDue, "catching_up": freshness.catchingUp, "last_error": orNull(freshness.lastError),
            ],
            "counts": [
                "candidates": snapshot.candidates.count, "runs": snapshot.runs.count, "submitted": snapshot.inventory.submitted.count,
                "operations": snapshot.operations.count, "blockers": snapshot.status?.blockers.count ?? 0, "inventory": snapshot.inventory.items.count,
                "decisions": snapshot.inventory.decisions.count,
            ],
            "runs_coverage": ["read": snapshot.runsCoverage.read, "limit": snapshot.runsCoverage.limit, "truncated": snapshot.runsCoverage.truncated],
            "work_runs": snapshot.workRuns.map { ["work": $0.work, "runs": $0.runs.count, "truncated": $0.coverage.truncated] as [String: Any] } ?? NSNull(),
            "inspection": ["views": snapshot.inspection.views.count, "cursors": snapshot.inspection.cursors],
            "next_keys": snapshot.candidates.map { $0.key },
            "submitted_keys": snapshot.inventory.submitted.map { $0.key },
            "runs": snapshot.runs.prefix(40).map { ["id": $0.identity, "work": $0.workKey, "state": $0.state, "status": $0.status, "observation": $0.observation.text, "recorded": $0.recorded] as [String: Any] },
            "detail": snapshot.detail.map { ["title": $0.title, "loading": $0.loading, "error": orNull($0.error), "sections": $0.sections.map { $0.title }] as [String: Any] } ?? NSNull(),
            "window": snapshot.window.map { ["run": $0.run, "entries": $0.entries.count, "dropped": $0.droppedFromView, "recorded": $0.recorded, "gap": orNull($0.gap), "incomplete": orNull($0.incomplete), "lifecycle_incomplete": orNull($0.lifecycleIncomplete)] as [String: Any] } ?? NSNull(),
            "counters": [
                "polls": snapshot.counters.polls, "project_reads": snapshot.counters.projectReads, "run_reads": snapshot.counters.runReads,
                "time_reads": snapshot.counters.timeReads, "activity_applied": snapshot.counters.activityApplied,
                "activity_discarded": snapshot.counters.activityDiscarded, "gaps": snapshot.counters.gaps, "resets": snapshot.counters.resets,
                "reconnects": snapshot.counters.reconnects,
            ],
            "main_thread": ["ticks": ticks, "max_gap_ms": gaps.max, "window_max_gap_ms": gaps.window, "steady_max_gap_ms": orNull(gaps.steady)],
        ]
    }
}
