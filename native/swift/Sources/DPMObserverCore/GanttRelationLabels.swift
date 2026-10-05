// Where the Gantt's emphasised relation labels go on the chart. Several relations can end at one point of
// one row (every incoming relation of a milestone, say), so a label is never simply drawn at its arrowhead:
// each one takes the first line above its arrowhead that touches no label already placed. A target with
// more incoming relations than fit gets an aggregate line instead of the rest; a label that finds no free
// line is left off the chart. Nothing is lost: every relation stays in the row's text and in Detail.
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

        public init(id: String, target: String, text: String, x: Double, y: Double, leading: Bool) {
            self.id = id
            self.target = target
            self.text = text
            self.x = x
            self.y = y
            self.leading = leading
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

        public var width: Double { Double(text.count) * GanttRelationLabels.characterWidth + 4 }
        public var minX: Double { leading ? x : x - width }
        public var maxX: Double { leading ? x + width : x }
        public var minY: Double { y - GanttRelationLabels.lineHeight / 2 }
        public var maxY: Double { y + GanttRelationLabels.lineHeight / 2 }

        public func overlaps(_ other: Label) -> Bool {
            minX < other.maxX && other.minX < maxX && minY < other.maxY && other.minY < maxY
        }
    }

    /// The label of one relation, from the relation's own words: kind, lead or lag, policy and where it comes from.
    public static func text(_ relation: GanttRelation, from key: String) -> String {
        "\(relation.abbreviation) \(GanttRelation.lagWords(relation.lagHours, basis: relation.lagBasis)) · \(relation.policy) · from \(key)"
    }

    /// The line of a target whose incoming relations do not all get a label.
    public static func more(_ count: Int) -> String {
        "+\(count) more incoming relation\(count == 1 ? "" : "s"): every one is in the row text and Detail"
    }

    /// A generous width per character of the 9-point label font, so an estimated rectangle covers the drawn text.
    public static let characterWidth = 5.6
    public static let lineHeight = 12.0
    /// The first line sits this far above the arrowhead; each further line one `lineHeight` higher.
    public static let firstLine = 9.0
    /// At most this many lines per target; with more incoming relations the last line is the aggregate.
    public static let perTarget = 4
    /// How many lines above its arrowhead a label may move to find room.
    public static let lines = 6

    /// The labels to draw, none overlapping another. Targets are taken top to bottom (then left to right, then by
    /// identity), and a target's relations by their text, then id, so the input order does not matter.
    public static func place(_ requests: [Request]) -> [Label] {
        var groups: [String: [Request]] = [:]
        for request in requests { groups[request.target, default: []].append(request) }
        let ordered = groups.values.map { group in group.sorted { ($0.text, $0.id) < ($1.text, $1.id) } }.sorted { left, right in
            let a = left[0], b = right[0]
            return (a.y, a.x, a.target) < (b.y, b.x, b.target)
        }
        var placed: [Label] = []
        for group in ordered {
            var wanted = group.map { Label(id: $0.id, target: $0.target, text: $0.text, x: $0.x, y: $0.y, leading: $0.leading) }
            if wanted.count > perTarget {
                let hidden = wanted.count - (perTarget - 1)
                let at = wanted[perTarget - 1]
                wanted = Array(wanted.prefix(perTarget - 1)) + [Label(id: nil, target: at.target, text: more(hidden), x: at.x, y: at.y, leading: at.leading)]
            }
            for label in wanted {
                for line in 0..<lines {
                    var candidate = label
                    candidate.y = label.y - firstLine - Double(line) * lineHeight
                    if !placed.contains(where: { $0.overlaps(candidate) }) {
                        placed.append(candidate)
                        break
                    }
                }
            }
        }
        return placed
    }
}
