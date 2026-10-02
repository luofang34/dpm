// A connection to one helper process and the workspace it owns.
//
// One lock guards the whole state: which phase the connection is in, which helper it holds, the
// requests admitted but not yet sent, and the one in flight. Two queues do the blocking work. The
// exchange queue performs one request at a time: write the frame, read the one answer, correlate
// it. The control queue launches, ends and replaces the helper. They are separate so that closing,
// killing or reconnecting never waits behind an exchange that is itself waiting on the helper: the
// control path aborts the blocked read, tells the caller of the exchange at once, and ends the
// process within bounds.
//
// Admission is bounded: a few requests may wait for the one in flight, and one more is refused as
// busy and never sent. A request is pinned to the helper it was admitted to and refused if that
// helper is gone; it is never moved to a replacement. A cancelled request that never left is
// removed from the queue at once; one that left is not stopped, and its answer is read and
// discarded so the next exchange stays in step. Nothing here ever resends anything.

import Foundation

public final class NativeConnection: @unchecked Sendable {
    public let configuration: Configuration
    private let lock = NSLock()
    private enum Phase {
        case opening
        case open
        case closed
    }

    private var phase = Phase.opening
    private var generation = 0
    private var helper: HelperProcess?
    /// Helpers that must still be ended. One stays here until it is known to have ended, so a
    /// failed shutdown can be retried and is never forgotten.
    private var retired: [HelperProcess] = []
    private var queue: [Exchange] = []
    private var inFlight: Exchange?
    private var draining = false
    private var counter = 0
    private var attachedIdentity: Identity?
    private var negotiated: Hello?
    private var openCancelled = false
    private let exchangeQueue = DispatchQueue(label: "dpm.native.exchange", qos: .userInitiated)
    private let controlQueue = DispatchQueue(label: "dpm.native.control", qos: .userInitiated)

    /// Requests that may wait for the one in flight.
    static let maxWaiting = 4

    /// How long to wait for the exit event of a helper whose output has ended.
    private static let exitEventBound: TimeInterval = 2

    /// Codes the helper's transport makes for a frame, which answer a request, unlike a startup
    /// refusal, which arrives before any request was read.
    private static let frameCodes: Set<String> = [
        "frame_too_large", "invalid_encoding", "truncated_frame", "response_too_large",
        "invalid_id", "response_encoding_failed",
    ]

    private init(_ configuration: Configuration) { self.configuration = configuration }

    deinit {
        // A connection released without `close` must not leave its helper behind. Nothing is sent
        // and nothing is rolled back: the application's state, and any task a client owns in it,
        // is exactly what the last request left.
        helper?.abandon(bound: configuration.shutdownBound)
        for stale in retired { stale.abandon(bound: configuration.shutdownBound) }
    }

    // MARK: Observing

    public var capabilities: Capabilities? { locked { negotiated?.capabilities } }
    public var protocolVersion: Int? { locked { negotiated?.protocolVersion } }
    /// What this connection is attached to; set by `open` and checked by `reconnect`.
    public var identity: Identity? { locked { attachedIdentity } }
    public var isOpen: Bool { locked { phase == .open } }
    /// The helper's process identifier, for diagnostics and fault injection.
    public var helperProcessIdentifier: Int32? { locked { helper?.pid } }

    private func locked<T>(_ work: () throws -> T) rethrows -> T {
        lock.lock()
        defer { lock.unlock() }
        return try work()
    }

    private func step(_ name: String) { configuration.onBlockingStep?(name) }

    private func nextId() -> String {
        locked {
            counter &+= 1
            return "c-\(counter)"
        }
    }

    private func onControl<T: Sendable>(_ work: @escaping @Sendable () throws -> T) async throws -> T {
        try await withCheckedThrowingContinuation { continuation in
            controlQueue.async { continuation.resume(with: Result { try work() }) }
        }
    }

    // MARK: Opening

