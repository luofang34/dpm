// The worked sequences the Swift client proves against the real helper.
//
// Each scenario names the property it shows. Together they cover negotiation, snapshot-to-
// subscription handover with a write during startup and a page delivered twice, typed success and
// refusal, a clock-only change announced by the boundary, reads that telemetry cannot starve, link
// invalidation with a failed refresh and a reconnect, and a main thread that never takes part in
// the helper's launch, exchanges, kill or close.

import Foundation

final class Scenarios {
    let arguments: Arguments
    let monitor: MainThreadMonitor
    let checks = Checks()
    let audit = ThreadAudit()
    private(set) var client: NativeClient!

    init(arguments: Arguments, monitor: MainThreadMonitor) {
        self.arguments = arguments
        self.monitor = monitor
    }

    func launch() async throws -> NativeClient {
        try await NativeClient.launchHelper(executable: arguments.helper, arguments: ["--db", arguments.database], audit: audit)
    }

    func cli(_ words: [String]) async throws -> JSON {
        try await command(arguments.dpm, ["--database", arguments.database, "--json"] + words)
    }

    /// The helper ends abruptly and another is started on the same store: a reconnect.
    func reconnect() async throws {
        try await client.shutdown(kill: true)
        client = try await launch()
    }

    func run() async -> Bool {
        do {
            client = try await launch()
            try await negotiation()
            try await handover()
            try await compositeStartup()
            try await clockOnly()
            try await typedCommands()
            try await telemetry()
            try await linkInvalidation()
            try await contractEdges()
            try await mainThread()
            try await capture()
            try await lifecycleCycles()
            try await client.shutdown(kill: false)
            try await lifecycleAudit()
        } catch {
            checks.check(false, "the proof stopped on an error: \(error)")
        }
        print("\(checks.passed) checks passed, \(checks.failures.count) failed")
        return !checks.failures.isEmpty
    }

    // MARK: Negotiation

    func negotiation() async throws {
        print("negotiation and capabilities")
        let hello = try await client.hello([9, 1])
        checks.check(hello.protocolVersion == 1 && hello.supported == [1], "the highest common protocol is chosen")
        checks.check(hello.capabilities.views.map { $0.view } == ["now", "live", "review", "detail"], "the four macOS views are mapped")
        checks.check(hello.capabilities.commands, "a live store accepts commands")
        let controls = hello.capabilities.unsupported.map { $0.control }
        checks.check(["steer_run", "stop_run", "answer_input_request", "provider_session_control"].allSatisfy(controls.contains), "unsupported controls are stated, not discovered by failure")
        checks.check(hello.capabilities.runs.observation == ["managed", "reported_only"], "managed and reported-only observation are distinguished")
        checks.check(hello.capabilities.limits.maxPage == 1000 && hello.capabilities.limits.staleAfterSeconds == 300, "bounds are advertised")
        await checks.refused(.unsupportedProtocol, "an unsupported protocol offer") { _ = try await self.client.hello([9]) }
        let future = try await client.response(.attach(expectWorkspace: nil), protocolVersion: 2)
        checks.check(future.error?.code == "unsupported_protocol" && future.error?.details?["supported"][0] == .integer(1), "a newer client's request is refused for its version and names what is supported")
    }

    // MARK: Snapshot to subscription

