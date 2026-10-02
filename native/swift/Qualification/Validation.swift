// What a connection refuses before it trusts anything: configurations that could not run, requests
// that do not fit, and answers that contradict what was negotiated.

import DPMNative
import Foundation

extension Context {
    func configurationAndSizes() async throws {
        print("configuration is validated, and oversized requests are refused unsent")
        let database = try await newStore("validation")
        let invalid: [(String, (inout Configuration) -> Void)] = [
            ("a NaN request timeout", { $0.requestTimeout = .nan }),
            ("an infinite request timeout", { $0.requestTimeout = .infinity }),
            ("a zero request timeout", { $0.requestTimeout = 0 }),
            ("a negative request timeout", { $0.requestTimeout = -1 }),
            ("a NaN shutdown bound", { $0.shutdownBound = .nan }),
            ("a response bound below the minimum", { $0.maxResponseBytes = 10 }),
            ("a zero request bound", { $0.helperMaxRequestBytes = 0 }),
            ("no supported protocol", { $0.supportedProtocols = [] }),
            ("more protocol versions than a configuration may list", { $0.supportedProtocols = Array(1...17) }),
            ("a request bound the opening request cannot fit in", { $0.helperMaxRequestBytes = 2 }),
        ]
        for (name, change) in invalid {
            await checks.fails(name, { error in
                if case .invalidConfiguration = error { return true }
                return false
            }) { _ = try await open(.database(database), adjust: change) }
        }
        let started = try await helpersRunning(on: database)
        check(!started, "no helper was started for any invalid configuration")

        let connection = try await open(.database(database), adjust: { $0.helperMaxRequestBytes = 1024 })
        await checks.fails("a NaN timeout on one request", { error in
            if case .invalidConfiguration = error { return true }
            return false
        }) { _ = try await connection.send(.query(statusQuery, attached: nil), timeout: .nan) }
        check(connection.isOpen, "an invalid timeout refuses the request and leaves the connection open")

        let tooLong = JSON.string(String(repeating: "k", count: 5000))
        // Over the bound by its payload, and over it only once framed: both are refused before sending.
        let nearly = JSON.string(String(repeating: "k", count: 1000))
        for (name, key) in [("a payload over the bound", tooLong), ("a frame over the bound", nearly)] {
            await checks.fails(name, { error in
                if case .requestTooLarge(let limit) = error { return limit == 1024 && error.fate == .notSent }
                return false
            }) { _ = try await connection.query(query("show", ["key": key])) }
        }
        // Measured as it will be written: 400 control characters are 400 characters but 2400 bytes.
        await checks.fails("a payload that is short but escapes long", { error in
            if case .requestTooLarge = error { return error.fate == .notSent }
            return false
        }) { _ = try await connection.query(query("show", ["key": .string(String(repeating: "\u{0}", count: 400))])) }
        await checks.fails("an identity too long to send", { error in
            if case .requestTooLarge = error { return error.fate == .notSent }
            return false
        }) { _ = try await connection.attach(expectWorkspace: String(repeating: "w", count: 5000)) }
        check(connection.isOpen, "refusing an oversized request leaves the connection open")
        let view = try await connection.query(statusQuery)
        check(view.basis.project != nil, "the next request is answered, in step")
        try await connection.close()
    }

    func negotiatedConsistency() async throws {
        print("an answer that contradicts what was negotiated closes the connection")
        let database = try await newStore("consistency")
        for mode in ["wrong-api", "wrong-protocol", "ok-false", "ok-true-error", "error-api", "wrong-kind"] {
            let connection = try await open(.database(database), helper: faultyHelper(mode))
            await checks.fails("\(mode): an answer that contradicts the negotiation or the call", { error in
                if case .invalidFrame(_, let fate) = error { return fate == .mayHaveBeenProcessed }
                return false
            }) { _ = try await connection.query(statusQuery) }
            check(!connection.isOpen, "\(mode): the connection is closed")
            await checks.fails("\(mode): the next request", { error in
                if case .connectionClosed(let fate) = error { return fate == .notSent }
                return false
            }) { _ = try await connection.query(statusQuery) }
            _ = try? await connection.close()
        }
    }

    func mainThreadStaysResponsive() async throws {
        print("the main thread keeps running while the bridge waits on a stopped helper")
        let database = try await newStore("responsive")
        let connection = try await open(.database(database), adjust: { $0.shutdownBound = 0.5 })
        guard let pid = connection.helperProcessIdentifier else { throw SuiteError(description: "no helper pid") }
        check(await freeze(pid), "the helper is stopped")
        let blocked = Task { try await connection.query(statusQuery) }
        monitor.reset()
        // A window of main-queue time passes while the exchange waits; the monitor ticks every 2 ms.
        try await Task.sleep(nanoseconds: 400_000_000)
        let window = monitor.snapshot
        check(window.ticks >= 20, "the main queue kept running while a request was blocked (\(window.ticks) ticks)")
        check(window.maxGapMilliseconds < 250, "no main-queue gap longer than 250 ms (longest \(Int(window.maxGapMilliseconds)) ms)")
        _ = try await connection.close()
        _ = await blocked.result
        try await auditDetectsTheMainThread()
    }

    /// The audit that no blocking step ran on the main thread must be able to fail: a step recorded
    /// from the main queue is seen, and one recorded elsewhere is not.
    private func auditDetectsTheMainThread() async throws {
        let control = BlockingAudit()
        await withCheckedContinuation { (continuation: CheckedContinuation<Void, Never>) in
            DispatchQueue.main.async {
                control.record("exchange")
                continuation.resume()
            }
        }
        control.record("launch")
        check(control.mainThreadSteps == ["exchange"], "a step taken on the main thread is detected, and one taken elsewhere is not: \(control.mainThreadSteps)")
    }

    func repeatedLifecycles() async throws {
        print("repeated open, close and kill cycles each end their helper")
        let database = try await newStore("cycles")
        var ended = 0
        for round in 0..<6 {
            let connection = try await open(.database(database))
            guard let pid = connection.helperProcessIdentifier else { continue }
            let gone = exitEvent(of: pid)
            _ = try await connection.attach()
            if round % 2 == 0 { try await connection.close() } else { try await connection.kill() }
            if await gone.wait() { ended += 1 }
            let again = try await connection.close()
            check(again == nil || round % 2 == 1, "ending an ended connection again does not wait or fail")
        }
        check(ended == 6, "all six cycles saw their helper exit (\(ended))")
        let left = try await helpersRunning(on: database)
        check(!left, "none is left running")
    }
}