    /// Start the helper, negotiate, attach, and publish the connection as open only once all of
    /// that, and the source check, has passed. Every blocking step runs off the caller's thread.
    public static func open(_ configuration: Configuration) async throws -> NativeConnection {
        try configuration.validate()
        let connection = NativeConnection(configuration)
        do {
            try await withTaskCancellationHandler {
                try await connection.onControl { try connection.launchAndAttach(adopt: true) }
            } onCancel: {
                connection.cancelOpening()
            }
        } catch {
            // Whatever step the cancellation interrupted, the caller asked to stop: say so.
            if connection.locked({ connection.openCancelled }) { throw BridgeError.cancelled(fate: .notSent) }
            throw error
        }
        if connection.locked({ connection.openCancelled }) {
            _ = try? await connection.kill()
            throw BridgeError.cancelled(fate: .notSent)
        }
        return connection
    }

    /// Cancellation of an open in progress is remembered, not just acted on: a helper that is
    /// launched after this is ended before it is ever published.
    private func cancelOpening() {
        let launched = locked { () -> HelperProcess? in
            openCancelled = true
            return helper
        }
        // Off the cancelling thread, and not on the control queue, which may be the very thing
        // that is blocked launching: a helper that does not exist yet is stopped at its launch.
        let bound = configuration.shutdownBound
        DispatchQueue.global().async {
            self.step("shutdown")
            if let launched = launched { _ = try? launched.shutdown(force: true, bound: bound) }
        }
    }

    /// Launch, negotiate and attach on the control queue. `adopt` accepts whatever source the new
    /// helper holds; otherwise it must be the one this connection was attached to.
    private func launchAndAttach(adopt: Bool) throws {
        let expected: Identity? = try locked {
            if openCancelled { throw BridgeError.cancelled(fate: .notSent) }
            phase = .opening
            return adopt ? nil : attachedIdentity
        }
        step("launch")
        // A cancellation that arrived while the launch was held stops it here, before any process
        // exists, so no helper is ever started on behalf of a caller that has gone.
        if locked({ openCancelled }) { throw BridgeError.cancelled(fate: .notSent) }
        let launched = try HelperProcess(
            executable: configuration.helper,
            arguments: configuration.arguments,
            workingDirectory: configuration.workingDirectory,
            maxFrameBytes: configuration.maxResponseBytes,
            shutdownBound: configuration.shutdownBound
        )
        let mine: Int = locked {
            generation &+= 1
            helper = launched
            return generation
        }
        do {
            if locked({ openCancelled }) { throw BridgeError.cancelled(fate: .notSent) }
            let hello = try negotiate(launched, generation: mine)
            let found = try attachSync(launched, generation: mine, hello: hello, expect: expected?.workspaceId)
            if let expected = expected, expected != found {
                throw BridgeError.sourceIdentityChanged(expected: expected, found: found)
            }
            // Published together: nothing sees the connection open before negotiation, attach and
            // the source check have all passed.
            try locked {
                guard !openCancelled, helper === launched, generation == mine else { throw BridgeError.cancelled(fate: .notSent) }
                negotiated = hello
                attachedIdentity = found
                phase = .open
            }
        } catch {
            locked {
                if helper === launched { helper = nil }
                retired.append(launched)
                phase = .closed
            }
            _ = try? endRetired(force: false, current: launched)
            throw error
        }
    }

    private func internalExchange(_ call: Call, version: Int) -> Exchange {
        Exchange(id: nextId(), call: call, protocolVersion: version, deadline: .now() + configuration.requestTimeout, owner: nil)
    }

