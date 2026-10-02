// Part of the DPM native client library: exact RFC 3339 instants.

import Foundation

/// An RFC 3339 instant in UTC as the boundary writes it. Comparing instants never rounds.
public struct Instant: Codable, Comparable, Hashable, CustomStringConvertible, Sendable {
    public let seconds: Int64
    public let nanos: Int

    public init(seconds: Int64, nanos: Int = 0) {
        self.seconds = seconds
        self.nanos = nanos
    }

    public init?(_ text: String) {
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

    public init(from decoder: Decoder) throws {
        let text = try decoder.singleValueContainer().decode(String.self)
        guard let value = Instant(text) else {
            throw DecodingError.dataCorrupted(.init(codingPath: decoder.codingPath, debugDescription: "not an RFC 3339 UTC instant: \(text)"))
        }
        self = value
    }

    public func encode(to encoder: Encoder) throws {
        var single = encoder.singleValueContainer()
        try single.encode(description)
    }

    public func adding(seconds delta: Int64, nanos extra: Int = 0) -> Instant {
        var total = nanos + extra
        var whole = seconds + delta
        while total >= 1_000_000_000 { total -= 1_000_000_000; whole += 1 }
        while total < 0 { total += 1_000_000_000; whole -= 1 }
        return Instant(seconds: whole, nanos: total)
    }

    public static func < (left: Instant, right: Instant) -> Bool {
        (left.seconds, left.nanos) < (right.seconds, right.nanos)
    }

    public var description: String {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime]
        let whole = formatter.string(from: Date(timeIntervalSince1970: TimeInterval(seconds)))
        let fraction = String(format: "%09d", nanos)
        return String(whole.dropLast()) + "." + fraction + "Z"
    }
}

