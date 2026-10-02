// The persistent local helper as a request/response transport, and the client that keeps every
// blocking step of its life off the main thread.
//
// One request line goes in, one response line comes out. The helper holds no per-client state, so
// a client that loses it simply starts another and continues from its own cursors. Launching the
// process, each exchange, ending it abruptly and closing it all run on the client's private serial
// queue; an audit records any step that touched the main thread, so a proof can assert none did.
// Production packaging, cancellation and supervision belong to UI-30.

import Foundation

enum TransportError: Error, CustomStringConvertible {
    case launch(String)
    case closed
    case encoding(String)
    case mismatch(expected: String, actual: String)
    /// The helper did not exit within the bound, even after being terminated.
    case shutdown(String)

    var description: String {
        switch self {
        case .launch(let reason): return "could not start the helper: \(reason)"
        case .closed: return "the helper closed its output"
        case .encoding(let reason): return "could not encode or decode a line: \(reason)"
        case .mismatch(let expected, let actual): return "response id \(actual) does not answer request \(expected)"
        case .shutdown(let reason): return "the helper did not stop: \(reason)"
        }
    }
}

/// Which lifecycle steps ran, and which of them ran on the main thread.
final class ThreadAudit: @unchecked Sendable {
    private let lock = NSLock()
    private var all: [String] = []
    private var main: [String] = []

    func record(_ step: String) {
        lock.lock()
        all.append(step)
        if Thread.isMainThread { main.append(step) }
        lock.unlock()
    }

    var steps: [String] { lock.lock(); defer { lock.unlock() }; return all }
    var mainThreadSteps: [String] { lock.lock(); defer { lock.unlock() }; return main }
}

/// Every line on the wire, to standard error, when DPM_NATIVE_TRACE is set.
func trace(_ line: String) {
    if ProcessInfo.processInfo.environment["DPM_NATIVE_TRACE"] != nil {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }
}

/// One line out, one line back. Blocking; never call it from the main thread.
protocol LineTransport: AnyObject {
    func exchange(_ line: String) throws -> String
    /// End cleanly and wait, bounded, for the child's exit event; throws `shutdown` past the bound.
    func close() throws
    /// End abruptly and wait, bounded, for the child's exit event; throws `shutdown` past the bound.
    func kill() throws
    /// Whether the child's exit event arrived within `timeout` seconds.
    func waitForExit(timeout: TimeInterval) -> Bool
}

/// The longest a helper may take to exit once it has been asked to, in seconds.
let exitBound: TimeInterval = 10

/// A helper process speaking lines over a pipe.
///
/// The child's exit is an event, not a poll: a termination handler is installed before the process
/// launches and leaves a group, so waiting for the exit is a bounded wait on that event. It does
/// not matter whether the child exits before, during or after the wait, and `waitUntilExit`, which
/// can miss an exit that has already happened, is never used.
final class HelperTransport: LineTransport {
    private let process = Process()
    private let input = Pipe()
    private let output = Pipe()
    private var pending = Data()
    private let lock = NSLock()
    private let audit: ThreadAudit
    private let exited = DispatchGroup()

    /// Starts the process: blocking, so it runs where the caller puts it, which must not be the main thread.
    init(executable: String, arguments: [String], audit: ThreadAudit) throws {
        self.audit = audit
        audit.record("launch")
        process.executableURL = URL(fileURLWithPath: executable)
        process.arguments = arguments
        process.standardInput = input
        process.standardOutput = output
        process.standardError = FileHandle.standardError
        exited.enter()
        let group = exited
        process.terminationHandler = { _ in group.leave() }
        do {
            try process.run()
        } catch {
            group.leave()
            throw TransportError.launch("\(error)")
        }
    }

    func exchange(_ line: String) throws -> String {
        lock.lock()
        defer { lock.unlock() }
        audit.record("exchange")
        guard exited.wait(timeout: .now()) == .timedOut else { throw TransportError.closed }
        trace("-> \(line)")
        input.fileHandleForWriting.write(Data((line + "\n").utf8))
        while true {
            if let newline = pending.firstIndex(of: 0x0A) {
                let text = String(decoding: pending[pending.startIndex..<newline], as: UTF8.self)
                pending.removeSubrange(pending.startIndex...newline)
                trace("<- \(text)")
                return text
            }
            let chunk = output.fileHandleForReading.availableData
            if chunk.isEmpty { throw TransportError.closed }
            pending.append(chunk)
        }
    }

    func waitForExit(timeout: TimeInterval) -> Bool {
        exited.wait(timeout: .now() + timeout) == .success
    }

    func close() throws {
        lock.lock()
        defer { lock.unlock() }
        audit.record("close")
        try? input.fileHandleForWriting.close()
        if waitForExit(timeout: exitBound) { return }
        // A helper that ignores the end of its input is asked to stop, and the wait is bounded again.
        process.terminate()
        guard waitForExit(timeout: exitBound) else { throw TransportError.shutdown("no exit event after close and terminate") }
    }

