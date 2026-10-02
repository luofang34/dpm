// Part of the DPM native client library: JSON the client passes through without interpreting.

import Foundation

/// JSON the client passes through without interpreting, such as a query's envelope data.
public enum JSON: Codable, Equatable, Sendable {
    case null
    case bool(Bool)
    case integer(Int64)
    case unsigned(UInt64)
    case number(Double)
    case string(String)
    case array([JSON])
    case object([String: JSON])

    /// Equal when they print the same: a non-negative integer is the same value whether it was
    /// read as signed or built as unsigned, so a payload compares equal to the text it came from.
    public static func == (left: JSON, right: JSON) -> Bool {
        switch (left, right) {
        case (.null, .null): return true
        case (.bool(let a), .bool(let b)): return a == b
        case (.string(let a), .string(let b)): return a == b
        case (.number(let a), .number(let b)): return a == b
        case (.integer(let a), .integer(let b)): return a == b
        case (.unsigned(let a), .unsigned(let b)): return a == b
        case (.integer(let signed), .unsigned(let unsigned)), (.unsigned(let unsigned), .integer(let signed)):
            return signed >= 0 && UInt64(signed) == unsigned
        case (.array(let a), .array(let b)): return a == b
        case (.object(let a), .object(let b)): return a == b
        default: return false
        }
    }

    public init(from decoder: Decoder) throws {
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

    public func encode(to encoder: Encoder) throws {
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

    public subscript(key: String) -> JSON {
        if case .object(let fields) = self { return fields[key] ?? .null }
        return .null
    }

    public subscript(index: Int) -> JSON {
        if case .array(let items) = self, items.indices.contains(index) { return items[index] }
        return .null
    }

    public var uint: UInt64? {
        switch self {
        case .integer(let value): return value >= 0 ? UInt64(value) : nil
        case .unsigned(let value): return value
        default: return nil
        }
    }

    public var string: String? { if case .string(let value) = self { return value } else { return nil } }
    public var bool: Bool? { if case .bool(let value) = self { return value } else { return nil } }
    public var array: [JSON]? { if case .array(let value) = self { return value } else { return nil } }
}


/// Measures how long JSON would be once written, and stops as soon as it passes a limit, so the
/// cost of asking is bounded by the limit and never by the size of what is measured. Strings are
/// measured with their escapes, as the encoder writes them, and numbers by their digits, so the
/// measure is the real length and not a guess: a value is refused for being too long only when it is.
struct SizeMeter {
    let limit: Int
    private(set) var total = 0

    init(limit: Int) { self.limit = limit }

    var exceeded: Bool { total > limit }

    mutating func add(_ amount: Int) -> Bool {
        total = total &+ amount
        return exceeded
    }

    /// A string with its quotes and escapes: control characters are escaped (`\n` and its kin in
    /// two bytes, the rest in six), as are the quote, the backslash and the slash.
    mutating func string(_ text: String) -> Bool {
        if add(2) { return true }
        for scalar in text.unicodeScalars {
            switch scalar.value {
            case 0x08, 0x09, 0x0A, 0x0C, 0x0D, 0x22, 0x5C, 0x2F: if add(2) { return true }
            case 0..<0x20: if add(6) { return true }
            default: if add(String(scalar).utf8.count) { return true }
            }
        }
        return false
    }

    mutating func number(_ text: String) -> Bool { add(text.utf8.count) }

    /// Whether the value is longer than the limit, found by walking it only as far as needed.
    mutating func walk(_ value: JSON) -> Bool {
        switch value {
        case .null: return add(4)
        case .bool(let flag): return add(flag ? 4 : 5)
        case .integer(let number): return add(String(number).utf8.count)
        case .unsigned(let number): return add(String(number).utf8.count)
        case .number(let number): return add("\(number)".utf8.count)
        case .string(let text): return string(text)
        case .array(let items):
            if add(2) { return true }
            for (index, item) in items.enumerated() where (index > 0 && add(1)) || walk(item) { return true }
            return false
        case .object(let fields):
            if add(2) { return true }
            var first = true
            for (key, item) in fields {
                if !first && add(1) { return true }
                first = false
                if string(key) || add(1) || walk(item) { return true }
            }
            return false
        }
    }
}

extension JSON {
    /// Whether this value, written out, would be longer than `limit` bytes.
    public func exceeds(_ limit: Int) -> Bool {
        var meter = SizeMeter(limit: limit)
        return meter.walk(self)
    }
}
