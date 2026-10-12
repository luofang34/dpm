// The typed calls of a connection, each one `send` whose answer must be of the kind the call asks for,
// and how a command's failure is classified so its caller knows whether it may have committed. A
// request's own size is measured here too, before it is ever encoded.

import Foundation

extension NativeConnection {
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
