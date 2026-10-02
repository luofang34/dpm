// One helper child process: its pipes, its exit as an event, and its bounded, escalating shutdown.
//
// The exit is an event registered before the process launches. Waiting for it is a bounded wait on
// that event, so it does not matter whether the child exits before, during or after the wait, and
// `Process.waitUntilExit`, which can miss an exit that has already happened, is never used.
//
// Descriptors are closed only when no read or write is using them. Every exchange brackets its I/O
// with `beginIO`/`endIO`; a shutdown stops new I/O, aborts what is running, waits for it to leave,
// and only then closes the input. A closed descriptor number can be reused by anything else in the
// embedding process, so using one after closing would be a race this ordering makes impossible.
// A shutdown escalates close, then SIGTERM, then SIGKILL, each with its own bound, and reports
// which step ended the child. A child still running after SIGKILL is an error, never a success.

import Foundation

#if canImport(Darwin)
import Darwin
#endif

final class HelperProcess: @unchecked Sendable {
    private let process = Process()
    private let standardInput = Pipe()
    private let standardOutput = Pipe()
    private let standardError = Pipe()
    private let exited = DispatchGroup()
    private let errorDrained = DispatchGroup()
    private let lock = NSLock()
    private var recorded: (status: Int32, reason: HelperExit.Reason)?
    private var tail = Data()
    private var inputClosed = false
    // I/O bracketing: how many reads/writes are using the descriptors, and whether more may start.
    private var ioActive = 0
    private var ioStopped = false
    private let ioIdle = DispatchGroup()
    let abort: AbortSignal
    /// Set once the child has launched and its pipes are configured, before `init` returns.
    private(set) var reader: FrameReader!
    private(set) var writeDescriptor: Int32 = -1
    private(set) var pid: Int32 = 0

    /// The most of the helper's diagnostic stream kept for reporting why it exited.
    private static let tailBytes = 16 * 1024

    /// Launch the helper. Blocking, so it runs where the caller puts it, which is never the main thread.
    init(executable: URL, arguments: [String], workingDirectory: URL?, maxFrameBytes: Int, shutdownBound: TimeInterval) throws {
        abort = try AbortSignal()
        process.executableURL = executable
        process.arguments = arguments
        if let directory = workingDirectory { process.currentDirectoryURL = directory }
        process.standardInput = standardInput
        process.standardOutput = standardOutput
        process.standardError = standardError
        exited.enter()
        errorDrained.enter()
        let group = exited
        let drained = errorDrained
        process.terminationHandler = { [weak self] finished in
            let reason: HelperExit.Reason = finished.terminationReason == .exit ? .exited : .uncaughtSignal
            self?.record(status: finished.terminationStatus, reason: reason)
            group.leave()
        }
        standardError.fileHandleForReading.readabilityHandler = { [weak self] handle in
            let data = handle.availableData
            if data.isEmpty {
                handle.readabilityHandler = nil
                drained.leave()
            } else {
                self?.keep(data)
            }
        }
        do {
            try process.run()
        } catch {
            group.leave()
            standardError.fileHandleForReading.readabilityHandler = nil
            drained.leave()
            throw BridgeError.launchFailed("\(executable.path): \(error)")
        }
        // The child owns its ends. Keeping ours open would stop end of input from ever arriving.
        try? standardInput.fileHandleForReading.close()
        try? standardOutput.fileHandleForWriting.close()
        try? standardError.fileHandleForWriting.close()
        writeDescriptor = standardInput.fileHandleForWriting.fileDescriptor
        let readDescriptor = standardOutput.fileHandleForReading.fileDescriptor
        pid = process.processIdentifier
        do {
            // Nonblocking descriptors are what let a deadline or an abort interrupt a read or a write,
            // and a write to a pipe whose reader has gone must fail on this descriptor with an error
            // rather than signal the embedding process, whose signal handling is not ours to change.
            try Self.configure(writeDescriptor, suppressingSigPipe: true)
            try Self.configure(readDescriptor, suppressingSigPipe: false)
        } catch let setup {
            // The child is already running and nobody will hold this object, so it is ended here,
            // within bounds and by the same escalation as any shutdown, and how it ended is part of
            // the error. A child that cannot be ended is reported, never silently left behind.
            let ending: String
            do {
                ending = "it was ended: \(try shutdown(force: true, bound: shutdownBound))"
            } catch let failure {
                ending = "it could not be ended: \(failure)"
            }
            throw BridgeError.launchFailed("helper setup failed (\(setup)); \(ending)")
        }
        reader = FrameReader(descriptor: readDescriptor, abort: abort, maxBytes: maxFrameBytes)
    }