    private func negotiate(_ launched: HelperProcess, generation: Int) throws -> Hello {
        let version = configuration.supportedProtocols.first ?? 1
        let exchange = internalExchange(.hello(protocols: configuration.supportedProtocols), version: version)
        let result = performSync(exchange, helper: launched, generation: generation, opening: true, protocols: nil, api: nil)
        switch result {
        case .failure(.helperExited(let exit, _)):
            throw BridgeError.launchFailed("\(exit)")
        case .failure(let error):
            throw error
        case .success(let response):
            if let error = response.error {
                let native = NativeError(error)
                if response.id.isEmpty && !Self.frameCodes.contains(error.code) { throw BridgeError.startupRefused(native) }
                if native.code == .unsupportedProtocol { throw BridgeError.incompatible(native.message) }
                throw BridgeError.refused(native)
            }
            guard case .hello(let hello)? = response.result else {
                throw BridgeError.invalidFrame("the first answer was not a hello", fate: .mayHaveBeenProcessed)
            }
            guard configuration.supportedProtocols.contains(hello.protocolVersion) else {
                throw BridgeError.incompatible("the helper chose protocol \(hello.protocolVersion); this client speaks \(configuration.supportedProtocols)")
            }
            guard configuration.supportedApiVersions.contains(hello.apiVersion) else {
                throw BridgeError.incompatible("the helper's envelopes are api version \(hello.apiVersion); this client reads \(configuration.supportedApiVersions)")
            }
            return hello
        }
    }

    private func attachSync(_ launched: HelperProcess, generation: Int, hello: Hello, expect: String?) throws -> Identity {
        let exchange = internalExchange(.attach(expectWorkspace: expect), version: hello.protocolVersion)
        let result = performSync(exchange, helper: launched, generation: generation, opening: true, protocols: [hello.protocolVersion], api: hello.apiVersion)
        switch result {
        case .failure(let error): throw error
        case .success(let response):
            if let error = response.error { throw BridgeError.refused(NativeError(error)) }
            guard case .attached(let attached)? = response.result else {
                throw BridgeError.invalidFrame("the answer to attach was not an attachment", fate: .mayHaveBeenProcessed)
            }
            return Identity(workspaceId: attached.attachment.workspaceId, lineageId: attached.attachment.lineageId, source: attached.source)
        }
    }

    // MARK: One exchange

    /// Write one request and read its one answer, within the exchange's deadline. Blocking: never
    /// on the main thread. The helper's descriptors are used only between `beginIO` and `endIO`.
    private func performSync(_ exchange: Exchange, helper: HelperProcess, generation: Int, opening: Bool, protocols: Set<Int>?, api: Int?) -> Result<Response, BridgeError> {
        guard exchange.beginSend() else { return .failure(.cancelled(fate: .notSent)) }
        step("exchange")
        let usable = locked { self.helper === helper && self.generation == generation && (phase == .open || (opening && phase == .opening)) }
        guard usable, helper.beginIO() else {
            exchange.unmarkSent()
            return .failure(.connectionClosed(fate: .notSent))
        }
        defer { helper.endIO() }
        // Measured before encoded: the encoder never sees a request longer than the bound plus the
        // fixed few hundred bytes of its own framing, whatever the caller handed in.
        if exchange.call.exceeds(configuration.maxRequestBytes) {
            exchange.unmarkSent()
            return .failure(.requestTooLarge(limit: configuration.maxRequestBytes))
        }
        var frame: Data
        do {
            frame = try JSONEncoder().encode(Request(protocolVersion: exchange.protocolVersion, id: exchange.id, call: exchange.call))
        } catch {
            exchange.unmarkSent()
            return .failure(.invalidFrame("the request could not be encoded: \(error)", fate: .notSent))
        }
        if frame.count > configuration.maxRequestBytes {
            exchange.unmarkSent()
            return .failure(.requestTooLarge(limit: configuration.maxRequestBytes))
        }
        frame.append(0x0A)
        switch writeFrame(frame, to: helper.writeDescriptor, abort: helper.abort, deadline: exchange.deadline) {
        case .written:
            break
        case .closed(let bytes):
            // The helper stopped reading. What it wrote before stopping, such as a startup
            // refusal, is still readable below.
            if bytes == 0 { exchange.unmarkSent() }
        case .aborted(let bytes):
            if bytes == 0 { exchange.unmarkSent() }
            return .failure(.connectionClosed(fate: exchange.fate))
        case .timedOut(let bytes):
            if bytes == 0 { exchange.unmarkSent() }
            return .failure(.timedOut(fate: exchange.fate))
        }
        return readAnswer(exchange, helper: helper, protocols: protocols, api: api)
    }

