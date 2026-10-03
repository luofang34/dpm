// What the observer scenarios share: a recorder that turns the engine's publications into events a
// test can wait for, without sleeping and retrying, and helpers that drive the real CLI as another
// process would. A wait ends when a published snapshot satisfies it; its time limit is only the
// bound that turns a missed event into a failure.

import Combine
import DPMNative
import DPMObserverCore
import Foundation

final class Recorder: @unchecked Sendable {
    private struct Waiter {
        let predicate: (ObserverSnapshot) -> Bool
        let continuation: CheckedContinuation<ObserverSnapshot?, Never>
    }

    private let lock = NSLock()
    private var waiters: [Int: Waiter] = [:]
    private var counter = 0
    private(set) var latest = ObserverSnapshot()
    /// Every snapshot published, newest last, bounded.
    private(set) var history: [ObserverSnapshot] = []

    func publish(_ snapshot: ObserverSnapshot) {
        lock.lock()
        latest = snapshot
        history.append(snapshot)
        if history.count > 4000 { history.removeFirst(history.count - 4000) }
        let ready = waiters.filter { $0.value.predicate(snapshot) }
        for id in ready.keys { waiters[id] = nil }
        lock.unlock()
        for waiter in ready.values { waiter.continuation.resume(returning: snapshot) }
    }

    /// The first snapshot, now or later, that satisfies `predicate`; nil if none does within `seconds`.
    func wait(_ predicate: @escaping (ObserverSnapshot) -> Bool) async -> ObserverSnapshot? {
        await wait(20, predicate)
    }

    func wait(_ seconds: TimeInterval, _ predicate: @escaping (ObserverSnapshot) -> Bool) async -> ObserverSnapshot? {
        await withCheckedContinuation { (continuation: CheckedContinuation<ObserverSnapshot?, Never>) in
            lock.lock()
            if predicate(latest) {
                let found = latest
                lock.unlock()
                continuation.resume(returning: found)
                return
            }
            counter += 1
            let id = counter
            waiters[id] = Waiter(predicate: predicate, continuation: continuation)
            lock.unlock()
            DispatchQueue.global().asyncAfter(deadline: .now() + seconds) { [self] in
                lock.lock()
                let expired = waiters.removeValue(forKey: id)
                lock.unlock()
                expired?.continuation.resume(returning: nil)
            }
        }
    }

    /// Whether any snapshot published so far satisfies `predicate`.
    func saw(_ predicate: (ObserverSnapshot) -> Bool) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        return history.contains(where: predicate)
    }

    var snapshots: [ObserverSnapshot] {
        lock.lock()
        defer { lock.unlock() }
        return history
    }
}

final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0
    func next() -> Int {
        lock.lock()
        defer { lock.unlock() }
        value += 1
        return value
    }
}

/// Run a process to its end on the calling thread. Used from inside a blocking step, where an
/// async call cannot be awaited, to make another process write at an exact point of a startup.
func runSynchronously(_ executable: String, _ arguments: [String]) {
    let process = Process()
    process.executableURL = URL(fileURLWithPath: executable)
    process.arguments = arguments
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try? process.run()
    process.waitUntilExit()
}

extension Recorder {
    /// The connected snapshot, or a failure at once if the observer reports it cannot connect.
    func connected(_ what: String) async throws -> ObserverSnapshot {
        let shown = try await expect(what) { if $0.connection == .connected { return true }; if case .failed = $0.connection { return true }; return false }
        if case .failed(let message) = shown.connection { throw SuiteError(description: "stopped early: \(what) failed: \(message)") }
        return shown
    }

    /// The first snapshot that satisfies `predicate`, or a failure that says what was awaited and what
    /// the observer was showing instead. A scenario never goes on past a missing event.
    func expect(_ what: String, _ seconds: TimeInterval = 20, _ predicate: @escaping (ObserverSnapshot) -> Bool) async throws -> ObserverSnapshot {
        if let found = await wait(seconds, predicate) { return found }
        let shown = latest
        throw SuiteError(description: "timed out after \(Int(seconds)) s waiting for \(what); the observer showed: connection \(shown.connection), revision \(shown.revision.map { String($0) } ?? "none"), detail \(shown.detail?.title ?? "none") (loading \(shown.detail?.loading ?? false)), error \(shown.freshness.lastError ?? "none"), \(snapshots.count) snapshots published")
    }
}

