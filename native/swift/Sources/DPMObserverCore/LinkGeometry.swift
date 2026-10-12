// Where a link being drafted is drawn, on the Gantt and on the network alike: an elbow connector out of the end it
// leaves and into the end it joins, and the draft's links on the network between the node ends they join. Geometry
// only: a drawn draft link is not part of the plan until the draft is applied.

import Foundation

/// The corner points of an elbow connector, in drawing order.
public enum ElbowRoute {
    /// From `from`, leaving outward (positive to the right), to `to`, entered inward from the side its end faces
    /// (negative from the left, 0 when there is no target yet), so the last run never crosses the target. Ends facing
    /// each other with room between turn once; ends facing the same way turn beyond both; a link running back turns
    /// between the rows.
    public static func points(from: NetworkPoint, outward: Double, to: NetworkPoint, inward: Double) -> [NetworkPoint] {
        let out = from.x + outward, entry = to.x + inward
        var points = [from]
        let facing = (outward > 0 && inward < 0 && entry >= out) || (outward < 0 && inward > 0 && entry <= out)
        let sameWay = (outward > 0 && inward > 0) || (outward < 0 && inward < 0) || inward == 0
        if facing {
            points += [NetworkPoint(x: entry, y: from.y), NetworkPoint(x: entry, y: to.y)]
        } else if sameWay {
            let turn = inward == 0 ? out : (outward > 0 ? max(out, entry) : min(out, entry))
            points += [NetworkPoint(x: turn, y: from.y), NetworkPoint(x: turn, y: to.y)]
            if inward != 0 { points.append(NetworkPoint(x: entry, y: to.y)) }
        } else {
            let between = (from.y + to.y) / 2
            points += [NetworkPoint(x: out, y: from.y), NetworkPoint(x: out, y: between), NetworkPoint(x: entry, y: between), NetworkPoint(x: entry, y: to.y)]
        }
        return points + [to]
    }

    /// How far a connector runs out of an end before it turns, and its direction for that end.
    public static let reach = 10.0
    public static func outward(_ end: PlanEnd) -> Double { end == .finish ? reach : -reach }
    public static func inward(_ end: PlanEnd) -> Double { end == .start ? -reach : reach }
}

/// One link of the draft as the network draws it, in graph points.
public struct NetworkDraftLink: Equatable, Sendable {
    public let from: String
    public let to: String
    /// `FinishStart`, `StartStart`, `FinishFinish` or `StartFinish`.
    public let kind: String
    public let points: [NetworkPoint]
}

extension ObserverModel {
    /// The middle of a node's start (left) or finish (right) edge, in graph points; nil when the node has no place.
    public func networkEnd(_ identity: String, _ end: PlanEnd) -> NetworkPoint? {
        guard let place = networkGraph.place[identity] else { return nil }
        let rect = NetworkLayout.rect(place)
        return NetworkPoint(x: end == .start ? rect.x : rect.x + rect.width, y: rect.y + rect.height / 2)
    }

    /// The draft's links whose two ends are both shown on the network, each routed from the end it leaves to the end
    /// it joins. A link with an end filtered out stays in the draft and is listed there, but is not drawn.
    public var networkDraftLinks: [NetworkDraftLink] {
        let shown = Set(networkNodes.map(\.identity))
        return (editing.draft?.edits ?? []).compactMap { edit in
            guard case let .link(_, from, fromEnd, to, toEnd, _) = edit, shown.contains(from), shown.contains(to),
                  let start = networkEnd(from, fromEnd), let tip = networkEnd(to, toEnd) else { return nil }
            return NetworkDraftLink(from: from, to: to, kind: PlanEdit.kind(from: fromEnd, to: toEnd),
                                    points: ElbowRoute.points(from: start, outward: ElbowRoute.outward(fromEnd), to: tip, inward: ElbowRoute.inward(toEnd)))
        }
    }
}
