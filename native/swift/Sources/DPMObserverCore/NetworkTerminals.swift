// The Start and Finish markers of the network, as a precedence diagram draws them: one Start before the first column
// joined to every activity with no temporal predecessor, one Finish after the last column joined from every activity
// with no temporal successor, the Finish carrying the projection's finish and its percentiles. They are view markers
// only: not plan items, never selected or walked, and every fact they show is also in the text list. Decisions and
// work packages are not activities and join neither. The connectors use the column and row gaps, so none crosses a box.

import Foundation

public struct NetworkTerminals: Equatable, Sendable {
    public let start: NetworkPoint
    public let finish: NetworkPoint
    /// The activities joined to Start and to Finish, in plan order.
    public let starts: [String]
    public let finishes: [String]
    /// Connector routes, keyed `start>ID` and `ID>finish`.
    public let routes: [String: NetworkRoute]
    public let finishHours: Double?
    public let uncertainty: GanttUncertainty?

    init?(nodes: [NetworkNode], edges: [NetworkEdge], place: [String: NetworkPlace], columns: Int, schedule: GanttSchedule?) {
        let activities = nodes.filter { ($0.kind == .task || $0.kind == .milestone) && place[$0.identity] != nil }
        guard !activities.isEmpty else { return nil }
        let temporal = edges.filter { $0.kind == .temporal }
        let hasPredecessor = Set(temporal.map(\.to)), hasSuccessor = Set(temporal.map(\.from))
        starts = activities.filter { !hasPredecessor.contains($0.identity) }.map(\.identity)
        finishes = activities.filter { !hasSuccessor.contains($0.identity) }.map(\.identity)
        let lane = NetworkLayout.terminalLane, radius = NetworkLayout.terminalRadius
        let rowCentre = NetworkLayout.origin(column: 0, row: 0).y + NetworkLayout.nodeHeight / 2
        start = NetworkPoint(x: 20 + radius, y: rowCentre)
        let lastRight = NetworkLayout.origin(column: max(columns, 1) - 1, row: 0).x + NetworkLayout.nodeWidth
        finish = NetworkPoint(x: lastRight + lane - radius, y: rowCentre)
        finishHours = schedule?.projectFinishHours
        uncertainty = schedule?.uncertainty
        var routes: [String: NetworkRoute] = [:]
        let stub = 10.0
        let startSpine = start.x + radius + (lane - 2 * radius - 20) / 2
        for id in starts {
            guard let at = place[id] else { continue }
            let box = NetworkLayout.origin(column: at.column, row: at.row)
            let y = box.y + 10
            let from = NetworkPoint(x: start.x + radius, y: start.y)
            if at.column == 0 {
                routes["start>\(id)"] = NetworkRoute(points: [from, NetworkPoint(x: startSpine, y: from.y), NetworkPoint(x: startSpine, y: y), NetworkPoint(x: box.x, y: y)], detour: true)
            } else {
                let gap = NetworkRouting.gapY(at.row)
                routes["start>\(id)"] = NetworkRoute(points: [from, NetworkPoint(x: startSpine, y: from.y), NetworkPoint(x: startSpine, y: gap),
                                                               NetworkPoint(x: box.x - stub, y: gap), NetworkPoint(x: box.x - stub, y: y), NetworkPoint(x: box.x, y: y)], detour: true)
            }
        }
        let finishSpine = finish.x - radius - (lane - 2 * radius - 20) / 2
        for id in finishes {
            guard let at = place[id] else { continue }
            let box = NetworkLayout.origin(column: at.column, row: at.row)
            let right = box.x + NetworkLayout.nodeWidth, y = box.y + NetworkLayout.nodeHeight - 10
            let into = NetworkPoint(x: finish.x - radius, y: finish.y)
            if at.column == columns - 1 {
                routes["\(id)>finish"] = NetworkRoute(points: [NetworkPoint(x: right, y: y), NetworkPoint(x: finishSpine, y: y), NetworkPoint(x: finishSpine, y: into.y), into], detour: true)
            } else {
                let gap = NetworkRouting.gapY(at.row + 1)
                routes["\(id)>finish"] = NetworkRoute(points: [NetworkPoint(x: right, y: y), NetworkPoint(x: right + stub, y: y), NetworkPoint(x: right + stub, y: gap),
                                                                NetworkPoint(x: finishSpine, y: gap), NetworkPoint(x: finishSpine, y: into.y), into], detour: true)
            }
        }
        self.routes = routes
    }

    /// The Finish marker's words: the projection's finish and, when simulated, its percentiles.
    public var finishWords: String {
        let hours = { (value: Double) in "\(String(format: "%.1f", value)) h" }
        guard let finishHours else { return "finish not scheduled" }
        guard let spread = uncertainty else { return "finish \(hours(finishHours))" }
        return "finish \(hours(finishHours)) · P50 \(hours(spread.p50)) · P80 \(hours(spread.p80)) · P95 \(hours(spread.p95))"
    }

    public static func ==(left: NetworkTerminals, right: NetworkTerminals) -> Bool {
        left.starts == right.starts && left.finishes == right.finishes && left.routes == right.routes && left.finishHours == right.finishHours
    }
}