    /// End the helper abruptly, as a crash or a killed process would. A child that has already
    /// exited needs nothing: its event has fired.
    func kill() throws {
        audit.record("kill")
        if !waitForExit(timeout: 0) { process.terminate() }
        guard waitForExit(timeout: exitBound) else { throw TransportError.shutdown("no exit event after terminate") }
    }
}

/// An asynchronous client over a transport.
///
/// Every step that can block runs on one private serial queue: starting the helper, each exchange,
/// killing it and closing it. The caller's thread is never blocked and two calls never interleave
/// their lines. A refusal is thrown as a typed `NativeError`.
final class NativeClient: @unchecked Sendable {
    private var transport: LineTransport?
    private let queue = DispatchQueue(label: "dpm.native.client", qos: .userInitiated)
    private var counter = 0
    let audit: ThreadAudit

    private init(audit: ThreadAudit) { self.audit = audit }

    /// Start a client whose transport is created on its own queue, off the caller's thread.
    static func launch(audit: ThreadAudit, make: @escaping @Sendable () throws -> LineTransport) async throws -> NativeClient {
        let client = NativeClient(audit: audit)
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            client.queue.async {
                do {
                    client.transport = try make()
                    continuation.resume()
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
        return client
    }

    /// Start the helper process as a persistent local helper.
    static func launchHelper(executable: String, arguments: [String], audit: ThreadAudit) async throws -> NativeClient {
        try await launch(audit: audit) { try HelperTransport(executable: executable, arguments: arguments, audit: audit) }
    }

    /// End the transport on the owner queue: abruptly when `kill`, cleanly otherwise. Both wait,
    /// bounded, for the child's exit event and throw `TransportError.shutdown` past the bound. A
    /// client whose transport is already gone has nothing to end.
    func shutdown(kill: Bool) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            queue.async {
                do {
                    defer { self.transport = nil }
                    if kill { try self.transport?.kill() } else { try self.transport?.close() }
                    continuation.resume()
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Whether the child's exit event arrived within `timeout` seconds; the wait runs on the owner queue.
    func awaitExit(timeout: TimeInterval) async -> Bool {
        await withCheckedContinuation { (continuation: CheckedContinuation<Bool, Never>) in
            queue.async { continuation.resume(returning: self.transport?.waitForExit(timeout: timeout) ?? true) }
        }
    }

    private func exchange(_ line: String) throws -> String {
        guard let transport = transport else { throw TransportError.closed }
        return try transport.exchange(line)
    }

    /// Send a call and return the decoded response, whether it succeeded or was refused.
    func response(_ call: Call, protocolVersion: Int = 1) async throws -> Response {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do {
                    self.counter += 1
                    let id = "c-\(self.counter)"
                    let request = try JSONEncoder().encode(Request(protocolVersion: protocolVersion, id: id, call: call))
                    let line = try self.exchange(String(decoding: request, as: UTF8.self))
                    let response = try JSONDecoder().decode(Response.self, from: Data(line.utf8))
                    guard response.id == id else { throw TransportError.mismatch(expected: id, actual: response.id) }
                    continuation.resume(returning: response)
                } catch {
                    continuation.resume(throwing: error)
                }
            }
        }
    }

    /// Send raw text and return the raw answer; for malformed-request proofs and the test affordance.
    func raw(_ line: String) async throws -> String {
        try await withCheckedThrowingContinuation { continuation in
            queue.async {
                do { continuation.resume(returning: try self.exchange(line)) } catch { continuation.resume(throwing: error) }
            }
        }
    }

    /// The result of a call, or the typed refusal thrown.
    func result(_ call: Call, protocolVersion: Int = 1) async throws -> Result {
        let answer = try await response(call, protocolVersion: protocolVersion)
        if let error = answer.error {
            throw NativeError(code: ErrorCode(error.code), message: error.message, details: error.details)
        }
        guard let result = answer.result else { throw TransportError.encoding("neither a result nor a refusal") }
        return result
    }

    func hello(_ protocols: [Int]) async throws -> Hello {
        guard case .hello(let value) = try await result(.hello(protocols: protocols)) else { throw TransportError.encoding("not a hello") }
        return value
    }

    func attach(expect: String? = nil) async throws -> Attached {
        guard case .attached(let value) = try await result(.attach(expectWorkspace: expect)) else { throw TransportError.encoding("not an attach") }
        return value
    }

    func query(_ query: JSON, attached: Attachment? = nil) async throws -> View {
        guard case .view(let value) = try await result(.query(query, attached: attached)) else { throw TransportError.encoding("not a view") }
        return value
    }

    func changes(_ since: Cursors, limit: Int? = nil, attached: Attachment? = nil) async throws -> Changes {
        guard case .changes(let value) = try await result(.changes(since, limit: limit, attached: attached)) else { throw TransportError.encoding("not changes") }
        return value
    }

    func command(_ request: JSON, attached: Attachment? = nil) async throws -> Committed {
        guard case .committed(let value) = try await result(.command(request, attached: attached)) else { throw TransportError.encoding("not a commit") }
        return value
    }
}
