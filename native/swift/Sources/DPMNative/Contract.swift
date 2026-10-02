// Part of the DPM native client library: typed values of the versioned native contract.

import Foundation

// MARK: - Typed refusals

/// The stable category of a refusal. A code this build of the client does not know is kept, never
/// mapped to a guess: a newer host may add codes.
public enum ErrorCode: Equatable, CustomStringConvertible {
    case unsupportedProtocol, invalidRequest, workspaceMismatch, lineageMismatch, sourceChanged
    case workspaceChanging, revisionConflict, readOnlyProject, archivedStore, invalidCommand, notFound
    /// A request line was longer than the helper accepts.
    case frameTooLarge
    /// A request line was not valid UTF-8.
    case invalidEncoding
    /// The helper's input ended inside a request line.
    case truncatedFrame
    /// An answer was longer than the helper sends, so it was not sent. For a command it names what
    /// was committed in the error's details.
    case responseTooLarge
    case responseEncodingFailed
    /// The helper was started with arguments it cannot run.
    case invalidOptions
    case workingDirectoryUnavailable
    /// A resend of a command whose identity is already recorded.
    case duplicateOperation
    /// A correlation identifier outside the accepted form; the request was not processed.
    case invalidId
    case unknown(String)

    public init(_ code: String) {
        switch code {
        case "unsupported_protocol": self = .unsupportedProtocol
        case "invalid_request": self = .invalidRequest
        case "workspace_mismatch": self = .workspaceMismatch
        case "lineage_mismatch": self = .lineageMismatch
        case "source_changed": self = .sourceChanged
        case "workspace_changing": self = .workspaceChanging
        case "revision_conflict": self = .revisionConflict
        case "read_only_project": self = .readOnlyProject
        case "archived_store": self = .archivedStore
        case "invalid_command": self = .invalidCommand
        case "not_found": self = .notFound
        case "frame_too_large": self = .frameTooLarge
        case "invalid_encoding": self = .invalidEncoding
        case "truncated_frame": self = .truncatedFrame
        case "response_too_large": self = .responseTooLarge
        case "response_encoding_failed": self = .responseEncodingFailed
        case "invalid_options": self = .invalidOptions
        case "working_directory_unavailable": self = .workingDirectoryUnavailable
        case "duplicate_operation": self = .duplicateOperation
        case "invalid_id": self = .invalidId
        default: self = .unknown(code)
        }
    }

    /// The code as it crosses the boundary.
    public var wire: String {
        switch self {
        case .unsupportedProtocol: return "unsupported_protocol"
        case .invalidRequest: return "invalid_request"
        case .workspaceMismatch: return "workspace_mismatch"
        case .lineageMismatch: return "lineage_mismatch"
        case .sourceChanged: return "source_changed"
        case .workspaceChanging: return "workspace_changing"
        case .revisionConflict: return "revision_conflict"
        case .readOnlyProject: return "read_only_project"
        case .archivedStore: return "archived_store"
        case .invalidCommand: return "invalid_command"
        case .notFound: return "not_found"
        case .frameTooLarge: return "frame_too_large"
        case .invalidEncoding: return "invalid_encoding"
        case .truncatedFrame: return "truncated_frame"
        case .responseTooLarge: return "response_too_large"
        case .responseEncodingFailed: return "response_encoding_failed"
        case .invalidOptions: return "invalid_options"
        case .workingDirectoryUnavailable: return "working_directory_unavailable"
        case .duplicateOperation: return "duplicate_operation"
        case .invalidId: return "invalid_id"
        case .unknown(let code): return code
        }
    }

    public var description: String { wire }
}

/// A refusal as it crosses the boundary, in the shape CLI and tool error objects use.
public struct ErrorBody: Decodable {
    public let apiVersion: Int
    public let code: String
    public let message: String
    public let details: JSON?

    enum CodingKeys: String, CodingKey { case apiVersion = "api_version", code, message, details }
}

/// A refusal thrown to the caller, typed by its code.
public struct NativeError: Error, CustomStringConvertible {
    public let code: ErrorCode
    public let message: String
    public let details: JSON?
    /// The application wire contract version the refusal was made under, when the helper said.
    public let apiVersion: Int?
    public var description: String { "\(code): \(message)" }