    private func readAnswer(_ exchange: Exchange, helper: HelperProcess, protocols: Set<Int>?, api: Int?) -> Result<Response, BridgeError> {
        switch helper.reader.nextLine(deadline: exchange.deadline) {
        case .line(let data):
            // The deadline bounds the whole exchange: an answer that is only now accepted, however
            // it arrived, is a late answer.
            if expired(exchange.deadline) { return .failure(.timedOut(fate: exchange.fate)) }
            if String(data: data, encoding: .utf8) == nil {
                return .failure(.invalidFrame("an answer is not valid UTF-8", fate: exchange.fate))
            }
            let response: Response
            do {
                response = try JSONDecoder().decode(Response.self, from: data)
            } catch {
                return .failure(.invalidFrame("an answer is not a response: \(error)", fate: exchange.fate))
            }
            // The helper answers in order, one per request. A refusal of the frame itself has no
            // identifier to echo, so it answers the request in flight.
            guard response.id == exchange.id || (response.id.isEmpty && !response.ok) else {
                return .failure(.responseMismatch(expected: exchange.id, actual: response.id, fate: exchange.fate))
            }
            if let reason = Self.inconsistency(response, call: exchange.call, protocols: protocols, api: api) {
                return .failure(.invalidFrame(reason, fate: exchange.fate))
            }
            if expired(exchange.deadline) { return .failure(.timedOut(fate: exchange.fate)) }
            return .success(response)
        case .endOfInput:
            let truncated = helper.reader.pending > 0
            if helper.waitForExit(timeout: Self.exitEventBound), let exit = helper.exitInfo {
                return .failure(truncated ? .invalidFrame("the helper exited inside a frame: \(exit)", fate: exchange.fate) : .helperExited(exit, fate: exchange.fate))
            }
            return .failure(truncated ? .invalidFrame("the helper's output ended inside a frame", fate: exchange.fate) : .connectionClosed(fate: exchange.fate))
        case .timedOut:
            return .failure(.timedOut(fate: exchange.fate))
        case .aborted:
            return .failure(.connectionClosed(fate: exchange.fate))
        case .tooLarge:
            return .failure(.frameTooLarge(limit: configuration.maxResponseBytes, fate: exchange.fate))
        }
    }

    /// Whether an answer contradicts what was negotiated: another protocol than the one agreed, or
    /// an envelope of another api version than the helper announced.
    /// Before a protocol is chosen (`protocols` is nil) any version may answer, since the helper
    /// answers an offer it cannot meet in its own.
    ///
    /// Everything that makes an answer well formed for the call it answers is decided here, before
    /// the answer is accepted, so a bad one invalidates the connection before any caller sees it:
    /// exactly one of result and error, agreeing with `ok`; a result of the kind the call asks for;
    /// and the negotiated api version on every envelope and on every refusal.
    private static func inconsistency(_ response: Response, call: Call, protocols: Set<Int>?, api: Int?) -> String? {
        if let protocols = protocols, !protocols.contains(response.protocolVersion) {
            return "an answer is in protocol \(response.protocolVersion), not the negotiated \(protocols.sorted())"
        }
        if response.ok != (response.error == nil) || response.ok != (response.result != nil) {
            return "an answer's ok flag (\(response.ok)) disagrees with its content (result \(response.result != nil ? "present" : "absent"), error \(response.error != nil ? "present" : "absent"))"
        }
        if let kind = response.result, !kind.answers(call) {
            return "an answer of kind \(kind.kind) does not answer a \(call.kind) call"
        }
        guard let api = api else { return nil }
        switch response.result {
        case .view(let view)? where view.envelope.apiVersion != api:
            return "a view is in api version \(view.envelope.apiVersion), not the negotiated \(api)"
        case .committed(let committed)? where committed.envelope.apiVersion != api:
            return "a commit is in api version \(committed.envelope.apiVersion), not the negotiated \(api)"
        default:
            break
        }
        if let error = response.error, error.apiVersion != api {
            return "a refusal is in api version \(error.apiVersion), not the negotiated \(api)"
        }
        return nil
    }