    private static func configure(_ descriptor: Int32, suppressingSigPipe: Bool) throws {
        let flags = fcntl(descriptor, F_GETFL)
        guard flags >= 0, fcntl(descriptor, F_SETFL, flags | O_NONBLOCK) == 0 else {
            throw BridgeError.launchFailed("could not make a helper pipe nonblocking: errno \(errno)")
        }
        #if canImport(Darwin)
        if suppressingSigPipe, fcntl(descriptor, F_SETNOSIGPIPE, 1) != 0 {
            throw BridgeError.launchFailed("could not suppress SIGPIPE on the helper's input pipe: errno \(errno)")
        }
        #endif
    }

    private func record(status: Int32, reason: HelperExit.Reason) {
        lock.lock()
        recorded = (status, reason)
        lock.unlock()
    }

    private func keep(_ data: Data) {
        lock.lock()
        tail.append(data)
        if tail.count > Self.tailBytes { tail.removeFirst(tail.count - Self.tailBytes) }
        lock.unlock()
    }

    /// Whether the exit event has fired.
    var hasExited: Bool { exited.wait(timeout: .now()) == .success }

    /// Wait at most `timeout` seconds for the exit event.
    func waitForExit(timeout: TimeInterval) -> Bool {
        exited.wait(timeout: .now() + timeout) == .success
    }

    /// How the child ended, once it has. What it wrote to its diagnostic stream before exiting is
    /// given a moment to arrive, since the pipe drains independently of the exit event.
    var exitInfo: HelperExit? {
        guard hasExited else { return nil }
        _ = errorDrained.wait(timeout: .now() + 1)
        lock.lock()
        defer { lock.unlock() }
        let text = String(decoding: tail, as: UTF8.self).trimmingCharacters(in: .whitespacesAndNewlines)
        guard let recorded = recorded else { return nil }
        return HelperExit(status: recorded.status, reason: recorded.reason, stderrTail: text)
    }

    // MARK: I/O bracketing

    /// Permission to read or write. False once a shutdown has begun.
    func beginIO() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        if ioStopped { return false }
        ioActive += 1
        if ioActive == 1 { ioIdle.enter() }
        return true
    }

    func endIO() {
        lock.lock()
        ioActive -= 1
        let idle = ioActive == 0
        lock.unlock()
        if idle { ioIdle.leave() }
    }

    /// Stop new I/O, wake what is running, and wait for it to leave. False if it did not leave in
    /// time, in which case no descriptor may be closed.
    private func quiesce(bound: TimeInterval) -> Bool {
        lock.lock()
        ioStopped = true
        lock.unlock()
        abort.raise()
        return ioIdle.wait(timeout: .now() + bound) == .success
    }

    /// Stop new I/O and wake whatever is blocked reading or writing this helper.
    func abortExchanges() {
        lock.lock()
        ioStopped = true
        lock.unlock()
        abort.raise()
    }

    private func closeInput() {
        lock.lock()
        let first = !inputClosed
        inputClosed = true
        lock.unlock()
        if first { try? standardInput.fileHandleForWriting.close() }
    }

    private func send(_ number: Int32) { if !hasExited { kill(pid, number) } }

    /// Let go of the helper without waiting, for a connection released without a close. I/O is
    /// stopped; the input is closed once nothing is using it, on a background queue that also makes
    /// sure the helper does not outlive the bounds.
    func abandon(bound: TimeInterval) {
        abortExchanges()
        DispatchQueue.global(qos: .utility).async {
            if self.quiesce(bound: bound) { self.closeInput() }
            if self.waitForExit(timeout: bound) { return }
            self.send(SIGTERM)
            if self.waitForExit(timeout: bound) { return }
            self.send(SIGKILL)
        }
    }

    /// End the helper within bounds and say how. `force` skips the polite steps, as a crash would.
    /// The input is closed only after the I/O has quiesced; if it does not, this throws and the
    /// caller keeps the helper to try again.
    func shutdown(force: Bool, bound: TimeInterval) throws -> ShutdownOutcome {
        guard quiesce(bound: bound) else {
            throw BridgeError.shutdownFailed("pid \(pid): a read or write did not stop within \(bound) seconds of the abort")
        }
        if let exit = exitInfo {
            closeInput()
            return .alreadyExited(exit)
        }
        if !force {
            closeInput()
            if waitForExit(timeout: bound), let exit = exitInfo { return .exitedOnClose(exit) }
            send(SIGTERM)
            if waitForExit(timeout: bound), let exit = exitInfo { return .terminated(exit) }
        }
        send(SIGKILL)
        if waitForExit(timeout: bound), let exit = exitInfo {
            closeInput()
            return .killed(exit)
        }
        throw BridgeError.shutdownFailed("pid \(pid) is still running \(bound) seconds after SIGKILL")
    }
}