    func handover() async throws {
        print("snapshot-to-subscription handover with a write during startup and a page delivered twice")
        let attached = try await client.attach()
        checks.check(attached.source == "live" && attached.watermark.project.historyHead == 0, "attach names a live source at the start of history")
        let snapshot = try await client.query(statusQuery)
        checks.check(snapshot.basis.project?.historyHead == 0, "the snapshot names the position it is anchored to")
        checks.check(snapshot.basis.runs == nil, "a project-only answer carries no run basis")

        // Another process commits before the client has subscribed.
        _ = try await cli(["claim", "TEST-A", "--actor", "agent:worker"])
        let consumer = Consumer(views: [snapshot])
        let first = try await consumer.poll(client)
        checks.check(consumer.project == [1] && first.project?.more == false, "the first poll delivers the write once and is caught up")
        checks.check(first.watermark.project.historyHead == 1, "the answer's watermark is the head the cursor reached")

        // A response lost in transit: the same poll from the same cursor returns the same page.
        let retry = Consumer(views: [snapshot])
        try await retry.poll(client)
        checks.check(retry.project == [1], "a repeated poll returns the same page, never a different one")

        // The very same page delivered twice to the same consumer: applied once, discarded by
        // identity and sequence the second time, cursors unmoved.
        let page = try await client.changes(Consumer(views: [snapshot]).cursors)
        let repeated = Consumer(views: [snapshot])
        repeated.apply(page)
        let cursors = repeated.cursors
        repeated.apply(page)
        repeated.apply(page)
        checks.check(repeated.project == [1] && repeated.discarded == 2, "a page already applied is discarded by feed identity and sequence")
        checks.check(repeated.cursors.project == cursors.project, "a repeated page does not move the cursor")

        let quiet = try await consumer.poll(client)
        checks.check(quiet.project?.entries.isEmpty == true && consumer.project == [1], "polling from the advanced cursor adds nothing")

        // The response to a refresh requested before the write arrives late, after the poll saw the
        // write. It was anchored before the write, so it cannot make the client current, and empty
        // polls afterwards do not either; only a view read after the write does.
        checks.check(consumer.projectStale, "the client is stale after the poll delivered the write")
        consumer.install([snapshot])
        checks.check(consumer.projectStale, "a delayed response anchored before the write leaves it stale")
        try await consumer.poll(client)
        try await consumer.poll(client)
        checks.check(consumer.projectStale, "empty polls do not make it current")
        try await consumer.refresh(client, queries: [statusQuery])
        checks.check(!consumer.projectStale, "a view read after the write does")
        // The old response arrives once more, after the fresh one was installed: the display is
        // stale again, and an empty poll does not hide it.
        consumer.install([snapshot])
        checks.check(consumer.projectStale, "an old response installed after a fresh one makes the display stale again")
        try await consumer.poll(client)
        checks.check(consumer.projectStale, "an empty poll does not hide it")
        try await consumer.refresh(client, queries: [statusQuery])
        checks.check(!consumer.projectStale, "reading again makes it current")

        // The producer is replaceable: a new helper continues from the client's cursors.
        _ = try await cli(["start", "TEST-A", "--actor", "agent:worker"])
        try await reconnect()
        try await consumer.drain(client)
        checks.check(consumer.project == [1, 2] && consumer.resets.isEmpty && consumer.identityResets.isEmpty, "after a helper restart the cursors continue without a hole or a repeat")
    }

    // MARK: Composite startup

    func compositeStartup() async throws {
        print("a client seeded from several views is anchored at the earliest")
        let older = try await client.query(statusQuery)
        _ = try await cli(["progress", "TEST-A", "10", "--actor", "agent:worker"])
        let newer = try await client.query(query(["query": .string("next"), "capabilities": .array([]), "probabilistic": .bool(false), "limit": .integer(5)]))
        checks.check((newer.basis.project?.historyHead ?? 0) > (older.basis.project?.historyHead ?? 0), "the second view was read after the operation the first lacks")
        for held in [[newer, older], [older, newer]] {
            let consumer = Consumer(views: held)
            try await consumer.poll(client)
            checks.check(consumer.project.count == 1, "the first poll delivers the operation the older view lacks, whichever view is listed first")
            consumer.install([newer, older])
            checks.check(consumer.projectStale, "installing both views is not enough: the older one is behind what the poll saw")
        }
    }

    // MARK: Time passing