    /// Whether a failure leaves the byte stream out of step or the helper untrustworthy.
    private func desynchronizes(_ error: BridgeError) -> Bool {
        switch error {
        case .helperExited, .timedOut, .frameTooLarge, .invalidFrame, .responseMismatch: return true
        case .connectionClosed(let fate): return fate == .mayHaveBeenProcessed
        default: return false
        }
    }

    /// Make the connection unusable before anyone learns the helper failed, so that no later
    /// request can be sent on a broken stream. Queued requests, which never left, are refused.
    private func invalidate(_ failed: HelperProcess, generation: Int) -> Bool {
        let unsent: [Exchange]? = locked {
            guard self.generation == generation, helper === failed else { return nil }
            phase = .closed
            helper = nil
            retired.append(failed)
            let waiting = queue
            queue = []
            return waiting
        }
        guard let unsent = unsent else { return false }
        for exchange in unsent { exchange.finish(.failure(.connectionClosed(fate: .notSent))) }
        return true
    }

    // MARK: The exchange queue

    private func admit(_ exchange: Exchange) {
        let verdict: BridgeError? = locked {
            guard phase == .open else { return .connectionClosed(fate: .notSent) }
            if exchange.isResolved { return .cancelled(fate: .notSent) }
            guard queue.count + (inFlight == nil ? 0 : 1) <= Self.maxWaiting else { return .busy(limit: Self.maxWaiting) }
            exchange.pin(generation: generation)
            queue.append(exchange)
            if !draining {
                draining = true
                exchangeQueue.async { self.drain() }
            }
            return nil
        }
        if let verdict = verdict {
            exchange.finish(.failure(verdict))
        } else {
            step("admit")
        }
    }

    /// Called by a cancelled request that never left, so it frees its place at once.
    func dropQueued(_ exchange: Exchange) {
        locked { queue.removeAll { $0 === exchange } }
    }

    private enum Next {
        case idle
        case refuse(Exchange)
        case run(Exchange, HelperProcess, Int)
    }

    private func drain() {
        while true {
            let next: Next = locked {
                guard !queue.isEmpty else {
                    draining = false
                    return .idle
                }
                let exchange = queue.removeFirst()
                guard phase == .open, exchange.generation == generation, let helper = helper else { return .refuse(exchange) }
                inFlight = exchange
                return .run(exchange, helper, generation)
            }
            switch next {
            case .idle:
                return
            case .refuse(let exchange):
                // Admitted to a helper that is gone: refused, never moved to another.
                exchange.finish(.failure(.connectionClosed(fate: .notSent)))
            case .run(let exchange, let helper, let generation):
                let negotiatedProtocol = locked { negotiated.map { Set([$0.protocolVersion]) } ?? Set(configuration.supportedProtocols) }
                let result = performSync(exchange, helper: helper, generation: generation, opening: false, protocols: negotiatedProtocol, api: locked { negotiated?.apiVersion })
                var retire = false
                if case .failure(let error) = result, desynchronizes(error) {
                    // Invalidated before the caller hears of it.
                    retire = invalidate(helper, generation: generation)
                }
                locked { if inFlight === exchange { inFlight = nil } }
                exchange.finish(result)
                if retire { controlQueue.async { _ = try? self.endRetired(force: true, current: nil) } }
            }
        }
    }