    public init(code: ErrorCode, message: String, details: JSON?, apiVersion: Int? = nil) {
        self.code = code
        self.message = message
        self.details = details
        self.apiVersion = apiVersion
    }

    init(_ body: ErrorBody) {
        self.init(code: ErrorCode(body.code), message: body.message, details: body.details, apiVersion: body.apiVersion)
    }

    /// The refusal in the shape the CLI `--json` output and the agent tools use.
    public var json: JSON {
        var fields: [String: JSON] = ["code": .string(code.wire), "message": .string(message)]
        if let version = apiVersion { fields["api_version"] = .integer(Int64(version)) }
        if let details = details, details != .null { fields["details"] = details }
        return .object(["error": .object(fields)])
    }
}

// MARK: - Identity and positions

public struct Attachment: Codable, Equatable {
    public let workspaceId: String
    public let lineageId: String?
    enum CodingKeys: String, CodingKey { case workspaceId = "workspace_id", lineageId = "lineage_id" }

    public init(workspaceId: String, lineageId: String?) {
        self.workspaceId = workspaceId
        self.lineageId = lineageId
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(workspaceId, forKey: .workspaceId)
        try container.encodeIfPresent(lineageId, forKey: .lineageId)
    }
}

/// Where the project's operation history stands. Revisions wrap, so a watermark is compared only
/// for equality, never for order.
public struct ProjectWatermark: Codable, Equatable {
    public let workspaceId: String
    public let lineageId: String?
    public let revision: UInt64
    public let historyHead: UInt64
    enum CodingKeys: String, CodingKey {
        case workspaceId = "workspace_id", lineageId = "lineage_id", revision, historyHead = "history_head"
    }
}

public struct RunHeads: Codable, Equatable {
    public let epoch: String?
    public let lifecycleHead: UInt64
    public let activityHead: UInt64
    public let activityPrunedThrough: UInt64
    public let linkCount: UInt64
    enum CodingKeys: String, CodingKey {
        case epoch, lifecycleHead = "lifecycle_head", activityHead = "activity_head"
        case activityPrunedThrough = "activity_pruned_through", linkCount = "link_count"
    }
}

public struct Watermark: Codable, Equatable {
    public let project: ProjectWatermark
    public let runs: RunHeads
}

public struct ProjectCursor: Codable, Equatable {
    public var lineageId: String?
    public var afterSequence: UInt64
    enum CodingKeys: String, CodingKey { case lineageId = "lineage_id", afterSequence = "after_sequence" }

    public init(lineageId: String?, afterSequence: UInt64) {
        self.lineageId = lineageId
        self.afterSequence = afterSequence
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(lineageId, forKey: .lineageId)
        try container.encode(afterSequence, forKey: .afterSequence)
    }
}

public struct FeedCursor: Codable, Equatable {
    public var epoch: String?
    public var afterSequence: UInt64
    enum CodingKeys: String, CodingKey { case epoch, afterSequence = "after_sequence" }

    public init(epoch: String?, afterSequence: UInt64) {
        self.epoch = epoch
        self.afterSequence = afterSequence
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(epoch, forKey: .epoch)
        try container.encode(afterSequence, forKey: .afterSequence)
    }
}

public struct LinkMark: Codable, Equatable {
    public var epoch: String?
    public var count: UInt64

    public init(epoch: String?, count: UInt64) {
        self.epoch = epoch
        self.count = count
    }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(epoch, forKey: .epoch)
        try container.encode(count, forKey: .count)
    }
    enum CodingKeys: String, CodingKey { case epoch, count }
}

public struct Cursors: Encodable {
    public var project: ProjectCursor?
    public var lifecycle: FeedCursor?
    public var activity: FeedCursor?
    public var links: LinkMark?

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(project, forKey: .project)
        try container.encodeIfPresent(lifecycle, forKey: .lifecycle)
        try container.encodeIfPresent(activity, forKey: .activity)
        try container.encodeIfPresent(links, forKey: .links)
    }
    enum CodingKeys: String, CodingKey { case project, lifecycle, activity, links }
}

