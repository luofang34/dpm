// Where the network's nodes and edge labels go on the canvas. Pure geometry over the graph as decoded:
// a column, row or label position never changes a relation, its words or what gates what.

import Foundation

/// Where the canvas puts the short edge labels (FS, FF + 2 h, Soft): each at the first spot along its edge whose
/// rectangle touches no node and no label already placed, the edges of the cursor node first. A label with no free
/// spot is left off the canvas and counted; every relation stays complete in the text list and the inspector.
/// Presentation only: a label's words are its relation's, unchanged.
public enum NetworkLabels {
    public struct Candidate: Sendable {
        public let edge: String
        public let text: String
        public let from: NetworkPoint
        public let to: NetworkPoint
        public let priority: Bool

        public init(edge: String, text: String, from: NetworkPoint, to: NetworkPoint, priority: Bool) {
            self.edge = edge
            self.text = text
            self.from = from
            self.to = to
            self.priority = priority
        }
    }

    public struct Placed: Equatable, Sendable {
        public let edge: String
        public let text: String
        /// The label's rectangle at zoom 1, in canvas points; its centre is where the text is drawn.
        public let x: Double, y: Double, width: Double, height: Double

        public func touches(_ r: (x: Double, y: Double, width: Double, height: Double)) -> Bool {
            x < r.x + r.width && r.x < x + width && y < r.y + r.height && r.y < y + height
        }
    }

    static let fractions = [0.5, 0.36, 0.64, 0.24, 0.76]
    static let lifts = [-7.0, 7.0]
    /// At most this many labels are tried in one draw; the rest are counted as left off.
    public static let limit = 400

    public static func size(_ text: String) -> (width: Double, height: Double) { (Double(text.count) * 5.2 + 6, 12) }

    public static func place(_ candidates: [Candidate], nodes: [(x: Double, y: Double, width: Double, height: Double)]) -> (placed: [Placed], omitted: Int) {
        let ordered = candidates.enumerated().sorted { a, b in a.element.priority != b.element.priority ? a.element.priority : a.offset < b.offset }.map(\.element)
        var placed: [Placed] = []
        var omitted = 0
        for (index, label) in ordered.enumerated() {
            guard index < limit else { omitted += 1; continue }
            let size = size(label.text)
            var spot: Placed?
            search: for fraction in fractions {
                for lift in lifts {
                    let cx = label.from.x + (label.to.x - label.from.x) * fraction, cy = label.from.y + (label.to.y - label.from.y) * fraction + lift
                    let candidate = Placed(edge: label.edge, text: label.text, x: cx - size.width / 2, y: cy - size.height / 2, width: size.width, height: size.height)
                    if !nodes.contains(where: { candidate.touches($0) }), !placed.contains(where: { candidate.touches(($0.x, $0.y, $0.width, $0.height)) }) {
                        spot = candidate
                        break search
                    }
                }
            }
            if let spot { placed.append(spot) } else { omitted += 1 }
        }
        return (placed, omitted)
    }
}

public enum NetworkLayout {
    public static let nodeWidth = 180.0
    public static let nodeHeight = 44.0
    public static let columnGap = 70.0
    public static let rowGap = 22.0
    public static let zoomSteps: [Double] = [0.25, 0.4, 0.6, 0.8, 1, 1.25, 1.6]
    public static let defaultZoom = 4

    public static func scale(_ level: Int) -> Double { zoomSteps[max(0, min(zoomSteps.count - 1, level))] }

    /// The node's rectangle at zoom 1, in points from the canvas origin.
    public static func origin(column: Int, row: Int) -> (x: Double, y: Double) {
        (Double(column) * (nodeWidth + columnGap) + 20, Double(row) * (nodeHeight + rowGap) + 20)
    }

    /// The whole drawing at zoom 1: every node and every edge route with its end mark, plus the margin.
    public static func extent(_ graph: NetworkGraph) -> (width: Double, height: Double) {
        (max(Double(graph.columns) * (nodeWidth + columnGap) + 40, graph.routeExtent.x + 20),
         max(Double(graph.tallest) * (nodeHeight + rowGap) + 40, graph.routeExtent.y + 20))
    }

    /// The node's rectangle at zoom 1.
    public static func rect(_ place: NetworkPlace) -> (x: Double, y: Double, width: Double, height: Double) {
        let at = origin(column: place.column, row: place.row)
        return (at.x, at.y, nodeWidth, nodeHeight)
    }

    /// The largest pan along each axis: the scaled content beyond the viewport, never below 0.
    public static func maxPan(_ graph: NetworkGraph, level: Int, viewport: (width: Double, height: Double)) -> (x: Double, y: Double) {
        maxPan(graph, scale: scale(level), viewport: viewport)
    }

    public static func maxPan(_ graph: NetworkGraph, scale: Double, viewport: (width: Double, height: Double)) -> (x: Double, y: Double) {
        let size = extent(graph)
        return (max(0, size.width * scale - viewport.width), max(0, size.height * scale - viewport.height))
    }

    /// The scale at which the whole extent, margins included, fits the canvas along both axes: the smaller of the two
    /// ratios, never above the largest zoom step. Nil without a measured canvas. Geometry only.
    public static func fitScale(_ graph: NetworkGraph, viewport: (width: Double, height: Double)) -> Double? {
        let size = extent(graph)
        guard viewport.width > 0, viewport.height > 0, size.width > 0, size.height > 0 else { return nil }
        return min(zoomSteps[zoomSteps.count - 1], viewport.width / size.width, viewport.height / size.height)
    }

    /// The node drawn at a point of the canvas (canvas coordinates, top left), if any: geometry only.
    public static func hit(_ graph: NetworkGraph, shown: [NetworkNode], at point: (x: Double, y: Double), pan: (x: Double, y: Double), level: Int) -> NetworkNode? {
        hit(graph, shown: shown, at: point, pan: pan, scale: scale(level))
    }

    public static func hit(_ graph: NetworkGraph, shown: [NetworkNode], at point: (x: Double, y: Double), pan: (x: Double, y: Double), scale: Double) -> NetworkNode? {
        let x = (point.x + pan.x) / scale, y = (point.y + pan.y) / scale
        return shown.first { node in
            guard let place = graph.place[node.identity] else { return false }
            let r = rect(place)
            return x >= r.x && x <= r.x + r.width && y >= r.y && y <= r.y + r.height
        }
    }
}
