// Where the Gantt's emphasised relation labels go on the chart. Several relations can end at one point of
// one row (every incoming relation of a milestone, say), so a label is never simply drawn at its arrowhead:
// each one takes the first line above its arrowhead that touches no label already placed. A target with
// more incoming relations than fit gets an aggregate line instead of the rest; a label that finds no free
// line is left off the chart. Nothing is lost: every relation stays in the row's text and in Detail. A label
// stays inside the chart's width, and prefers a line above or below that covers no bar, so it never hides the work it describes.
// Pure view geometry: the text is the relation's own words and the order is fixed, so every computation
// over the same relations places the same labels.

import Foundation

public enum GanttRelationLabels {
    /// One relation label to place: its arrowhead, and the side of it the text runs to.
    public struct Request: Equatable, Sendable {
        public var id: String
        /// The work item the relation ends at.
        public var target: String
        public var text: String
        public var x: Double
        public var y: Double
        /// The text starts at `x` and runs right; otherwise it ends at `x`.
        public var leading: Bool
        /// The drawn text's own width when the caller measured it; otherwise a generous estimate is used.
        public var measured: Double?

        public init(id: String, target: String, text: String, x: Double, y: Double, leading: Bool, measured: Double? = nil) {
            self.id = id
            self.target = target
            self.text = text
            self.x = x
            self.y = y
            self.leading = leading
            self.measured = measured
        }
    }

    /// A rectangle the labels should leave visible (a bar), in chart points.
    public struct Obstacle: Equatable, Sendable {
        public var minX: Double, minY: Double, maxX: Double, maxY: Double

        public init(minX: Double, minY: Double, maxX: Double, maxY: Double) {
            self.minX = minX
            self.minY = minY
            self.maxX = maxX
            self.maxY = maxY
        }
    }

    public struct Label: Equatable, Sendable {
        /// The relation's id, or nil for a target's aggregate line.
        public var id: String?
        public var target: String
        public var text: String
        /// The anchor point the text is drawn at.
        public var x: Double
        public var y: Double
        public var leading: Bool
        public var measured: Double?

        public var width: Double { (measured ?? Double(text.count) * GanttRelationLabels.characterWidth) + 4 }
        public var minX: Double { leading ? x : x - width }
        public var maxX: Double { leading ? x + width : x }
        public var minY: Double { y - GanttRelationLabels.lineHeight / 2 }
        public var maxY: Double { y + GanttRelationLabels.lineHeight / 2 }

        public func overlaps(_ other: Label) -> Bool {
            minX < other.maxX && other.minX < maxX && minY < other.maxY && other.minY < maxY
        }

        public func covers(_ obstacle: Obstacle) -> Bool {
            minX < obstacle.maxX && obstacle.minX < maxX && minY < obstacle.maxY && obstacle.minY < maxY
        }

        /// The same label moved sideways, as little as needed, so it lies within `bounds` (when it fits at all).
        func kept(within bounds: ClosedRange<Double>?) -> Label {
            guard let bounds else { return self }
            var moved = self
            if moved.maxX > bounds.upperBound { moved.x -= moved.maxX - bounds.upperBound }
            if moved.minX < bounds.lowerBound { moved.x += bounds.lowerBound - moved.minX }
            return moved
        }
    }

    /// The label of one relation, from the relation's own words: kind, lead or lag, policy, whether it is a link of
    /// the critical path, and where it comes from.
    public static func text(_ relation: GanttRelation, from key: String, critical: Bool = false) -> String {
        "\(relation.abbreviation) \(GanttRelation.lagWords(relation.lagHours, basis: relation.lagBasis)) · \(relation.policy)\(critical ? " · critical path" : "") · from \(key)"
    }

    /// The line of a target whose incoming relations do not all get a label.
    public static func more(_ count: Int) -> String {
        "+\(count) more incoming relation\(count == 1 ? "" : "s"): every one is in the row text and Detail"
    }

    /// A generous width per character of the 10-point label font (the caption style), so an estimated rectangle covers
    /// the drawn text.
    public static let characterWidth = 6.2
    /// One label line, its plate included: the caption font's line with room so neighbouring plates never touch.
    public static let lineHeight = 15.0
    /// The first line sits this far above the arrowhead; each further line one `lineHeight` higher.
    public static let firstLine = 9.0
    /// At most this many lines per target; with more incoming relations the last line is the aggregate.
    public static let perTarget = 4
    /// How many lines above its arrowhead a label may move to find room.
    public static let lines = 6

    /// The labels to draw, none overlapping another and each within `bounds` (the chart's width) when given. A label
    /// takes the first line that covers neither another label nor an obstacle; failing that, the first line that only
    /// avoids other labels. Targets are taken top to bottom (then left to right, then by identity), and a target's
    /// relations by their text, then id, so the input order does not matter.
    public static func place(_ requests: [Request], bounds: ClosedRange<Double>? = nil, obstacles: [Obstacle] = []) -> [Label] {
        var groups: [String: [Request]] = [:]
        for request in requests { groups[request.target, default: []].append(request) }
        let ordered = groups.values.map { group in group.sorted { ($0.text, $0.id) < ($1.text, $1.id) } }.sorted { left, right in
            let a = left[0], b = right[0]
            return (a.y, a.x, a.target) < (b.y, b.x, b.target)
        }
        var placed: [Label] = []
        for group in ordered {
            var wanted = group.map { Label(id: $0.id, target: $0.target, text: $0.text, x: $0.x, y: $0.y, leading: $0.leading, measured: $0.measured) }
            if wanted.count > perTarget {
                let hidden = wanted.count - (perTarget - 1)
                let at = wanted[perTarget - 1]
                wanted = Array(wanted.prefix(perTarget - 1)) + [Label(id: nil, target: at.target, text: more(hidden), x: at.x, y: at.y, leading: at.leading)]
            }
            for label in wanted {
                let lift = { (offset: Double) -> Label in
                    var candidate = label
                    candidate.y = label.y + offset
                    return candidate.kept(within: bounds)
                }
                let above = (0..<lines).map { lift(-firstLine - Double($0) * lineHeight) }
                // Below the arrowhead too, but only to keep a bar visible: the lines above are preferred otherwise.
                let below = (0..<lines).map { lift(firstLine + Double($0) * lineHeight) }
                let free = { (candidate: Label) in !placed.contains(where: { $0.overlaps(candidate) }) }
                if let best = (above + below).first(where: { free($0) && !obstacles.contains(where: $0.covers) }) ?? above.first(where: free) {
                    placed.append(best)
                }
            }
        }
        return placed
    }
}
