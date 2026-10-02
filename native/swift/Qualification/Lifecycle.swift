// Cancellation, close, early exit, crash, reconnect, escalation and window cleanup, each against a
// real helper process and driven by events: the helper is stopped with SIGSTOP and its stop is
// awaited as an event, so a request is truly in flight and unanswered while it is cancelled.

import DPMNative
import Foundation

#if canImport(Darwin)
import Darwin
#endif

/// A one-shot event a test can await without blocking a thread.
final class Event: @unchecked Sendable {
    private let lock = NSLock()
    private var fired = false
    private var waiters: [CheckedContinuation<Bool, Never>] = []

    var isSignalled: Bool {
        lock.lock()
        defer { lock.unlock() }
        return fired
    }

    func signal() {
        lock.lock()
        fired = true
        let pending = waiters
        waiters = []
        lock.unlock()
        for waiter in pending { waiter.resume(returning: true) }
    }

    /// True once signalled; false if `timeout` seconds pass first.
    func wait(timeout: TimeInterval = 15) async -> Bool {
        await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            lock.lock()
            if fired {
                lock.unlock()
                continuation.resume(returning: true)
                return
            }
            waiters.append(continuation)
            lock.unlock()
            DispatchQueue.global().asyncAfter(deadline: .now() + timeout) { [self] in
                lock.lock()
                guard let index = waiters.firstIndex(where: { _ in true }), !fired else { lock.unlock(); return }
                _ = index
                let pending = waiters
                waiters = []
                lock.unlock()
                for waiter in pending { waiter.resume(returning: false) }
            }
        }
    }
}

/// Stop a process and wait for the kernel to say it stopped.
func freeze(_ pid: Int32) async -> Bool {
    await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
        DispatchQueue.global().async {
            kill(pid, SIGSTOP)
            var status: Int32 = 0
            let result = waitpid(pid, &status, WUNTRACED)
            continuation.resume(returning: result == pid && (status & 0x7F) == 0x7F)
        }
    }
}

/// Resume a stopped process.
func thaw(_ pid: Int32) { kill(pid, SIGCONT) }

/// An event for the exit of any process, even one this process did not start.
func exitEvent(of pid: Int32) -> Event {
    let event = Event()
    let source = DispatchSource.makeProcessSource(identifier: pid, eventMask: .exit, queue: .global())
    source.setEventHandler {
        event.signal()
        source.cancel()
    }
    source.resume()
    exitSources.withLock { $0.append(source) }
    if kill(pid, 0) != 0 { event.signal() }
    return event
}

private let exitSources = LockedBox<[DispatchSourceProcess]>([])

final class LockedBox<Value>: @unchecked Sendable {
    private let lock = NSLock()
    private var value: Value
    init(_ value: Value) { self.value = value }
    func withLock<T>(_ work: (inout Value) -> T) -> T {
        lock.lock()
        defer { lock.unlock() }
        return work(&value)
    }
}

/// Signal `event` when the first request after the connection opened is on the wire: opening makes
/// two exchanges of its own, a hello and an attach, and the third is the caller's.
func signalAtFirstRequest(_ configuration: inout Configuration, _ event: Event) {
    let exchanges = LockedBox(0)
    let audited = configuration.onBlockingStep
    configuration.onBlockingStep = { step in
        audited?(step)
        if step == "exchange", exchanges.withLock({ $0 += 1; return $0 }) == 3 { event.signal() }
    }
}

extension Context {
    private func claimRequest(_ work: String, base: UInt64) -> CommandRequest {
        CommandRequest(actor: workerActor, baseRevision: base, command: .object(["Claim": .object(["work": .string(work)])]))
    }

    func cancellationAndPreemption() async throws {
        print("cancellation: a sent request is not stopped, an unsent one is never sent, nothing is resent")
        let database = try await newStore("cancel")
        let work = try await cliJSON(database, ["show", "TEST-A"])["data"]["id"].string ?? ""
        let sent = Event()
        let connection = try await open(.database(database), adjust: { signalAtFirstRequest(&$0, sent) })
        guard let pid = connection.helperProcessIdentifier else { throw SuiteError(description: "no helper pid") }
        // The helper is stopped, so whatever is sent now stays unanswered.
        check(await freeze(pid), "the helper is stopped (its stop was awaited as an event)")
        let request = claimRequest(work, base: 0)
        let command = Task { try await connection.execute(request) }
        check(await sent.wait(), "the command is on the wire, unanswered")
        // Queued behind the command, which holds the exchange queue while it waits on the helper.
        let queued = Task { try await connection.query(statusQuery) }
        queued.cancel()
        command.cancel()
        let queuedResult = await queued.result
        let commandResult = await command.result
        if case .failure(let error as BridgeError) = queuedResult, case .cancelled(let fate) = error {
            check(fate == .notSent, "a request cancelled before it left reports that it was never sent")
        } else {
            check(false, "the queued request should be cancelled unsent: \(queuedResult)")
        }
        if case .failure(let error as BridgeError) = commandResult, case .commandOutcomeUnknown(let id, _) = error {
            check(id == request.operationId, "a command cancelled after it left reports an unknown outcome, naming its operation")
        } else {
            check(false, "the sent command should report an unknown outcome: \(commandResult)")
        }

        // The helper runs again: it processes the command that was already sent, and only that.
        thaw(pid)
        let view = try await connection.query(statusQuery)
        check(view.basis.project?.revision == 1, "the cancelled command was processed by the helper and committed")
        check(try await historyCount(database) == 1, "and exactly once: nothing was resent")
        let reconciled = try await connection.reconcile(request)
        check(reconciled.envelope.data["id"].string == request.operationId || reconciled.envelope.revision == 1, "an explicit reconcile returns the recorded operation")
        check(try await historyCount(database) == 1, "reconciling did not commit a second operation")
        try await connection.close()

        try await preemption()
    }

