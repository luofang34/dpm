// The typed side of the DPM native boundary, as a macOS client would hold it.
//
// This file decodes the answers of the Rust boundary and encodes its requests. It holds no
// readiness, validation or scheduling rule: every value that depends on a rule (ready, blocked,
// stale, the instant a view next changes) arrives in an answer and is only compared or displayed.
// A proof fixture, not the UI-30 bridge.

import Foundation

// MARK: - Dynamic JSON for the parts of an answer the shared application owns

/// JSON the client passes through without interpreting, such as a query's envelope data.
enum JSON: Codable, Equatable {
    case null
    case bool(Bool)
    case integer(Int64)
    case unsigned(UInt64)
    case number(Double)
    case string(String)
    case array([JSON])
    case object([String: JSON])

    init(from decoder: Decoder) throws {
        let single = try decoder.singleValueContainer()
        if single.decodeNil() {
            self = .null
        } else if let value = try? single.decode(Bool.self) {
            self = .bool(value)
        } else if let value = try? single.decode(Int64.self) {
            self = .integer(value)
        } else if let value = try? single.decode(UInt64.self) {
            self = .unsigned(value)
        } else if let value = try? single.decode(Double.self) {
            self = .number(value)
        } else if let value = try? single.decode(String.self) {
            self = .string(value)
        } else if let value = try? single.decode([JSON].self) {
            self = .array(value)
        } else {
            self = .object(try single.decode([String: JSON].self))
        }
    }

    func encode(to encoder: Encoder) throws {
        var single = encoder.singleValueContainer()
        switch self {
        case .null: try single.encodeNil()
        case .bool(let value): try single.encode(value)
        case .integer(let value): try single.encode(value)
        case .unsigned(let value): try single.encode(value)
        case .number(let value): try single.encode(value)
        case .string(let value): try single.encode(value)
        case .array(let value): try single.encode(value)
        case .object(let value): try single.encode(value)
        }
    }

    subscript(key: String) -> JSON {
        if case .object(let fields) = self { return fields[key] ?? .null }
        return .null
    }

    subscript(index: Int) -> JSON {
        if case .array(let items) = self, items.indices.contains(index) { return items[index] }
        return .null
    }

    var uint: UInt64? {
        switch self {
        case .integer(let value): return value >= 0 ? UInt64(value) : nil
        case .unsigned(let value): return value
        default: return nil
        }
    }

    var string: String? { if case .string(let value) = self { return value } else { return nil } }
    var bool: Bool? { if case .bool(let value) = self { return value } else { return nil } }
    var array: [JSON]? { if case .array(let value) = self { return value } else { return nil } }
}

// MARK: - Instants (RFC 3339, exact to the nanosecond, never a Double)

/// An RFC 3339 instant in UTC as the boundary writes it. Comparing instants never rounds.
struct Instant: Codable, Comparable, Hashable, CustomStringConvertible {
    let seconds: Int64
    let nanos: Int

    init(seconds: Int64, nanos: Int = 0) {
        self.seconds = seconds
        self.nanos = nanos
    }

    init?(_ text: String) {
        guard text.hasSuffix("Z") || text.hasSuffix("+00:00") else { return nil }
        let body = text.hasSuffix("Z") ? String(text.dropLast()) : String(text.dropLast(6))
        let parts = body.split(separator: ".", maxSplits: 1, omittingEmptySubsequences: false)
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        guard let whole = formatter.date(from: String(parts[0]) + "Z") else { return nil }
        var nanos = 0
        if parts.count == 2 {
            let digits = parts[1].prefix(9)
            guard let value = Int(digits) else { return nil }
            nanos = value * Int(pow(10.0, Double(9 - digits.count)))
        }
        self.seconds = Int64(whole.timeIntervalSince1970.rounded())
        self.nanos = nanos
    }

    init(from decoder: Decoder) throws {
        let text = try decoder.singleValueContainer().decode(String.self)
        guard let value = Instant(text) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "not an RFC 3339 UTC instant: \(text)"))
        }
        self = value
    }

    func encode(to encoder: Encoder) throws {
        var single = encoder.singleValueContainer()
        try single.encode(description)
    }

    func adding(seconds delta: Int64, nanos extra: Int = 0) -> Instant {
        var total = nanos + extra
        var whole = seconds + delta
        while total >= 1_000_000_000 { total -= 1_000_000_000; whole += 1 }
        while total < 0 { total += 1_000_000_000; whole -= 1 }
        return Instant(seconds: whole, nanos: total)
    }

    static func < (left: Instant, right: Instant) -> Bool {
        (left.seconds, left.nanos) < (right.seconds, right.nanos)
    }

    var description: String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        let whole = formatter.string(from: Date(timeIntervalSince1970: TimeInterval(seconds)))
        let fraction = String(format: "%09d", nanos)
        return String(whole.dropLast()) + "." + fraction + "Z"
    }
}

