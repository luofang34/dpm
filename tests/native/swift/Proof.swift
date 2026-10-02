// A compiled Swift client driving the real persistent helper, to prove the request/response/typed
// error contract end to end. It is a proof fixture for UI-10, not the UI-30 bridge.
//
// Usage: proof --helper PATH --database PATH --dpm PATH --clock RFC3339 [--out PATH]
//
// Writes to the project go through the real `dpm` CLI, as another process would make them; every
// read and the one native command go through the helper. The proof prints one line per check and
// exits non-zero if any fails.

import Foundation

// MARK: - Arguments and checks

struct Arguments {
    var helper = ""
    var database = ""
    var dpm = ""
    var clock = ""
    var out: String?

    init(_ values: [String]) {
        var iterator = values.dropFirst().makeIterator()
        while let flag = iterator.next() {
            let value = iterator.next() ?? ""
            switch flag {
            case "--helper": helper = value
            case "--database": database = value
            case "--dpm": dpm = value
            case "--clock": clock = value
            case "--out": out = value
            default: break
            }
        }
    }
}

final class Checks {
    private(set) var failures: [String] = []
    private(set) var passed = 0

    func check(_ condition: Bool, _ message: @autoclosure () -> String) {
        if condition {
            passed += 1
            print("  ok: \(message())")
        } else {
            failures.append(message())
            print("  FAIL: \(message())")
        }
    }

    /// Expect the call to fail as the transport reports a closed helper.
    func refusedTransport(_ message: String, _ call: () async throws -> Void) async {
        do {
            try await call()
            check(false, "\(message): expected the helper to be closed, but it succeeded")
        } catch TransportError.closed {
            check(true, "\(message): reported as closed")
        } catch {
            check(false, "\(message): failed with \(error)")
        }
    }

    /// Expect the call to be refused with exactly this typed code.
    func refused(_ code: ErrorCode, _ message: String, _ call: () async throws -> Void) async {
        do {
            try await call()
            check(false, "\(message): expected \(code), but it succeeded")
        } catch let error as NativeError {
            check(error.code == code, "\(message): refused as \(error.code)")
        } catch {
            check(false, "\(message): refused with a non-typed error \(error)")
        }
    }
}

// MARK: - Main-thread responsiveness

/// Ticks on the main queue, so a blocked main thread shows up as a long gap between ticks.
final class MainThreadMonitor: @unchecked Sendable {
    private let timer = DispatchSource.makeTimerSource(queue: .main)
    private let lock = NSLock()
    private var last = DispatchTime.now()
    private var gap = 0.0
    private var count = 0

    func start() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(2))
        timer.setEventHandler { [self] in
            let now = DispatchTime.now()
            lock.lock()
            gap = max(gap, Double(now.uptimeNanoseconds - last.uptimeNanoseconds) / 1_000_000)
            last = now
            count += 1
            lock.unlock()
        }
        timer.resume()
    }

    func reset() {
        lock.lock()
        last = DispatchTime.now()
        gap = 0
        count = 0
        lock.unlock()
    }

    var snapshot: (ticks: Int, maxGapMilliseconds: Double) {
        lock.lock()
        defer { lock.unlock() }
        return (count, gap)
    }
}

// MARK: - The real CLI, as another process

func command(_ executable: String, _ arguments: [String]) async throws -> JSON {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global().async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            let output = Pipe()
            process.standardOutput = output
            process.standardError = FileHandle.standardError
            // The exit is an event installed before launch, never a wait that can miss it.
            let exited = DispatchGroup()
            exited.enter()
            process.terminationHandler = { _ in exited.leave() }
            do {
                try process.run()
            } catch {
                exited.leave()
                continuation.resume(throwing: error)
                return
            }
            let data = output.fileHandleForReading.readDataToEndOfFile()
            guard exited.wait(timeout: .now() + exitBound) == .success else {
                process.terminate()
                continuation.resume(throwing: TransportError.shutdown("\(arguments.joined(separator: " ")) did not exit"))
                return
            }
            guard process.terminationStatus == 0 else {
                continuation.resume(throwing: TransportError.launch("\(arguments.joined(separator: " ")) exited \(process.terminationStatus): \(String(decoding: data, as: UTF8.self))"))
                return
            }
            do { continuation.resume(returning: try JSONDecoder().decode(JSON.self, from: data)) } catch { continuation.resume(throwing: error) }
        }
    }
}

// MARK: - Fixtures the proof shares

let worker: JSON = .object(["kind": .string("Agent"), "name": .string("worker")])
let runId = "0192f000-0000-7000-8000-0000000000a1"
let wrongWorkspace = Attachment(workspaceId: "00000000-0000-4000-8000-000000000999", lineageId: nil)

func query(_ fields: [String: JSON]) -> JSON { .object(fields) }

let statusQuery = query(["query": .string("status"), "probabilistic": .bool(false)])
let runsQuery = query(["query": .string("runs")])

func explain(_ key: String) -> JSON { query(["query": .string("explain"), "key": .string(key)]) }

func envelopeJSON(_ view: View) -> JSON {
    .object([
        "api_version": .integer(Int64(view.envelope.apiVersion)),
        "revision": view.envelope.revision.map { .unsigned($0) } ?? .null,
        "lineage_id": view.envelope.lineageId.map { .string($0) } ?? .null,
        "data": view.envelope.data,
    ])
}

/// The test affordance of the example helper, not part of the protocol.
func setClock(_ client: NativeClient, _ instant: Instant?) async throws {
    let answer = try await client.raw("!clock \(instant.map { $0.description } ?? "system")")
    if answer != #"{"ok":true}"# { throw TransportError.encoding("unexpected answer to !clock: \(answer)") }
}
