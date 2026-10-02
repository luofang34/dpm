// The UI-10 contract, held through the real bridge: negotiation, snapshot-to-subscription handover,
// a client seeded from several views, a clock-only change announced by the boundary, typed
// commands, reads that telemetry cannot starve, and link invalidation across a failed refresh and a
// replaced helper. The clock is pinned only by the helper's startup option, never by a request.

import DPMNative
import Foundation

extension Context {
    private var wrongWorkspace: Attachment { Attachment(workspaceId: UUIDv7.make(), lineageId: nil) }

    func contractThroughTheBridge() async throws {
        let database = try await newStore("contract", plan: tools.laggedPlan)
        let runId = UUIDv7.make()
        let connection = try await open(.database(database))
        try await negotiation(connection)
        try await handover(connection, database)
        try await compositeStartup(connection, database)
        try await clockOnly(database)
        let operation = try await typedCommands(connection, database, runId: runId)
        try await telemetry(connection, database, runId: runId)
        try await linkInvalidation(connection, database, runId: runId, operation: operation)
        try await contractEdges()
        try await connection.close()
    }

    /// The consumer keeps a bounded window of what it applied for diagnostics. What it decides
    /// from, the last sequence applied and the cursors, does not live in that window.
    func boundedConsumer() async throws {
        print("a consumer retains a bounded window and loses nothing it depends on")
        let database = try await newStore("bounded-consumer")
        let connection = try await open(.database(database))
        let snapshot = try await connection.query(statusQuery)
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        for percent in stride(from: 10, through: 80, by: 10) { try await worker(database, ["progress", "TEST-A", "\(percent)"]) }
        let everything = try await connection.changes(Consumer(views: [snapshot]).cursors)
        let consumer = Consumer(views: [snapshot], retaining: 3)
        let pages = try await consumer.drain(connection, limit: 4)
        check(pages >= 3, "ten operations arrived in several pages (\(pages))")
        check(consumer.applied.project == 10 && consumer.project == [8, 9, 10], "all ten were applied; only the most recent three are retained: \(consumer.project)")
        check(consumer.cursors.project?.afterSequence == 10, "the cursor is at the end of what was applied")
        consumer.apply(everything)
        check(consumer.discarded == 10 && consumer.applied.project == 10 && consumer.project == [8, 9, 10], "a page applied long ago is still discarded by the last sequence, not by the window")
        check(consumer.unreadable == 0, "every entry had a readable sequence")
        try await connection.close()
    }

    private func negotiation(_ connection: NativeConnection) async throws {
        print("negotiation and capabilities")
        guard case .hello(let hello) = try await connection.result(.hello(protocols: [9, 1])) else {
            check(false, "hello answers with a hello")
            return
        }
        check(hello.protocolVersion == 1 && hello.supported == [1], "the highest common protocol is chosen")
        check(hello.capabilities.runs.observation == ["managed", "reported_only"], "managed and reported-only observation are distinguished")
        check(hello.capabilities.limits.maxPage == 1000 && hello.capabilities.limits.staleAfterSeconds == 300, "bounds are advertised")
        await checks.refused(.unsupportedProtocol, "an unsupported protocol offer") { _ = try await connection.result(.hello(protocols: [9])) }
        let future = try await connection.send(.attach(expectWorkspace: nil), protocolVersion: 2)
        check(future.error?.code == "unsupported_protocol" && future.error?.details?["supported"][0] == .integer(1), "a newer client's request is refused for its version and names what is supported")
        check(connection.isOpen, "a refused version leaves the connection usable")
    }