// MARK: - Typed refusals

/// The stable category of a refusal. A code this build of the client does not know is kept, never
/// mapped to a guess: a newer host may add codes.
enum ErrorCode: Equatable, CustomStringConvertible {
    case unsupportedProtocol, invalidRequest, workspaceMismatch, lineageMismatch, sourceChanged
    case workspaceChanging, revisionConflict, readOnlyProject, archivedStore, invalidCommand, notFound
    case unknown(String)

    init(_ code: String) {
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
        default: self = .unknown(code)
        }
    }

    /// The code as it crosses the boundary.
    var wire: String {
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
        case .unknown(let code): return code
        }
    }

    var description: String { wire }
}

/// A refusal as it crosses the boundary, in the shape CLI and tool error objects use.
struct ErrorBody: Decodable {
    let apiVersion: Int
    let code: String
    let message: String
    let details: JSON?

    enum CodingKeys: String, CodingKey { case apiVersion = "api_version", code, message, details }
}

/// A refusal thrown to the caller, typed by its code.
struct NativeError: Error, CustomStringConvertible {
    let code: ErrorCode
    let message: String
    let details: JSON?
    var description: String { "\(code): \(message)" }
}

// MARK: - Identity and positions

struct Attachment: Codable, Equatable {
    let workspaceId: String
    let lineageId: String?
    enum CodingKeys: String, CodingKey { case workspaceId = "workspace_id", lineageId = "lineage_id" }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encode(workspaceId, forKey: .workspaceId)
        try container.encodeIfPresent(lineageId, forKey: .lineageId)
    }
}

/// Where the project's operation history stands. Revisions wrap, so a watermark is compared only
/// for equality, never for order.
struct ProjectWatermark: Codable, Equatable {
    let workspaceId: String
    let lineageId: String?
    let revision: UInt64
    let historyHead: UInt64
    enum CodingKeys: String, CodingKey {
        case workspaceId = "workspace_id", lineageId = "lineage_id", revision, historyHead = "history_head"
    }
}

struct RunHeads: Codable, Equatable {
    let epoch: String?
    let lifecycleHead: UInt64
    let activityHead: UInt64
    let activityPrunedThrough: UInt64
    let linkCount: UInt64
    enum CodingKeys: String, CodingKey {
        case epoch, lifecycleHead = "lifecycle_head", activityHead = "activity_head"
        case activityPrunedThrough = "activity_pruned_through", linkCount = "link_count"
    }
}

struct Watermark: Codable, Equatable {
    let project: ProjectWatermark
    let runs: RunHeads
}

struct ProjectCursor: Codable, Equatable {
    var lineageId: String?
    var afterSequence: UInt64
    enum CodingKeys: String, CodingKey { case lineageId = "lineage_id", afterSequence = "after_sequence" }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(lineageId, forKey: .lineageId)
        try container.encode(afterSequence, forKey: .afterSequence)
    }
}

struct FeedCursor: Codable, Equatable {
    var epoch: String?
    var afterSequence: UInt64
    enum CodingKeys: String, CodingKey { case epoch, afterSequence = "after_sequence" }

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(epoch, forKey: .epoch)
        try container.encode(afterSequence, forKey: .afterSequence)
    }
}

struct LinkMark: Codable, Equatable {
    var epoch: String?
    var count: UInt64

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(epoch, forKey: .epoch)
        try container.encode(count, forKey: .count)
    }
    enum CodingKeys: String, CodingKey { case epoch, count }
}

struct Cursors: Encodable {
    var project: ProjectCursor?
    var lifecycle: FeedCursor?
    var activity: FeedCursor?
    var links: LinkMark?

    func encode(to encoder: Encoder) throws {
        var container = encoder.container(keyedBy: CodingKeys.self)
        try container.encodeIfPresent(project, forKey: .project)
        try container.encodeIfPresent(lifecycle, forKey: .lifecycle)
        try container.encodeIfPresent(activity, forKey: .activity)
        try container.encodeIfPresent(links, forKey: .links)
    }
    enum CodingKeys: String, CodingKey { case project, lifecycle, activity, links }
}

// MARK: - Answers

