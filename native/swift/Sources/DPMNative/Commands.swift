// A project command as the client builds it, and the identity that makes it safe to reconcile.
//
// Every command carries a version 7 operation identity, minted here when the caller supplies none.
// The application treats that identity as an idempotency key: a resend with the same identity and
// the same content returns the operation it recorded, and any other content under that identity is
// refused. So a command whose outcome is unknown can be reconciled by an explicit resend of this
// same request, and the library never resends one on its own.

import Foundation

/// A version 7 (time-ordered) UUID in the lowercase hyphenated form the application requires.
public enum UUIDv7 {
    public static func make() -> String {
        var bytes = [UInt8](repeating: 0, count: 16)
        for index in 0..<16 { bytes[index] = UInt8.random(in: 0...255) }
        let milliseconds = UInt64(Date().timeIntervalSince1970 * 1000)
        for index in 0..<6 { bytes[index] = UInt8((milliseconds >> UInt64(8 * (5 - index))) & 0xFF) }
        bytes[6] = (bytes[6] & 0x0F) | 0x70
        bytes[8] = (bytes[8] & 0x3F) | 0x80
        let hex = bytes.map { String(format: "%02x", $0) }
        let text = hex.joined()
        let parts = [0..<8, 8..<12, 12..<16, 16..<20, 20..<32].map { range -> String in
            let start = text.index(text.startIndex, offsetBy: range.lowerBound)
            let end = text.index(text.startIndex, offsetBy: range.upperBound)
            return String(text[start..<end])
        }
        return parts.joined(separator: "-")
    }
}

/// A shared project command with the preconditions the application checks: actor, revision and
/// lineage. Nothing here decides whether the command is valid.
public struct CommandRequest: Sendable {
    public var actor: JSON
    public var baseRevision: UInt64
    public var baseLineage: String?
    public var operationId: String
    /// The command in the application's own form, such as `{"Claim": {"work": "<id>"}}`.
    public var command: JSON

    public init(actor: JSON, baseRevision: UInt64, baseLineage: String? = nil, operationId: String = UUIDv7.make(), command: JSON) {
        self.actor = actor
        self.baseRevision = baseRevision
        self.baseLineage = baseLineage
        self.operationId = operationId
        self.command = command
    }

    var json: JSON {
        var fields: [String: JSON] = [
            "actor": actor,
            "base_revision": .unsigned(baseRevision),
            "operation_id": .string(operationId),
            "command": command,
        ]
        if let lineage = baseLineage { fields["base_lineage"] = .string(lineage) }
        return .object(fields)
    }
}