    func clockOnly() async throws {
        print("a clock-only change, announced by the boundary")
        let show = try await client.query(query(["query": .string("show"), "key": .string("TEST-A")]))
        guard let started = show.envelope.data["execution"]["events"]["started_at"].string.flatMap(Instant.init) else {
            checks.check(false, "TEST-A has a start time")
            return
        }
        let opens = started.adding(seconds: 1800)
        try await setClock(client, started.adding(seconds: 1799))
        let waiting = try await client.query(explain("TEST-B"))
        checks.check(waiting.envelope.data["ready"].bool == false, "the shared evaluator reports TEST-B waiting one second before its lag ends")
        checks.check(waiting.refreshAt == opens, "the boundary names the exact instant the answer changes")
        try await setClock(client, waiting.refreshAt)
        let released = try await client.query(explain("TEST-B"))
        checks.check(released.envelope.data["ready"].bool == true && released.evaluatedAt == opens, "re-evaluating at that instant releases it")
        checks.check(released.basis == waiting.basis && released.refreshAt == nil, "no project operation, no feed entry and no further deadline")
        let idle = try await Consumer(views: [released]).poll(client)
        checks.check(idle.project?.entries.isEmpty == true && idle.evaluatedAt == opens, "the project feed saw nothing: time passing is not a project change")
        try await setClock(client, nil)
    }

    // MARK: Typed commands

    var submittedOperation: String?

    func typedCommands() async throws {
        print("typed success and typed rejection")
        _ = try await cli(["run", "start", "TEST-A", "--actor", "agent:worker", "--run-id", runId])
        let show = try await client.query(query(["query": .string("show"), "key": .string("TEST-A")]))
        guard let work = show.envelope.data["id"].string, let head = show.basis.project?.historyHead,
              let revision = show.basis.project?.revision
        else {
            checks.check(false, "TEST-A has an identity and the view is anchored")
            return
        }
        func submit(base: UInt64) -> JSON {
            .object([
                "actor": worker,
                "base_revision": .unsigned(base),
                "command": .object(["Submit": .object(["work": .string(work), "note": .null])]),
            ])
        }
        await checks.refused(.revisionConflict, "a command at a stale revision") {
            _ = try await self.client.command(submit(base: revision &- 1))
        }
        checks.check(try await client.attach().watermark.project.historyHead == head, "the refused command changed nothing")
        let committed = try await client.command(submit(base: revision))
        checks.check(committed.envelope.data["resulting_revision"].uint != nil, "a command at the observed revision is committed with its own envelope")
        submittedOperation = committed.envelope.data["id"].string
        checks.check(submittedOperation != nil, "the recorded operation is named")
        checks.check(try await client.attach().watermark.project.historyHead == head + 1, "the new operation is the next project feed entry")
    }

    // MARK: Telemetry does not starve reads

    /// Set by the writer when its last heartbeat is recorded, so the reader's loop is driven by an
    /// event and not by a delay.
    actor Finished {
        private var done = false
        func set() { done = true }
        var value: Bool { done }
    }

    func telemetry() async throws {
        print("reads stay answerable while run telemetry continues")
        let beats = 20
        let finished = Finished()
        let writer = Task { () -> Int in
            for sequence in 1...beats {
                _ = try await self.cli(["run", "record", runId, "heartbeat", "--sequence", "\(sequence)", "--actor", "agent:worker"])
            }
            await finished.set()
            return beats
        }
        var project = 0, runs = 0
        var lowerBound = true
        var projectOnly = true
        // The reader starts while heartbeats are landing and continues until the last one has.
        while true {
            let status = try await client.query(statusQuery)
            projectOnly = projectOnly && status.basis.runs == nil
            project += 1
            let view = try await client.query(query(["query": .string("run"), "id": .string(runId)]))
            let recorded = view.envelope.data["activity"]["recorded"].uint ?? 0
            lowerBound = lowerBound && (view.basis.runs?.activityHead ?? .max) <= recorded
            runs += 1
            if await finished.value { break }
        }
        _ = try await writer.value
        checks.check(project >= 1 && runs >= 1, "\(project) project queries and \(runs) run queries were answered during \(beats) heartbeats")
        checks.check(projectOnly, "no project-only answer was held up by, or stamped with, run activity")
        checks.check(lowerBound, "every run answer's anchor is a lower bound of what it holds")
        let settled = try await client.query(query(["query": .string("run"), "id": .string(runId)]))
        checks.check(settled.basis.runs?.activityHead == UInt64(beats), "the activity position is the last heartbeat once the writer is done")
    }

    // MARK: Links, a failed refresh and a reconnect

