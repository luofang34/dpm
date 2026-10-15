// The network's optional time-scaled layout, as a scheduling tool draws its network by day or by week: each column is
// one period of elapsed hours from the clock reading, and a box stands in the period holding its earliest start. Empty
// periods keep their column, so distance across the page reads as time. Geometry only: no hour here is computed, each
// is the projection's own earliest start, and no calendar date is ever inferred.

import Foundation

public enum NetworkTimescale: String, CaseIterable, Sendable {
    /// Columns by the longest chain of gating edges before a node.
    case logical
    case day
    case week

    /// The hours one column spans; nil for the logical layout.
    public var periodHours: Double? {
        switch self {
        case .logical: return nil
        case .day: return 24
        case .week: return 168
        }
    }

    /// The short name the toolbar shows.
    public var title: String {
        switch self {
        case .logical: return "Logical"
        case .day: return "By day"
        case .week: return "By week"
        }
    }

    /// The next layout the key cycles to: logical, by day, by week, and back.
    public var next: NetworkTimescale {
        let all = Self.allCases
        return all[((all.firstIndex(of: self) ?? 0) + 1) % all.count]
    }

    /// The period a column spans, in words: its ordinal and its elapsed hours from the clock reading.
    public func label(column: Int) -> String? {
        guard let period = periodHours else { return nil }
        let from = Double(column) * period
        let unit = self == .day ? "Day" : "Week"
        return "\(unit) \(column + 1) · \(String(format: "%.0f", from))–\(String(format: "%.0f", from + period)) h"
    }

    /// The column of each node, by position in `nodes`: the period of its earliest start. A decision has no time of its
    /// own and stands in the earliest column of the work it gates; anything else without a scheduled start stands in
    /// the first column.
    static func columns(nodes: [NetworkNode], edges: [NetworkEdge], index: [String: Int], period: Double) -> [Int] {
        var column = nodes.map { node in node.earliestStart.map { Int((max(0, $0) / period + 1e-9).rounded(.down)) } }
        for edge in edges where edge.kind == .decisionBlock {
            guard let from = index[edge.from], let to = index[edge.to], nodes[from].earliestStart == nil, let target = column[to] else { continue }
            column[from] = min(column[from] ?? target, target)
        }
        return column.map { $0 ?? 0 }
    }
}