    /// Send a call and return its response, whether it succeeded or was refused by the application.
    public func send(_ call: Call, protocolVersion: Int? = nil, timeout: TimeInterval? = nil) async throws -> Response {
        let limit = timeout ?? configuration.requestTimeout
        guard limit.isFinite, limit > 0, limit <= 86_400 else {
            throw BridgeError.invalidConfiguration("a timeout must be a finite number of seconds between 0 and 86400, not \(limit)")
        }
        // The request's own size is checked before it is encoded, so an oversized one costs a walk
        // that stops at the bound, not an allocation of its full length.
        if call.exceeds(configuration.maxRequestBytes) { throw BridgeError.requestTooLarge(limit: configuration.maxRequestBytes) }
        let version = protocolVersion ?? self.protocolVersion ?? configuration.supportedProtocols.first ?? 1
        let exchange = Exchange(id: nextId(), call: call, protocolVersion: version, deadline: .now() + limit, owner: self)
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                exchange.attach(continuation)
                admit(exchange)
            }
        } onCancel: {
            exchange.abandon()
        }
    }

    // MARK: Typed calls

    /// The result of a call, or the application's refusal thrown as `BridgeError.refused`.
    public func result(_ call: Call, timeout: TimeInterval? = nil) async throws -> NativeResult {
        let response = try await send(call, timeout: timeout)
        if let error = response.error { throw BridgeError.refused(NativeError(error)) }
        guard let result = response.result else {
            throw BridgeError.invalidFrame("an answer carried neither a result nor a refusal", fate: .mayHaveBeenProcessed)
        }
        return result
    }

    public func attach(expectWorkspace: String? = nil) async throws -> Attached {
        guard case .attached(let value) = try await result(.attach(expectWorkspace: expectWorkspace)) else {
            throw BridgeError.invalidFrame("the answer to attach was not an attachment", fate: .mayHaveBeenProcessed)
        }
        return value
    }

    public func query(_ query: JSON, attached: Attachment? = nil, timeout: TimeInterval? = nil) async throws -> View {
        guard case .view(let value) = try await result(.query(query, attached: attached), timeout: timeout) else {
            throw BridgeError.invalidFrame("the answer to a query was not a view", fate: .mayHaveBeenProcessed)
        }
        return value
    }

    public func changes(_ since: Cursors, limit: Int? = nil, attached: Attachment? = nil) async throws -> Changes {
        guard case .changes(let value) = try await result(.changes(since, limit: limit, attached: attached)) else {
            throw BridgeError.invalidFrame("the answer to changes was not a change set", fate: .mayHaveBeenProcessed)
        }
        return value
    }

    /// Send a command once. A failure that leaves its outcome unknown is `commandOutcomeUnknown`:
    /// the command may have committed, and this library does not resend it. A commit whose answer
    /// could not be delivered whole is `commandCommitted`, naming what committed.
    public func execute(_ request: CommandRequest, attached: Attachment? = nil) async throws -> Committed {
        do {
            guard case .committed(let value) = try await result(.command(request.json, attached: attached)) else {
                throw BridgeError.invalidFrame("the answer to a command was not a commit", fate: .mayHaveBeenProcessed)
            }
            return value
        } catch let error as BridgeError {
            throw Self.classify(error, for: request)
        }
    }

    /// Send the same command again, on purpose, to learn what became of it. The operation identity
    /// makes this safe: if it committed, the recorded operation comes back; if the same identity
    /// names different content, the application refuses. Never called by the library itself.
    public func reconcile(_ request: CommandRequest, attached: Attachment? = nil) async throws -> Committed {
        try await execute(request, attached: attached)
    }

    static func classify(_ error: BridgeError, for request: CommandRequest) -> BridgeError {
        switch error {
        case .refused(let native) where native.code == .responseTooLarge:
            if let committed = native.details?["committed"], committed != .null {
                return .commandCommitted(CommittedIdentity(
                    operationId: committed["operation_id"].string ?? request.operationId,
                    resultingRevision: committed["resulting_revision"].uint,
                    lineageId: committed["lineage_id"].string
                ))
            }
            return .commandOutcomeUnknown(operationId: request.operationId, cause: "\(native)")
        case .refused(let native) where native.code == .responseEncodingFailed:
            return .commandOutcomeUnknown(operationId: request.operationId, cause: "\(native)")
        default:
            if error.fate == .mayHaveBeenProcessed {
                return .commandOutcomeUnknown(operationId: request.operationId, cause: error.description)
            }
            return error
        }
    }

    // MARK: Ending and replacing the helper

    /// End the helper cleanly, escalating within bounds, and say how it went. It does not wait for
    /// an exchange in flight: that exchange is told the connection closed. Nil when nothing was open.
    @discardableResult
    public func close() async throws -> ShutdownOutcome? {
        try await onControl { try self.endAll(force: false) }
    }

    /// End the helper abruptly, as a crash would. The connection is closed afterwards.
    @discardableResult
    public func kill() async throws -> ShutdownOutcome? {
        try await onControl { try self.endAll(force: true) }
    }

    /// Replace the helper: end the current one, start another on the same selection, negotiate and
    /// attach again. The new helper must hold the same source, or the connection is refused unless
    /// the caller adopts the new identity. Requests that were in flight were not resent.
    public func reconnect(adoptNewIdentity: Bool = false) async throws -> Identity {
        try await onControl {
            _ = try self.endAll(force: false)
            try self.launchAndAttach(adopt: adoptNewIdentity)
            guard let identity = self.identity else { throw BridgeError.connectionClosed(fate: .notSent) }
            return identity
        }
    }

    /// Close the connection to every request, now and atomically: nothing queued will be sent,
    /// the one in flight is told the connection closed, and the helper is aborted and ended.
    private func endAll(force: Bool) throws -> ShutdownOutcome? {
        let (current, waiting, flying) = locked { () -> (HelperProcess?, [Exchange], Exchange?) in
            phase = .closed
            generation &+= 1
            let current = helper
            helper = nil
            if let current = current { retired.append(current) }
            let waiting = queue
            queue = []
            return (current, waiting, inFlight)
        }
        step("shutdown")
        for exchange in waiting { exchange.finish(.failure(.connectionClosed(fate: .notSent))) }
        // Whoever waits on the exchange in flight learns at once, not after the process is gone.
        if let flying = flying { flying.finish(.failure(.connectionClosed(fate: flying.fate))) }
        step("abort")
        current?.abortExchanges()
        return try endRetired(force: force, current: current)
    }

    /// End every helper that is still owed an ending, keeping any that cannot be ended so the next
    /// call retries. The outcome of `current` is returned, or of the first that was ended.
    private func endRetired(force: Bool, current: HelperProcess?) throws -> ShutdownOutcome? {
        let owed = locked { retired }
        var chosen: ShutdownOutcome?
        var failure: Error?
        for stale in owed {
            do {
                let outcome = try stale.shutdown(force: force, bound: configuration.shutdownBound)
                locked { retired.removeAll { $0 === stale } }
                if stale === current || chosen == nil { chosen = outcome }
            } catch {
                failure = failure ?? error
            }
        }
        if let failure = failure { throw failure }
        return chosen
    }
}

extension Call {
    /// Whether the request's own content, the part a caller controls, is longer than `limit`
    /// bytes. Measured exactly and stopped at the limit; what the framing adds around it is a few
    /// hundred bytes and is checked against the limit again once the frame is encoded.
    func exceeds(_ limit: Int) -> Bool {
        var meter = SizeMeter(limit: limit)
        func attachment(_ attached: Attachment?) -> Bool {
            guard let attached = attached else { return false }
            return meter.string(attached.workspaceId) || (attached.lineageId.map { meter.string($0) } ?? false)
        }
        switch self {
        case .hello(let protocols):
            // Measured like the rest, though validation already bounds how many there can be.
            return meter.walk(.array(protocols.map { .integer(Int64($0)) }))
        case .attach(let expected): return expected.map { meter.string($0) } ?? false
        case .query(let json, let attached): return meter.walk(json) || attachment(attached)
        case .command(let json, let attached): return meter.walk(json) || attachment(attached)
        case .changes(let since, _, let attached):
            let epochs = [since.project?.lineageId, since.lifecycle?.epoch, since.activity?.epoch, since.links?.epoch]
            for text in epochs.compactMap({ $0 }) where meter.string(text) { return true }
            return attachment(attached)
        }
    }
}
