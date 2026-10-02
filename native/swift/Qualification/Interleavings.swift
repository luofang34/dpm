// Interleavings that decide whether a client can be trusted, each run for real: an open cancelled
// while its launch is held, a command whose answer is lost or wrong, a full queue, a storm of
// cancellations, and a queued command that must never move to another helper. Every wait is for an
// event the library or the kernel reports; the only helper stops are SIGSTOP/SIGCONT.

import DPMNative
import Foundation

extension Context {
    private func claimWork(_ work: String) -> CommandRequest {
        CommandRequest(actor: workerActor, baseRevision: 0, command: .object(["Claim": .object(["work": .string(work)])]))
    }

    private func notSentClosed(_ error: BridgeError) -> Bool {
        if case .connectionClosed(let fate) = error { return fate == .notSent }
        return false
    }

    /// Hold the open at one of its blocking steps, cancel it, and let it go.
    private func cancelledOpen(heldAt held: String, _ name: String) async throws {
        let database = try await newStore("open-cancel-\(name)")
        let reached = Event()
        let cancelled = Event()
        let release = DispatchSemaphore(value: 0)
        let once = LockedBox(true)
        var configuration = self.configuration(.database(database))
        let audited = configuration.onBlockingStep
        configuration.onBlockingStep = { step in
            audited?(step)
            if step == held, once.withLock({ let first = $0; $0 = false; return first }) {
                reached.signal()
                _ = release.wait(timeout: .now() + 30)
            }
            if step == "shutdown" { cancelled.signal() }
        }
        let launching = configuration
        let opening = Task.detached { try await NativeConnection.open(launching) }
        check(await reached.wait(), "\(name): the open reached the held \(held) step")
        opening.cancel()
        check(await cancelled.wait(), "\(name): the cancellation's shutdown ran while the \(held) step was still held")
        release.signal()
        switch await opening.result {
        case .failure(let error as BridgeError):
            if case .cancelled(let fate) = error {
                check(fate == .notSent, "\(name): a cancelled open is refused, and nothing was sent")
            } else {
                check(false, "\(name): a cancelled open should report cancellation: \(error)")
            }
        case .failure(let error):
            check(false, "\(name): a cancelled open failed with a non-bridge error \(error)")
        case .success(let connection):
            _ = try? await connection.close()
            check(false, "\(name): a cancelled open returned a connection")
        }
        let left = try await helpersRunning(on: database)
        check(!left, "\(name): no helper is left running for the cancelled open")
    }

    func cancelledOpenings() async throws {
        print("an open cancelled while it is held returns no connection and leaves no helper")
        try await cancelledOpen(heldAt: "launch", "before-launch")
        try await cancelledOpen(heldAt: "exchange", "during-negotiation")
    }

    /// Faults that arrive after a command was sent: the proxy forwards it, so it commits, and then
    /// the answer is lost, corrupt, wrong, or never comes.
    func commandFaults() async throws {
        print("a command whose answer fails after it was sent has an unknown outcome, and may have committed")
        let modes: [(String, TimeInterval)] = [
            ("mismatch", 10), ("ok-false", 10), ("ok-true-error", 10), ("error-api", 10), ("wrong-kind", 10), ("garbage", 10), ("truncate", 10), ("exit", 10), ("oversize", 10), ("swallow", 1.5),
        ]
        for (mode, timeout) in modes {
            let database = try await newStore("fault-\(mode)")
            let work = try await cliJSON(database, ["show", "TEST-A"])["data"]["id"].string ?? ""
            let request = claimWork(work)
            let connection = try await open(.database(database), helper: faultyHelper(mode), adjust: {
                $0.requestTimeout = timeout
                $0.maxResponseBytes = 1 << 20
                $0.shutdownBound = 2
            })
            await checks.fails("\(mode): the command's failure", { error in
                if case .commandOutcomeUnknown(let id, _) = error { return id == request.operationId }
                return false
            }) { _ = try await connection.execute(request) }
            check(!connection.isOpen, "\(mode): the connection is closed before the caller hears of it")
            let committed = try await historyCount(database)
            check(committed == 1, "\(mode): the command did commit; the error did not claim a rollback")
            await checks.fails("\(mode): the next request on it", notSentClosed) { _ = try await connection.query(statusQuery) }
            _ = try? await connection.close()

            // Reconciling is a decision to resend the same command, made here and not by the library.
            let fresh = try await open(.database(database))
            _ = try await fresh.reconcile(request)
            let afterReconcile = try await historyCount(database)
            check(afterReconcile == 1, "\(mode): reconciling returned the recorded operation and committed nothing twice")
            try await fresh.close()
        }
    }

    /// The deadline is a point in time, not a gap between polls: a stream that never pauses and
    /// never ends a line is cut off at it, as timed out and not as an oversized frame.
    func deadlineIsAbsolute() async throws {
        print("the deadline is absolute: a stream that never pauses is cut off at it")
        let database = try await newStore("flood")
        let connection = try await open(.database(database), helper: faultyHelper("flood"), adjust: {
            $0.requestTimeout = 1
            $0.maxResponseBytes = 1 << 30
            $0.shutdownBound = 3
        })
        let started = DispatchTime.now()
        await checks.fails("a flood with no newline", { error in
            if case .timedOut(let fate) = error { return fate == .mayHaveBeenProcessed }
            return false
        }) { _ = try await connection.query(statusQuery) }
        let elapsed = Double(DispatchTime.now().uptimeNanoseconds - started.uptimeNanoseconds) / 1_000_000_000
        check(elapsed < 4, "it ended near its one-second deadline (\(String(format: "%.2f", elapsed)) s)")
        check(!connection.isOpen, "and the connection is closed")
        _ = try? await connection.close()
    }