    private func handover(_ connection: NativeConnection, _ database: URL) async throws {
        print("snapshot-to-subscription handover with a write during startup and a page delivered twice")
        let attached = try await connection.attach()
        check(attached.source == "live" && attached.watermark.project.historyHead == 0, "attach names a live source at the start of history")
        let snapshot = try await connection.query(statusQuery)
        check(snapshot.basis.project?.historyHead == 0, "the snapshot names the position it is anchored to")
        check(snapshot.basis.runs == nil, "a project-only answer carries no run basis")

        // Another process commits before the client has subscribed.
        try await worker(database, ["claim", "TEST-A"])
        let consumer = Consumer(views: [snapshot])
        let first = try await consumer.poll(connection)
        check(consumer.project == [1] && first.project?.more == false, "the first poll delivers the write once and is caught up")
        check(first.watermark.project.historyHead == 1, "the answer's watermark is the head the cursor reached")

        let retry = Consumer(views: [snapshot])
        try await retry.poll(connection)
        check(retry.project == [1], "a repeated poll returns the same page, never a different one")

        let page = try await connection.changes(Consumer(views: [snapshot]).cursors)
        let repeated = Consumer(views: [snapshot])
        repeated.apply(page)
        let cursors = repeated.cursors
        repeated.apply(page)
        repeated.apply(page)
        check(repeated.project == [1] && repeated.discarded == 2, "a page already applied is discarded by feed identity and sequence")
        check(repeated.cursors.project == cursors.project, "a repeated page does not move the cursor")

        let quiet = try await consumer.poll(connection)
        check(quiet.project?.entries.isEmpty == true && consumer.project == [1], "polling from the advanced cursor adds nothing")

        // A response to a refresh requested before the write arrives late: anchored before the write,
        // it cannot make the client current, and empty polls afterwards do not either.
        check(consumer.projectStale, "the client is stale after the poll delivered the write")
        consumer.install([snapshot])
        check(consumer.projectStale, "a delayed response anchored before the write leaves it stale")
        try await consumer.poll(connection)
        try await consumer.poll(connection)
        check(consumer.projectStale, "empty polls do not make it current")
        try await consumer.refresh(connection, queries: [statusQuery])
        check(!consumer.projectStale, "a view read after the write does")
        consumer.install([snapshot])
        check(consumer.projectStale, "an old response installed after a fresh one makes the display stale again")
        try await consumer.poll(connection)
        check(consumer.projectStale, "an empty poll does not hide it")
        try await consumer.refresh(connection, queries: [statusQuery])
        check(!consumer.projectStale, "reading again makes it current")

        // The producer is replaceable: the connection's own reconnect continues from the client's cursors.
        try await worker(database, ["start", "TEST-A"])
        _ = try await connection.reconnect()
        try await consumer.drain(connection)
        check(consumer.project == [1, 2] && consumer.resets.isEmpty && consumer.identityResets.isEmpty, "after a helper restart the cursors continue without a hole or a repeat")
    }

    private func compositeStartup(_ connection: NativeConnection, _ database: URL) async throws {
        print("a client seeded from several views is anchored at the earliest")
        let older = try await connection.query(statusQuery)
        try await worker(database, ["progress", "TEST-A", "10"])
        let newer = try await connection.query(query("next", ["capabilities": .array([]), "probabilistic": .bool(false), "limit": .integer(5)]))
        check((newer.basis.project?.historyHead ?? 0) > (older.basis.project?.historyHead ?? 0), "the second view was read after the operation the first lacks")
        for held in [[newer, older], [older, newer]] {
            let consumer = Consumer(views: held)
            try await consumer.poll(connection)
            check(consumer.project.count == 1, "the first poll delivers the operation the older view lacks, whichever view is listed first")
            consumer.install([newer, older])
            check(consumer.projectStale, "installing both views is not enough: the older one is behind what the poll saw")
        }
    }

    /// The clock is a startup option, so a clock-only change is read through helpers started at
    /// two instants of the same, unchanged store.
    private func clockOnly(_ database: URL) async throws {
        print("a clock-only change, announced by the boundary")
        let reader = try await open(.database(database))
        let show = try await reader.query(query("show", ["key": .string("TEST-A")]))
        try await reader.close()
        guard let started = show.envelope.data["execution"]["events"]["started_at"].string.flatMap(Instant.init) else {
            check(false, "TEST-A has a start time")
            return
        }
        let opens = started.adding(seconds: 1800)
        let before = try await open(.database(database), clock: started.adding(seconds: 1799))
        let waiting = try await before.query(query("explain", ["key": .string("TEST-B")]))
        check(waiting.envelope.data["ready"].bool == false, "the shared evaluator reports TEST-B waiting one second before its lag ends")
        check(waiting.refreshAt == opens, "the boundary names the exact instant the answer changes")
        try await before.close()
        guard let boundary = waiting.refreshAt else { return }
        let after = try await open(.database(database), clock: boundary)
        let released = try await after.query(query("explain", ["key": .string("TEST-B")]))
        check(released.envelope.data["ready"].bool == true && released.evaluatedAt == opens, "re-evaluating at that instant releases it")
        check(released.basis == waiting.basis && released.refreshAt == nil, "no project operation, no feed entry and no further deadline")
        let idle = try await Consumer(views: [released]).poll(after)
        check(idle.project?.entries.isEmpty == true && idle.evaluatedAt == opens, "the project feed saw nothing: time passing is not a project change")
        try await after.close()
    }

    private func typedCommands(_ connection: NativeConnection, _ database: URL, runId: String) async throws -> String? {
        print("typed success and typed rejection")
        _ = try await cliJSON(database, ["run", "start", "TEST-A", "--actor", "agent:worker", "--run-id", runId])
        let show = try await connection.query(query("show", ["key": .string("TEST-A")]))
        guard let work = show.envelope.data["id"].string, let head = show.basis.project?.historyHead, let revision = show.basis.project?.revision else {
            check(false, "TEST-A has an identity and the view is anchored")
            return nil
        }
        func submit(base: UInt64) -> CommandRequest {
            CommandRequest(actor: workerActor, baseRevision: base, command: .object(["Submit": .object(["work": .string(work), "note": .null])]))
        }
        await checks.refused(.revisionConflict, "a command at a stale revision") { _ = try await connection.execute(submit(base: revision &- 1)) }
        let unchanged = try await connection.attach().watermark.project.historyHead
        check(unchanged == head, "the refused command changed nothing")
        let committed = try await connection.execute(submit(base: revision))
        check(committed.envelope.data["resulting_revision"].uint != nil, "a command at the observed revision is committed with its own envelope")
        let operation = committed.envelope.data["id"].string
        check(operation != nil, "the recorded operation is named")
        let advanced = try await connection.attach().watermark.project.historyHead
        check(advanced == head + 1, "the new operation is the next project feed entry")
        return operation
    }

