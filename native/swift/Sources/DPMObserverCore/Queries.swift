// The only requests the observer makes. Every one is a shared read; there is no command here, and
// the engine has no way to send one.

import DPMNative
import Foundation

enum Queries {
    private static func query(_ name: String, _ fields: [String: JSON] = [:]) -> JSON {
        .object(["query": .string(name)].merging(fields) { $1 })
    }

    /// Forecasts on and five ranked tasks, as the command line defaults and the packaged host ask.
    static let status = query("status", ["probabilistic": .bool(true)])
    static let next = query("next", ["capabilities": .array([]), "probabilistic": .bool(true), "limit": .integer(10)])
    static let inventory = query("export")
    /// The shared schedule projection the Gantt draws, with the forecast on as `status` and `next` have it.
    static let schedule = query("schedule", ["probabilistic": .bool(true)])

    /// The newest runs, or those of one task when `key` names it.
    static func runs(limit: Int, key: String? = nil) -> JSON {
        var fields: [String: JSON] = ["limit": .integer(Int64(limit))]
        if let key = key { fields["key"] = .string(key) }
        return query("runs", fields)
    }
    static func explain(_ key: String) -> JSON { query("explain", ["key": .string(key)]) }
    static func run(_ id: String) -> JSON { query("run", ["id": .string(id)]) }

    static func history(after: UInt64, limit: Int) -> JSON {
        query("history", ["after_sequence": .unsigned(after), "limit": .integer(Int64(limit))])
    }

    static func lifecycle(run: String, after: UInt64, limit: Int) -> JSON {
        query("run_lifecycle", ["after_sequence": .unsigned(after), "limit": .integer(Int64(limit)), "run": .string(run)])
    }

    static func activity(run: String, after: UInt64, limit: Int) -> JSON {
        query("run_activity", ["after_sequence": .unsigned(after), "limit": .integer(Int64(limit)), "run": .string(run)])
    }
}