// MARK: - Answers

public struct Envelope: Decodable {
    public let apiVersion: Int
    public let revision: UInt64?
    public let lineageId: String?
    public let data: JSON
    enum CodingKeys: String, CodingKey { case apiVersion = "api_version", revision, lineageId = "lineage_id", data }
}

public struct ViewMapping: Decodable {
    public let view: String
    public let queries: [String]
    public let feeds: [String]
    public let note: String
}

public struct UnsupportedControl: Decodable {
    public let control: String
    public let reason: String
}

public struct Limits: Decodable {
    public let maxPage: Int
    public let defaultPage: Int
    public let staleAfterSeconds: Int
    public let reevaluateWithinSeconds: Int
    enum CodingKeys: String, CodingKey {
        case maxPage = "max_page", defaultPage = "default_page"
        case staleAfterSeconds = "stale_after_seconds", reevaluateWithinSeconds = "reevaluate_within_seconds"
    }
}

public struct RunCapabilities: Decodable {
    public let supported: Bool
    public let observation: [String]
    public let reportedOnly: String
    public let completion: String
    enum CodingKeys: String, CodingKey { case supported, observation, reportedOnly = "reported_only", completion }
}

public struct Capabilities: Decodable {
    public let views: [ViewMapping]
    public let runs: RunCapabilities
    public let commands: Bool
    public let unsupported: [UnsupportedControl]
    public let limits: Limits
}

public struct Hello: Decodable {
    public let protocolVersion: Int
    public let supported: [Int]
    public let apiVersion: Int
    public let capabilities: Capabilities
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol", supported, apiVersion = "api_version", capabilities }
}

public struct Attached: Decodable {
    public let attachment: Attachment
    public let source: String
    public let watermark: Watermark
    public let evaluatedAt: Instant
    enum CodingKeys: String, CodingKey { case attachment, source, watermark, evaluatedAt = "evaluated_at" }
}

/// Where each store an answer read stood just before it was read: a lower bound, so a subscription
/// from it misses nothing and may repeat what the answer already holds. A store the query did not
/// read has no basis, so a project-only answer can never move a run cursor.
public struct ViewBasis: Codable, Equatable {
    public let project: ProjectWatermark?
    public let runs: RunHeads?
}

public struct View: Decodable {
    public let evaluatedAt: Instant
    public let basis: ViewBasis
    public let refreshAt: Instant?
    public let envelope: Envelope
    enum CodingKeys: String, CodingKey { case evaluatedAt = "evaluated_at", basis, refreshAt = "refresh_at", envelope }
}

public struct Committed: Decodable { public let envelope: Envelope }

/// A page of one feed. `state` is `continue` or `reset`; a reset carries no entries.
public struct Delta: Decodable {
    public let state: String
    public let reason: String?
    public let entries: [JSON]
    public let nextAfterSequence: UInt64
    public let more: Bool
    public let gap: JSON?
    enum CodingKeys: String, CodingKey {
        case state, reason, entries, nextAfterSequence = "next_after_sequence", more, gap
    }
    public var isReset: Bool { state == "reset" }
}

public struct LinkSignal: Decodable {
    public let state: String
    public let reason: String?
    public let changed: Bool
    public let count: UInt64
    public var isReset: Bool { state == "reset" }
}

public struct Changes: Decodable {
    public let evaluatedAt: Instant
    public let watermark: Watermark
    public let project: Delta?
    public let lifecycle: Delta?
    public let activity: Delta?
    public let links: LinkSignal?
    enum CodingKeys: String, CodingKey { case evaluatedAt = "evaluated_at", watermark, project, lifecycle, activity, links }
}

public enum NativeResult {
    case hello(Hello)
    case attached(Attached)
    case view(View)
    case changes(Changes)
    case committed(Committed)
    /// A result kind this client does not know: kept, never guessed at.
    case unknown(String)
}