extension Context {
    /// Open a workspace in a fresh engine, require it to connect, run `body`, and always close the
    /// engine, whether the body finished or failed, so no helper outlives its scenario.
    func withObserver(_ selection: WorkspaceSelection, clock: Instant? = nil, production: Bool = false, adjust: (inout ObserverEngine.Settings) -> Void = { _ in },
                      _ body: (ObserverEngine, Recorder, ObserverSnapshot) async throws -> Void) async throws {
        let (engine, recorder) = observer(clock: clock, production: production, adjust: adjust)
        await engine.open(selection)
        do {
            let first = try await recorder.connected("the observer to connect to \(selection)")
            try await body(engine, recorder, first)
        } catch {
            await engine.close()
            throw error
        }
        await engine.close()
    }

    /// Select a subject and require its detail to be read.
    @discardableResult
    func selectAndRead(_ engine: ObserverEngine, _ recorder: Recorder, _ subject: DPMObserverCore.Subject, _ what: String) async throws -> ObserverSnapshot {
        await engine.select(subject)
        return try await recorder.expect("\(what) to be read") { $0.detail?.subject == subject && $0.detail?.loading == false }
    }

    /// An engine with every interval shortened for a test, and the recorder that hears it.
    func observer(clock: Instant? = nil, production: Bool = false, adjust: (inout ObserverEngine.Settings) -> Void = { _ in }) -> (ObserverEngine, Recorder) {
        let recorder = Recorder()
        var settings = ObserverEngine.Settings(helper: helperURL)
        settings.pinnedClock = clock
        if !production {
            settings.pollInterval = 0.05
            settings.reconnectDelays = [0.05, 0.1]
            settings.runsRefreshInterval = 0.2
        }
        settings.shutdownBound = 3
        let audit = self.audit
        settings.onBlockingStep = { audit.record($0) }
        adjust(&settings)
        return (ObserverEngine(settings: settings) { recorder.publish($0) }, recorder)
    }

    /// Open `selection` and wait until the engine says it is connected.
    func openObserver(_ engine: ObserverEngine, _ recorder: Recorder, _ selection: WorkspaceSelection) async -> ObserverSnapshot? {
        await engine.open(selection)
        return await recorder.wait { $0.connection == .connected }
    }

    func managedRun(_ database: URL, work: String = "TEST-A", id: String = UUIDv7.make()) async throws -> String {
        _ = try await cliJSON(database, ["run", "start", work, "--actor", "service:dpm-claude", "--run-id", id, "--observation", "managed", "--executor", "agent:worker"])
        return id
    }

    func reportedRun(_ database: URL, work: String = "TEST-A", id: String = UUIDv7.make()) async throws -> String {
        _ = try await cliJSON(database, ["run", "start", work, "--actor", "agent:worker", "--run-id", id])
        return id
    }

    func record(_ database: URL, _ run: String, _ kind: String, _ sequence: Int, _ text: String = "", actor: String = "service:dpm-claude") async throws {
        var words = ["run", "record", run, kind, "--sequence", "\(sequence)", "--actor", actor]
        if !text.isEmpty { words += ["--text", text] }
        _ = try await cliJSON(database, words)
    }

    func currentRevision(_ database: URL) async throws -> UInt64 {
        let answer = try await cliJSON(database, ["revision"])
        return answer["data"]["revision"].uint ?? answer["revision"].uint ?? 0
    }

    func identity(of key: String, in snapshot: ObserverSnapshot) -> String? {
        snapshot.inventory.items.first { $0.key == key }?.identity
    }

    func milliseconds(since start: DispatchTime) -> Double {
        Double(DispatchTime.now().uptimeNanoseconds - start.uptimeNanoseconds) / 1_000_000
    }
}