    private func preemption() async throws {
        print("close preempts an exchange blocked on the helper")
        let database = try await newStore("preempt")
        let sent = Event()
        let connection = try await open(.database(database), adjust: {
            $0.shutdownBound = 0.5
            signalAtFirstRequest(&$0, sent)
        })
        guard let pid = connection.helperProcessIdentifier else { return }
        check(await freeze(pid), "the helper is stopped")
        let blocked = Task { try await connection.query(statusQuery) }
        check(await sent.wait(), "the request is on the wire, unanswered")
        let outcome = try await connection.close()
        switch outcome {
        case .terminated(let exit)?, .killed(let exit)?:
            check(exit.reason == .uncaughtSignal, "a stopped helper cannot exit by itself on its closed input: the close had to end it by signal")
        default:
            check(false, "closing a stopped helper should have ended it by signal, got \(String(describing: outcome))")
        }
        let result = await blocked.result
        if case .failure(let error as BridgeError) = result, case .connectionClosed(let fate) = error {
            check(fate == .mayHaveBeenProcessed, "the blocked exchange was told the connection closed, without waiting for the process")
        } else {
            check(false, "the blocked exchange should report a closed connection: \(result)")
        }
        check(!connection.isOpen, "the connection is closed")
        await checks.fails("a request on a closed connection", { error in
            if case .connectionClosed(let fate) = error { return fate == .notSent }
            return false
        }) { _ = try await connection.query(statusQuery) }
    }

    func crashExitAndReconnect() async throws {
        print("crash, early exit and reconnect")
        let database = try await newStore("crash")
        let connection = try await open(.database(database))
        let identity = connection.identity
        guard let pid = connection.helperProcessIdentifier else { return }
        let consumer = Consumer(seed: try await connection.attach().watermark)
        kill(pid, SIGKILL)
        _ = await exitEvent(of: pid).wait()
        await checks.fails("a request after the helper was killed", { error in
            if case .helperExited(let exit, _) = error { return exit.reason == .uncaughtSignal && exit.status == SIGKILL }
            return false
        }) { _ = try await connection.query(statusQuery) }
        check(!connection.isOpen, "the connection is closed after a crash")
        // Another process changes the project while there is no helper.
        try await worker(database, ["claim", "TEST-A"])
        let reattached = try await connection.reconnect()
        check(reattached == identity, "a reconnect reaches the same source")
        try await consumer.poll(connection)
        check(consumer.project == [1] && consumer.resets.isEmpty, "the client's own cursors continue through the new helper: the change is delivered once")
        try await connection.close()

        let early = try faultyHelper("exit")
        let doomed = try await open(.database(database), helper: early)
        await checks.fails("a helper that exits with a message in the middle of a request", { error in
            if case .helperExited(let exit, let fate) = error {
                return exit.status == 7 && exit.reason == .exited && exit.stderrTail.contains("boom") && fate == .mayHaveBeenProcessed
            }
            return false
        }) { _ = try await doomed.query(statusQuery) }

        let sticky = try faultyHelper("ignore-term")
        let stubborn = try await open(.database(database), helper: sticky, adjust: { $0.shutdownBound = 0.5 })
        let outcome = try await stubborn.close()
        if case .killed? = outcome {
            check(true, "a helper that ignores its closed input and SIGTERM is killed, and the outcome says so")
        } else {
            check(false, "a stubborn helper should be killed, got \(String(describing: outcome))")
        }
    }

    func windowCleanup() async throws {
        print("a window going away leaves ownership alone and does not leave a helper behind")
        let database = try await newStore("window")
        let work = try await cliJSON(database, ["show", "TEST-A"])["data"]["id"].string ?? ""
        let pid = try await leaveWindowOpen(database, work: work)
        check(await exitEvent(of: pid).wait(), "the helper exited after its connection was released without a close")
        let owner = try await cliJSON(database, ["show", "TEST-A"])["data"]["execution"]
        check(owner["owner"]["name"].string == "worker" && owner["status"].string == "Claimed", "the agent still owns its claim")
        check(try await historyCount(database) == 1, "no release, handoff or verification was made for it")
        let reopened = try await open(.database(database))
        let show = try await reopened.query(query("show", ["key": .string("TEST-A")]))
        check(show.envelope.data["execution"]["owner"]["name"].string == "worker", "a new window sees the same owner")
        try await reopened.close()
    }

    private func leaveWindowOpen(_ database: URL, work: String) async throws -> Int32 {
        let connection = try await open(.database(database))
        _ = try await connection.execute(claimRequest(work, base: 0))
        return connection.helperProcessIdentifier ?? 0
    }
}