extension NativeResult {
    /// The name of the kind of result, as the boundary writes it.
    var kind: String {
        switch self {
        case .hello: return "hello"
        case .attached: return "attached"
        case .view: return "view"
        case .changes: return "changes"
        case .committed: return "committed"
        case .unknown(let kind): return kind
        }
    }

    /// Whether this is the kind of result the call asks for. A kind this client does not know
    /// answers no call it makes.
    func answers(_ call: Call) -> Bool { kind == call.kind }
}

extension Call {
    /// The kind of result this call is answered with.
    var kind: String {
        switch self {
        case .hello: return "hello"
        case .attach: return "attached"
        case .query: return "view"
        case .changes: return "changes"
        case .command: return "committed"
        }
    }
}

public struct Response: Decodable {
    public let protocolVersion: Int
    public let id: String
    public let ok: Bool
    public let result: NativeResult?
    public let error: ErrorBody?
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol", id, ok, result, error }

    public init(from decoder: Decoder) throws {
        let container = try decoder.container(keyedBy: CodingKeys.self)
        protocolVersion = try container.decode(Int.self, forKey: .protocolVersion)
        id = try container.decode(String.self, forKey: .id)
        ok = try container.decode(Bool.self, forKey: .ok)
        error = try container.decodeIfPresent(ErrorBody.self, forKey: .error)
        if container.contains(.result) {
            let nested = try container.superDecoder(forKey: .result)
            let kind = try nested.container(keyedBy: KindKey.self).decode(String.self, forKey: .kind)
            switch kind {
            case "hello": result = .hello(try Hello(from: nested))
            case "attached": result = .attached(try Attached(from: nested))
            case "view": result = .view(try View(from: nested))
            case "changes": result = .changes(try Changes(from: nested))
            case "committed": result = .committed(try Committed(from: nested))
            default: result = .unknown(kind)
            }
        } else {
            result = nil
        }
    }

    private enum KindKey: String, CodingKey { case kind }
}

// MARK: - Requests

/// A call. Every read is a shared query; the only write is the shared command request.
public enum Call {
    case hello(protocols: [Int])
    case attach(expectWorkspace: String?)
    case query(JSON, attached: Attachment?)
    case changes(Cursors, limit: Int?, attached: Attachment?)
    case command(JSON, attached: Attachment?)
}

public struct Request: Encodable {
    public let protocolVersion: Int
    public let id: String
    public let call: Call

    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol", id, call }
    private enum CallKeys: String, CodingKey { case type, protocols, expectWorkspace = "expect_workspace", query, attached, since, limit, request }

    public func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(protocolVersion, forKey: .protocolVersion)
        try container.encode(id, forKey: .id)
        var call = container.nestedContainer(keyedBy: CallKeys.self, forKey: .call)
        switch self.call {
        case .hello(let protocols):
            try call.encode("hello", forKey: .type)
            try call.encode(protocols, forKey: .protocols)
        case .attach(let expected):
            try call.encode("attach", forKey: .type)
            try call.encodeIfPresent(expected, forKey: .expectWorkspace)
        case .query(let query, let attached):
            try call.encode("query", forKey: .type)
            try call.encode(query, forKey: .query)
            try call.encodeIfPresent(attached, forKey: .attached)
        case .changes(let since, let limit, let attached):
            try call.encode("changes", forKey: .type)
            try call.encode(since, forKey: .since)
            try call.encodeIfPresent(limit, forKey: .limit)
            try call.encodeIfPresent(attached, forKey: .attached)
        case .command(let request, let attached):
            try call.encode("command", forKey: .type)
            try call.encode(request, forKey: .request)
            try call.encodeIfPresent(attached, forKey: .attached)
        }
    }
}

extension View {
    /// The envelope exactly as the CLI `--json` output and the agent tools carry it, for a client
    /// that hands a view on or compares it.
    public var envelopeJSON: JSON {
        .object([
            "api_version": .integer(Int64(envelope.apiVersion)),
            "revision": envelope.revision.map { .unsigned($0) } ?? .null,
            "lineage_id": envelope.lineageId.map { .string($0) } ?? .null,
            "data": envelope.data,
        ])
    }
}