    /// A request queued behind one that fails is refused unsent, never sent on a broken stream.
    func queuedBehindFailure() async throws {
        print("a request queued behind a failed exchange is refused unsent")
        let database = try await newStore("behind-failure")
        let exchanges = LockedBox(0)
        let flying = Event()
        let connection = try await open(.database(database), helper: faultyHelper("swallow"), adjust: { configuration in
            configuration.requestTimeout = 1.5
            let audited = configuration.onBlockingStep
            configuration.onBlockingStep = { step in
                audited?(step)
                if step == "exchange", exchanges.withLock({ $0 += 1; return $0 }) == 3 { flying.signal() }
            }
        })
        let first = Task { try await connection.query(statusQuery) }
        check(await flying.wait(), "the first request is in flight")
        let second = Task { try await connection.query(statusQuery) }
        if case .failure(let error as BridgeError) = await first.result, case .timedOut(let fate) = error {
            check(fate == .mayHaveBeenProcessed, "the first request timed out and may have been processed")
        } else {
            check(false, "the first request should time out")
        }
        if case .failure(let error as BridgeError) = await second.result {
            check(notSentClosed(error), "the second was refused unsent: \(error)")
        } else {
            check(false, "the second request should have been refused")
        }
        _ = try? await connection.close()
    }

    /// Hold the helper still and fill the connection: one in flight, four waiting, the rest busy.
    func boundedAdmission() async throws {
        print("admission is bounded, and cancelled requests free their place at once")
        let database = try await newStore("admission")
        let exchanges = LockedBox(0)
        let flying = Event()
        let connection = try await open(.database(database), adjust: { configuration in
            let audited = configuration.onBlockingStep
            configuration.onBlockingStep = { step in
                audited?(step)
                if step == "exchange", exchanges.withLock({ $0 += 1; return $0 }) == 3 { flying.signal() }
            }
        })
        guard let pid = connection.helperProcessIdentifier else { throw SuiteError(description: "no helper pid") }
        check(await freeze(pid), "the helper is stopped")
        let first = Task { try await connection.query(statusQuery) }
        check(await flying.wait(), "one request is in flight")

        var storm: [Task<View, Error>] = []
        for _ in 0..<50 {
            let task = Task { try await connection.query(statusQuery) }
            task.cancel()
            storm.append(task)
        }
        var unsent = 0
        for task in storm {
            if case .failure(let error as BridgeError) = await task.result, case .cancelled(let fate) = error, fate == .notSent { unsent += 1 }
        }
        check(unsent == 50, "fifty cancelled requests each ended unsent, and none was refused as busy (\(unsent))")

        let busy = LockedBox(0)
        let threeBusy = Event()
        let extra = (0..<7).map { _ in
            Task { () -> Bool in
                do {
                    _ = try await connection.query(statusQuery)
                    return true
                } catch BridgeError.busy {
                    if busy.withLock({ $0 += 1; return $0 }) == 3 { threeBusy.signal() }
                    return false
                } catch {
                    return false
                }
            }
        }
        check(await threeBusy.wait(), "with one in flight and four waiting, the three beyond them were refused busy while the helper was stopped")
        thaw(pid)
        var answered = 0
        for task in extra where await task.value { answered += 1 }
        check(answered == 4 && busy.withLock { $0 } == 3, "the four admitted were answered in order, once the helper ran: \(answered) answered")
        if case .success = await first.result { check(true, "the request in flight was answered") } else { check(false, "the request in flight should have been answered") }
        _ = try await connection.query(statusQuery)
        try await connection.close()
    }

    /// A command queued behind a request in flight is not moved to the replacement helper.
    func queuedCommandStaysBehind() async throws {
        print("a queued command is refused when its helper is replaced, never sent to the new one")
        let database = try await newStore("generation")
        let work = try await cliJSON(database, ["show", "TEST-A"])["data"]["id"].string ?? ""
        let exchanges = LockedBox(0)
        let admissions = LockedBox(0)
        let flying = Event()
        let queued = Event()
        let connection = try await open(.database(database), adjust: { configuration in
            configuration.shutdownBound = 0.5
            let audited = configuration.onBlockingStep
            configuration.onBlockingStep = { step in
                audited?(step)
                if step == "exchange", exchanges.withLock({ $0 += 1; return $0 }) == 3 { flying.signal() }
                // The first admission is the request in flight; the second is the command behind it.
                if step == "admit", admissions.withLock({ $0 += 1; return $0 }) == 2 { queued.signal() }
            }
        })
        guard let pid = connection.helperProcessIdentifier else { throw SuiteError(description: "no helper pid") }
        check(await freeze(pid), "the helper is stopped")
        let first = Task { try await connection.query(statusQuery) }
        check(await flying.wait(), "a request is in flight")
        let command = Task { try await connection.execute(claimWork(work)) }
        check(await queued.wait(), "the command was admitted behind it")
        let identity = connection.identity
        let reattached = try await connection.reconnect()
        check(reattached == identity, "the replacement reaches the same source")
        if case .failure(let error as BridgeError) = await first.result {
            check(error.fate == .mayHaveBeenProcessed, "the request in flight is told the connection closed: \(error)")
        } else {
            check(false, "the request in flight should have been ended")
        }
        if case .failure(let error as BridgeError) = await command.result {
            check(notSentClosed(error), "the queued command was refused unsent: \(error)")
        } else {
            check(false, "the queued command must not have run on the replacement")
        }
        let committed = try await historyCount(database)
        check(committed == 0, "no operation was written: the command never reached any helper")
        _ = try await connection.query(statusQuery)
        try await connection.close()
    }
}