    func linkInvalidation() async throws {
        print("a link invalidates the run views; a failed refresh and a reconnect do not forget it")
        guard let operation = submittedOperation else {
            checks.check(false, "the submitted operation is known")
            return
        }
        // A Now view is several queries: ranked work (project only) and the runs it shows.
        let nextQuery = query(["query": .string("next"), "capabilities": .array([]), "probabilistic": .bool(false), "limit": .integer(5)])
        let held = try await client.query(runsQuery)
        let consumer = Consumer(views: [held])
        _ = try await cli(["run", "link", runId, operation, "--actor", "agent:worker"])

        let first = try await consumer.poll(client)
        checks.check(first.links?.changed == true && consumer.runsStale, "the poll says the displayed runs are stale")
        checks.check(first.project?.entries.isEmpty == true && first.lifecycle?.entries.isEmpty == true && first.activity?.entries.isEmpty == true,
                     "a link is not a project operation, a transition or activity: only the token reports it")
        checks.check(first.links?.count == first.watermark.runs.linkCount, "the signal and the watermark agree")

        await checks.refused(.workspaceMismatch, "a refresh refused by the boundary") {
            _ = try await consumer.refresh(self.client, queries: [runsQuery], attached: wrongWorkspace)
        }
        checks.check(consumer.runsStale, "a failed refresh leaves the views stale")

        try await reconnect()
        let after = try await consumer.poll(client)
        checks.check(after.links?.changed == true && consumer.runsStale, "after a reconnect the next poll still reports the change")

        // Refreshing only the project-only part of a Now view cannot mark the run views current.
        try await consumer.refresh(client, queries: [nextQuery])
        checks.check(consumer.runsStale, "a project-only refresh does not clear run staleness")
        let refreshed = try await consumer.refresh(client, queries: [nextQuery, runsQuery])
        checks.check(refreshed[1].envelope.data["runs"][0]["operations"].array?.count == 1, "the refreshed view lists the linked operation")
        checks.check(!consumer.dirty, "installing the refreshed set clears it")
        let quiet = try await consumer.poll(client)
        checks.check(quiet.links?.changed == false && !consumer.dirty && consumer.resets.isEmpty, "the client is then current")
    }

    // MARK: Edges of the contract

