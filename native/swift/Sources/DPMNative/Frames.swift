// Bounded newline framing over file descriptors, with deadlines and an abort that preempts a
// blocked read or write. This is the only place bytes cross the process boundary.
//
// The reader never holds more than one frame of at most its bound plus one read chunk: a longer
// line is reported and the connection is not trusted afterwards. A frame is one line of UTF-8; a
// line that is not valid UTF-8 is reported, never repaired.

import Foundation

#if canImport(Darwin)
import Darwin
#endif

/// What reading the next frame found.
enum FrameRead {
    case line(Data)
    case endOfInput
    case timedOut
    case aborted
    case tooLarge
}

/// What writing a frame did.
enum FrameWrite: Equatable {
    case written
    /// The reader went away; `bytes` of the frame were accepted before that.
    case closed(bytes: Int)
    case timedOut(bytes: Int)
    case aborted(bytes: Int)
}

/// A pipe a blocked read or write polls alongside its own descriptor, so that closing or
/// cancelling a connection wakes it at once instead of queueing behind it.
final class AbortSignal: @unchecked Sendable {
    private let lock = NSLock()
    private var raised = false
    let readEnd: Int32
    private let writeEnd: Int32

    init() throws {
        var fds: [Int32] = [0, 0]
        guard pipe(&fds) == 0 else { throw BridgeError.launchFailed("could not create an abort pipe: errno \(errno)") }
        readEnd = fds[0]
        writeEnd = fds[1]
    }

    deinit {
        close(readEnd)
        close(writeEnd)
    }

    var isRaised: Bool {
        lock.lock()
        defer { lock.unlock() }
        return raised
    }

    /// Wake every poll on this signal, now and from here on.
    func raise() {
        lock.lock()
        let first = !raised
        raised = true
        lock.unlock()
        if first {
            var byte: UInt8 = 1
            _ = write(writeEnd, &byte, 1)
        }
    }
}

/// Whether `deadline` has passed. Checked on every pass of a read or write loop, not only when a
/// poll finds nothing ready: bytes that stay ready cannot keep a request alive past its deadline.
func expired(_ deadline: DispatchTime?) -> Bool {
    guard let deadline = deadline else { return false }
    return DispatchTime.now().uptimeNanoseconds >= deadline.uptimeNanoseconds
}

/// How long until `deadline`, in milliseconds for `poll`, or -1 for none.
private func milliseconds(until deadline: DispatchTime?) -> Int32 {
    guard let deadline = deadline else { return -1 }
    let now = DispatchTime.now().uptimeNanoseconds
    if deadline.uptimeNanoseconds <= now { return 0 }
    let remaining = (deadline.uptimeNanoseconds - now) / 1_000_000 + 1
    return Int32(min(remaining, UInt64(Int32.max)))
}

final class FrameReader {
    private let descriptor: Int32
    private let abort: AbortSignal
    private let maxBytes: Int
    private var buffer = Data()
    /// How much of `buffer` has been searched for a newline.
    private var scanned = 0
    /// The most bytes ever held, so a test can state the bound.
    private(set) var highWater = 0

    init(descriptor: Int32, abort: AbortSignal, maxBytes: Int) {
        self.descriptor = descriptor
        self.abort = abort
        self.maxBytes = maxBytes
    }

    /// The next line, waiting at most until `deadline`. The deadline is absolute: it is checked
    /// before every further read, so a stream that never pauses is cut off at it as well.
    func nextLine(deadline: DispatchTime?) -> FrameRead {
        while true {
            // Only bytes not yet searched are searched, so a long line costs its length once.
            if let newline = buffer[(buffer.startIndex + scanned)...].firstIndex(of: 0x0A) {
                let line = buffer[buffer.startIndex..<newline]
                scanned = 0
                if line.count > maxBytes { return .tooLarge }
                let frame = Data(line)
                buffer.removeSubrange(buffer.startIndex...newline)
                return .line(frame)
            }
            scanned = buffer.count
            if buffer.count > maxBytes { return .tooLarge }
            if abort.isRaised { return .aborted }
            if expired(deadline) { return .timedOut }
            var polls = [
                pollfd(fd: descriptor, events: Int16(POLLIN), revents: 0),
                pollfd(fd: abort.readEnd, events: Int16(POLLIN), revents: 0),
            ]
            let ready = poll(&polls, 2, milliseconds(until: deadline))
            if ready < 0 {
                if errno == EINTR { continue }
                return .endOfInput
            }
            if ready == 0 { return .timedOut }
            if polls[1].revents != 0 { return .aborted }
            var chunk = [UInt8](repeating: 0, count: 65536)
            let count = read(descriptor, &chunk, chunk.count)
            if count < 0 {
                if errno == EINTR || errno == EAGAIN { continue }
                return .endOfInput
            }
            if count == 0 {
                // End of input inside a frame is a truncated frame, never a short one.
                return .endOfInput
            }
            buffer.append(contentsOf: chunk[0..<count])
            highWater = max(highWater, buffer.count)
        }
    }

    /// Bytes read but not yet returned as a frame; non-zero at end of input means a truncated frame.
    var pending: Int { buffer.count }
}

/// Write one whole frame, waiting at most until `deadline`, and never past an abort.
func writeFrame(_ frame: Data, to descriptor: Int32, abort: AbortSignal, deadline: DispatchTime?) -> FrameWrite {
    var written = 0
    let total = frame.count
    while written < total {
        if abort.isRaised { return .aborted(bytes: written) }
        if expired(deadline) { return .timedOut(bytes: written) }
        var polls = [
            pollfd(fd: descriptor, events: Int16(POLLOUT), revents: 0),
            pollfd(fd: abort.readEnd, events: Int16(POLLIN), revents: 0),
        ]
        let ready = poll(&polls, 2, milliseconds(until: deadline))
        if ready < 0 {
            if errno == EINTR { continue }
            return .closed(bytes: written)
        }
        if ready == 0 { return .timedOut(bytes: written) }
        if polls[1].revents != 0 { return .aborted(bytes: written) }
        if polls[0].revents & Int16(POLLERR | POLLHUP | POLLNVAL) != 0 { return .closed(bytes: written) }
        let count = frame.withUnsafeBytes { raw -> Int in
            guard let base = raw.baseAddress else { return 0 }
            return write(descriptor, base.advanced(by: written), total - written)
        }
        if count < 0 {
            if errno == EINTR || errno == EAGAIN { continue }
            return .closed(bytes: written)
        }
        written += count
    }
    return .written
}