struct Envelope: Decodable {
    let apiVersion: Int
    let revision: UInt64?
    let lineageId: String?
    let data: JSON
    enum CodingKeys: String, CodingKey { case apiVersion = "api_version", revision, lineageId = "lineage_id", data }
}

struct ViewMapping: Decodable {
    let view: String
    let queries: [String]
    let feeds: [String]
    let note: String
}

struct UnsupportedControl: Decodable { let control: String; let reason: String }

struct Limits: Decodable {
    let maxPage: Int
    let defaultPage: Int
    let staleAfterSeconds: Int
    let reevaluateWithinSeconds: Int
    enum CodingKeys: String, CodingKey {
        case maxPage = "max_page", defaultPage = "default_page"
        case staleAfterSeconds = "stale_after_seconds", reevaluateWithinSeconds = "reevaluate_within_seconds"
    }
}

struct RunCapabilities: Decodable {
    let supported: Bool
    let observation: [String]
    let reportedOnly: String
    let completion: String
    enum CodingKeys: String, CodingKey { case supported, observation, reportedOnly = "reported_only", completion }
}

struct Capabilities: Decodable {
    let views: [ViewMapping]
    let runs: RunCapabilities
    let commands: Bool
    let unsupported: [UnsupportedControl]
    let limits: Limits
}

struct Hello: Decodable {
    let protocolVersion: Int
    let supported: [Int]
    let apiVersion: Int
    let capabilities: Capabilities
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol", supported, apiVersion = "api_version", capabilities }
}

struct Attached: Decodable {
    let attachment: Attachment
    let source: String
    let watermark: Watermark
    let evaluatedAt: Instant
    enum CodingKeys: String, CodingKey { case attachment, source, watermark, evaluatedAt = "evaluated_at" }
}

/// Where each store an answer read stood just before it was read: a lower bound, so a subscription
/// from it misses nothing and may repeat what the answer already holds. A store the query did not
/// read has no basis, so a project-only answer can never move a run cursor.
struct ViewBasis: Decodable, Equatable {
    let project: ProjectWatermark?
    let runs: RunHeads?
}

struct View: Decodable {
    let evaluatedAt: Instant
    let basis: ViewBasis
    let refreshAt: Instant?
    let envelope: Envelope
    enum CodingKeys: String, CodingKey { case evaluatedAt = "evaluated_at", basis, refreshAt = "refresh_at", envelope }
}

struct Committed: Decodable { let envelope: Envelope }

/// A page of one feed. `state` is `continue` or `reset`; a reset carries no entries.
struct Delta: Decodable {
    let state: String
    let reason: String?
    let entries: [JSON]
    let nextAfterSequence: UInt64
    let more: Bool
    let gap: JSON?
    enum CodingKeys: String, CodingKey {
        case state, reason, entries, nextAfterSequence = "next_after_sequence", more, gap
    }
    var isReset: Bool { state == "reset" }
}

struct LinkSignal: Decodable {
    let state: String
    let reason: String?
    let changed: Bool
    let count: UInt64
    var isReset: Bool { state == "reset" }
}

struct Changes: Decodable {
    let evaluatedAt: Instant
    let watermark: Watermark
    let project: Delta?
    let lifecycle: Delta?
    let activity: Delta?
    let links: LinkSignal?
    enum CodingKeys: String, CodingKey { case evaluatedAt = "evaluated_at", watermark, project, lifecycle, activity, links }
}

enum Result {
    case hello(Hello)
    case attached(Attached)
    case view(View)
    case changes(Changes)
    case committed(Committed)
    /// A result kind this client does not know: kept, never guessed at.
    case unknown(String)
}

struct Response: Decodable {
    let protocolVersion: Int
    let id: String
    let ok: Bool
    let result: Result?
    let error: ErrorBody?
    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol", id, ok, result, error }

    init(from decoder: Decoder) throws {
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
enum Call {
    case hello(protocols: [Int])
    case attach(expectWorkspace: String?)
    case query(JSON, attached: Attachment?)
    case changes(Cursors, limit: Int?, attached: Attachment?)
    case command(JSON, attached: Attachment?)
}

struct Request: Encodable {
    let protocolVersion: Int
    let id: String
    let call: Call

    enum CodingKeys: String, CodingKey { case protocolVersion = "protocol", id, call }
    private enum CallKeys: String, CodingKey { case type, protocols, expectWorkspace = "expect_workspace", query, attached, since, limit, request }

    func encode(to encoder: Encoder) throws {
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
