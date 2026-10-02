// One request on its way through a connection, and the single place that decides what became of it.
//
// The decision whether a request was sent and the decision whether it was cancelled are made under
// the same lock, so they cannot disagree: a cancellation either wins before the request leaves,
// and then it is never sent, or loses, and then it was sent and the caller is told it may have
// been processed. A request answered to its caller is answered exactly once; whatever arrives
// later, such as the discarded answer of a cancelled request, is ignored.

import Foundation

final class Exchange: @unchecked Sendable {
    let id: String
    let call: Call
    let protocolVersion: Int
    let deadline: DispatchTime
    private weak var owner: NativeConnection?
    private let lock = NSLock()

    private enum Stage {
        case queued
        case sending
        case resolved
    }

    private var stage = Stage.queued
    private var sent = false
    private var pinned = 0
    private var continuation: CheckedContinuation<Response, Error>?
    private var early: Result<Response, BridgeError>?

    init(id: String, call: Call, protocolVersion: Int, deadline: DispatchTime, owner: NativeConnection?) {
        self.id = id
        self.call = call
        self.protocolVersion = protocolVersion
        self.deadline = deadline
        self.owner = owner
    }

    /// The generation of the helper this request was admitted to. It runs there or not at all.
    var generation: Int { lock.lock(); defer { lock.unlock() }; return pinned }
    func pin(generation: Int) { lock.lock(); pinned = generation; lock.unlock() }

    var fate: RequestFate { lock.lock(); defer { lock.unlock() }; return sent ? .mayHaveBeenProcessed : .notSent }
    var isResolved: Bool { lock.lock(); defer { lock.unlock() }; return stage == .resolved }

    /// Wait for the result. If it already arrived, because the caller was cancelled before waiting
    /// began, it is delivered at once.
    func attach(_ continuation: CheckedContinuation<Response, Error>) {
        lock.lock()
        if let result = early {
            lock.unlock()
            continuation.resume(with: result.mapError { $0 as Error })
        } else {
            self.continuation = continuation
            lock.unlock()
        }
    }

    /// Claim the right to send. False when the request was already answered, which means it was
    /// cancelled before it left and must never be sent.
    func beginSend() -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard stage == .queued else { return false }
        stage = .sending
        sent = true
        return true
    }

    /// Nothing was written after all, so the request cannot have been processed.
    func unmarkSent() {
        lock.lock()
        sent = false
        lock.unlock()
    }

    /// Answer the caller once. A later result is ignored.
    func finish(_ result: Result<Response, BridgeError>) {
        lock.lock()
        guard stage != .resolved else {
            lock.unlock()
            return
        }
        stage = .resolved
        let waiting = continuation
        continuation = nil
        if waiting == nil { early = result }
        lock.unlock()
        waiting?.resume(with: result.mapError { $0 as Error })
    }

    /// The caller was cancelled. Decided and recorded under one lock together with the send
    /// decision, then the caller is answered; a request that never left also leaves the queue at
    /// once, so a storm of cancellations cannot pile up closures behind a blocked exchange.
    func abandon() {
        lock.lock()
        guard stage != .resolved else {
            lock.unlock()
            return
        }
        let wasQueued = stage == .queued
        stage = .resolved
        let result: Result<Response, BridgeError> = .failure(.cancelled(fate: sent ? .mayHaveBeenProcessed : .notSent))
        let waiting = continuation
        continuation = nil
        if waiting == nil { early = result }
        lock.unlock()
        waiting?.resume(with: result.mapError { $0 as Error })
        if wasQueued { owner?.dropQueued(self) }
    }
}