    func contractEdges() async throws {
        print("malformed requests and codes this client does not know")
        let malformed = try await client.raw("not json")
        let decoded = try JSONDecoder().decode(Response.self, from: Data(malformed.utf8))
        checks.check(decoded.ok == false && ErrorCode(decoded.error?.code ?? "") == .invalidRequest, "a malformed line gets a typed refusal, never silence")
        checks.check(ErrorCode("a_code_from_a_newer_host") == .unknown("a_code_from_a_newer_host"), "an unknown code is kept as unknown, never guessed")
        let unknownResult = try JSONDecoder().decode(Response.self, from: Data(#"{"protocol":1,"id":"x","ok":true,"result":{"kind":"from_the_future"}}"#.utf8))
        if case .unknown(let kind)? = unknownResult.result {
            checks.check(kind == "from_the_future", "an unknown result kind is kept, not decoded as another")
        } else {
            checks.check(false, "an unknown result kind is kept")
        }
    }

    // MARK: Lifecycle

    /// Repeated clean and abrupt lifecycles, a helper that has already exited, and ending twice. Every
    /// wait is on the child's exit event with a bound, so none of them can hang, and each reports a
    /// typed failure instead if the child never exits.
    func lifecycleCycles() async throws {
        print("repeated clean and abrupt helper lifecycles, including a child that already exited")
        var healthy = true
        for round in 0..<6 {
            let cycle = try await launch()
            let attached = try await cycle.attach()
            healthy = healthy && attached.source == "live"
            try await cycle.shutdown(kill: round % 2 == 1)
            let gone = await cycle.awaitExit(timeout: exitBound)
            healthy = healthy && gone
            // Ending an already-ended client is not an error and does not wait.
            try await cycle.shutdown(kill: round % 2 == 0)
        }
        checks.check(healthy, "6 launch, attach and clean or abrupt shutdown cycles each saw the exit event")

        // A helper started without arguments exits by itself at once. Its event has fired by the time
        // it is closed or killed, so neither waits, and an exchange reports the closed helper.
        let executable = arguments.helper
        let early = try await NativeClient.launch(audit: audit) { try HelperTransport(executable: executable, arguments: [], audit: self.audit) }
        checks.check(await early.awaitExit(timeout: exitBound), "a child that exits on its own is observed through its exit event")
        await checks.refusedTransport("an exchange with an exited helper") { _ = try await early.attach() }
        try await early.shutdown(kill: false)
        let again = try await NativeClient.launch(audit: audit) { try HelperTransport(executable: executable, arguments: [], audit: self.audit) }
        _ = await again.awaitExit(timeout: exitBound)
        try await again.shutdown(kill: true)
        checks.check(true, "closing and killing a child that already exited return without waiting")
    }

    // MARK: Responsiveness

    func mainThread() async throws {
        print("the main thread stays responsive while the helper works")
        monitor.reset()
        for _ in 0..<40 {
            _ = try await client.query(query(["query": .string("status"), "probabilistic": .bool(true)]))
        }
        let measured = monitor.snapshot
        checks.check(measured.ticks >= 20, "the main thread kept ticking during 40 helper calls (\(measured.ticks) ticks)")
        checks.check(measured.maxGapMilliseconds < 250, "its longest stall was \(Int(measured.maxGapMilliseconds)) ms")
    }

    /// After the whole lifecycle: launch, exchanges, the abrupt end of a helper and the clean close
    /// all ran, and none of them on the main thread. A control that does run them on the main
    /// thread is detected, so the assertion means something.
    func lifecycleAudit() async throws {
        print("the helper's whole lifecycle stayed off the main thread")
        let steps = Set(audit.steps)
        checks.check(["launch", "exchange", "kill", "close"].allSatisfy(steps.contains), "launch, exchange, kill and close were all exercised (\(audit.steps.count) steps)")
        checks.check(audit.mainThreadSteps.isEmpty, "none of them ran on the main thread: \(audit.mainThreadSteps)")

        let control = ThreadAudit()
        let executable = arguments.helper
        let database = arguments.database
        // The main dispatch queue is the main thread, which the monitor's timer also relies on.
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.main.async {
                if let transport = try? HelperTransport(executable: executable, arguments: ["--db", database], audit: control) {
                    _ = try? transport.exchange(#"{"protocol":1,"id":"m","call":{"type":"attach"}}"#)
                    try? transport.close()
                }
                continuation.resume()
            }
        }
        checks.check(Set(control.mainThreadSteps) == ["launch", "exchange", "close"], "the same steps made on the main thread are detected: \(control.mainThreadSteps) of \(control.steps)")
    }

    // MARK: Payloads for comparison with the CLI and the agent tools

    func capture() async throws {
        guard let out = arguments.out else { return }
        print("payloads at one pinned clock for comparison with real CLI and agent-tool output")
        let at = Instant(arguments.clock)
        try await setClock(client, at)
        // The defaults the CLI and the agent tools apply, spelled out: forecasts on, five ranked tasks.
        let pairs: [(String, JSON)] = [
            ("status", query(["query": .string("status"), "probabilistic": .bool(true)])),
            ("next", query(["query": .string("next"), "capabilities": .array([]), "probabilistic": .bool(true), "limit": .integer(5)])),
            ("explain", explain("TEST-B")),
            ("show", query(["query": .string("show"), "key": .string("TEST-A")])),
            ("history", query(["query": .string("history"), "after_sequence": .integer(0), "limit": .integer(100)])),
            ("runs", runsQuery),
            ("revision", query(["query": .string("revision")])),
        ]
        var payloads: [String: JSON] = [:]
        for (name, request) in pairs {
            let view = try await client.query(request)
            payloads[name] = envelopeJSON(view)
            checks.check(view.evaluatedAt == at, "\(name) was evaluated at the pinned instant")
        }
        let data = try JSONEncoder().encode(JSON.object(payloads))
        try data.write(to: URL(fileURLWithPath: out))
        try await setClock(client, nil)
    }
}