    private func telemetry(_ connection: NativeConnection, _ database: URL, runId: String) async throws {
        print("reads stay answerable while run telemetry continues")
        let beats = 20
        let finished = Event()
        let writer = Task { () -> Void in
            for sequence in 1...beats {
                _ = try await self.cliJSON(database, ["run", "record", runId, "heartbeat", "--sequence", "\(sequence)", "--actor", "agent:worker"])
            }
            finished.signal()
        }
        var project = 0, runs = 0
        var lowerBound = true, projectOnly = true
        let runQuery = query("run", ["id": .string(runId)])
        // The reader starts while heartbeats are landing and continues until the last one has.
        while true {
            let status = try await connection.query(statusQuery)
            projectOnly = projectOnly && status.basis.runs == nil
            project += 1
            let view = try await connection.query(runQuery)
            let recorded = view.envelope.data["activity"]["recorded"].uint ?? 0
            lowerBound = lowerBound && (view.basis.runs?.activityHead ?? .max) <= recorded
            runs += 1
            if finished.isSignalled { break }
        }
        try await writer.value
        check(project >= 1 && runs >= 1, "\(project) project queries and \(runs) run queries were answered during \(beats) heartbeats")
        check(projectOnly, "no project-only answer was held up by, or stamped with, run activity")
        check(lowerBound, "every run answer's anchor is a lower bound of what it holds")
        let settled = try await connection.query(runQuery)
        check(settled.basis.runs?.activityHead == UInt64(beats), "the activity position is the last heartbeat once the writer is done")
    }

    private func linkInvalidation(_ connection: NativeConnection, _ database: URL, runId: String, operation: String?) async throws {
        print("a link invalidates the run views; a failed refresh and a reconnect do not forget it")
        guard let operation = operation else {
            check(false, "the submitted operation is known")
            return
        }
        let nextQuery = query("next", ["capabilities": .array([]), "probabilistic": .bool(false), "limit": .integer(5)])
        let held = try await connection.query(runsQuery)
        let consumer = Consumer(views: [held])
        _ = try await cliJSON(database, ["run", "link", runId, operation, "--actor", "agent:worker"])

        let first = try await consumer.poll(connection)
        check(first.links?.changed == true && consumer.runsStale, "the poll says the displayed runs are stale")
        check(first.project?.entries.isEmpty == true && first.lifecycle?.entries.isEmpty == true && first.activity?.entries.isEmpty == true,
              "a link is not a project operation, a transition or activity: only the token reports it")
        check(first.links?.count == first.watermark.runs.linkCount, "the signal and the watermark agree")

        await checks.refused(.workspaceMismatch, "a refresh refused by the boundary") {
            _ = try await consumer.refresh(connection, queries: [runsQuery], attached: self.wrongWorkspace)
        }
        check(consumer.runsStale, "a failed refresh leaves the views stale")

        _ = try await connection.reconnect()
        let after = try await consumer.poll(connection)
        check(after.links?.changed == true && consumer.runsStale, "after a reconnect the next poll still reports the change")

        try await consumer.refresh(connection, queries: [nextQuery])
        check(consumer.runsStale, "a project-only refresh does not clear run staleness")
        let refreshed = try await consumer.refresh(connection, queries: [nextQuery, runsQuery])
        check(refreshed[1].envelope.data["runs"][0]["operations"].array?.count == 1, "the refreshed view lists the linked operation")
        check(!consumer.dirty, "installing the refreshed set clears it")
        let quiet = try await consumer.poll(connection)
        check(quiet.links?.changed == false && !consumer.dirty && consumer.resets.isEmpty, "the client is then current")
    }

    private func contractEdges() async throws {
        print("codes and results this client does not know")
        check(ErrorCode("a_code_from_a_newer_host") == .unknown("a_code_from_a_newer_host"), "an unknown code is kept as unknown, never guessed")
        let future = try JSONDecoder().decode(Response.self, from: Data(#"{"protocol":1,"id":"x","ok":true,"result":{"kind":"from_the_future"}}"#.utf8))
        if case .unknown(let kind)? = future.result {
            check(kind == "from_the_future", "an unknown result kind is kept, not decoded as another")
        } else {
            check(false, "an unknown result kind is kept")
        }
    }
}
