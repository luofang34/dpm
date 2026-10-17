// How the network canvas zooms and pans, modelled on a free canvas such as Freeform: the scale is continuous, a
// pinch or a ⌥⌘-scroll zooms about the pointer so the point under it stays put, the keys step through presets about
// the canvas centre, and the pan keeps the graph in reach (centred when it is smaller than the canvas, otherwise
// movable to each edge with some room) so it can be neither lost nor pinned to a corner. View state only.

import Foundation

extension NetworkLayout {
    /// The scale range a person can zoom through; a fit of a very large graph may go below it, never above it.
    public static let minScale = 0.1
    public static let maxScale = 4.0
    /// Actual size.
    public static let defaultScale = 1.0
    /// What the zoom keys and buttons step through.
    public static let zoomPresets: [Double] = [0.1, 0.25, 0.5, 0.75, 1, 1.5, 2, 3, 4]
    /// Below this the boxes are too small to read, so Locate returns to actual size.
    public static let readableScale = 0.75
    /// A fit never enlarges a small graph past this, so one or two nodes are not blown up to fill the window.
    public static let fitCeiling = 1.6
    /// Room the pan allows past each edge of a graph larger than the canvas.
    public static let panMargin = 120.0

    /// The next preset above `scale`, or the largest.
    public static func preset(above scale: Double) -> Double {
        zoomPresets.first { $0 > scale + 1e-9 } ?? zoomPresets[zoomPresets.count - 1]
    }

    /// The next preset below `scale`, or the smallest.
    public static func preset(below scale: Double) -> Double {
        zoomPresets.last { $0 < scale - 1e-9 } ?? zoomPresets[0]
    }

    /// `proposed` within the zoom range; a scale already below the range (a fit of a huge graph) may stay where it is
    /// but never shrinks further.
    public static func clampScale(_ proposed: Double, from current: Double) -> Double {
        min(maxScale, max(min(minScale, current), proposed))
    }

    /// The pan range along each axis at `scale`: a single centred value when the scaled graph is smaller than the
    /// canvas, otherwise from a margin before its leading edge to a margin past its trailing edge.
    public static func panBounds(_ graph: NetworkGraph, scale: Double, viewport: (width: Double, height: Double)) -> (x: ClosedRange<Double>, y: ClosedRange<Double>) {
        let size = extent(graph)
        return (axisBounds(content: size.width * scale, viewport: viewport.width), axisBounds(content: size.height * scale, viewport: viewport.height))
    }

    public static func axisBounds(content: Double, viewport: Double) -> ClosedRange<Double> {
        // Before the canvas is measured, the pan may cover the whole graph, as an unbounded window would.
        guard viewport > 0 else { return 0...max(0, content) }
        // Within half a point counts as fitting, so a fitted axis (filled up to rounding) is centred, not given room.
        if content <= viewport + 0.5 {
            let centred = -(viewport - content) / 2
            return centred...centred
        }
        let margin = min(panMargin, viewport / 3)
        return -margin...(content - viewport + margin)
    }

    public static func clampPan(_ pan: (x: Double, y: Double), within bounds: (x: ClosedRange<Double>, y: ClosedRange<Double>)) -> (x: Double, y: Double) {
        (min(bounds.x.upperBound, max(bounds.x.lowerBound, pan.x)), min(bounds.y.upperBound, max(bounds.y.lowerBound, pan.y)))
    }

    /// The pan that keeps the graph point under `anchor` (canvas points from the top left) under it when the scale
    /// changes from `old` to `new`: that point is `(pan + anchor) / scale` before and after.
    public static func anchoredPan(_ pan: (x: Double, y: Double), from old: Double, to new: Double, at anchor: (x: Double, y: Double)) -> (x: Double, y: Double) {
        guard old > 0 else { return pan }
        return ((pan.x + anchor.x) * new / old - anchor.x, (pan.y + anchor.y) * new / old - anchor.y)
    }
}

extension ObserverModel {
    /// The pan range of the current graph at a scale, in the measured canvas.
    func networkPanBounds(scale: Double) -> (x: ClosedRange<Double>, y: ClosedRange<Double>) {
        NetworkLayout.panBounds(networkGraph, scale: scale, viewport: (network.viewportWidth, network.viewportHeight))
    }

    /// Zoom continuously by `factor` about a canvas point (a pinch or a ⌥⌘-scroll under the pointer), within the zoom
    /// range; the graph point under the anchor stays there unless the pan bounds must move it.
    public func zoomNetwork(by factor: Double, at anchor: NetworkPoint) {
        guard factor.isFinite, factor > 0 else { return }
        let graph = networkGraph
        networkOperation { state in
            let old = state.scale, new = NetworkLayout.clampScale(old * factor, from: old)
            guard abs(new - old) > 1e-12 else { return }
            Self.applyZoom(&state, graph: graph, to: new, at: (anchor.x, anchor.y))
        }
    }

    /// Step to the next preset in a direction about the canvas centre, as the zoom keys and buttons do.
    func stepNetworkZoom(in: Bool) {
        let graph = networkGraph
        networkOperation { state in
            let new = `in` ? NetworkLayout.preset(above: state.scale) : NetworkLayout.preset(below: state.scale)
            Self.applyZoom(&state, graph: graph, to: new, at: (state.viewportWidth / 2, state.viewportHeight / 2))
        }
    }

    static func applyZoom(_ state: inout NetworkViewState, graph: NetworkGraph, to scale: Double, at anchor: (x: Double, y: Double)) {
        let pan = NetworkLayout.anchoredPan((state.panX, state.panY), from: state.scale, to: scale, at: anchor)
        let bounds = NetworkLayout.panBounds(graph, scale: scale, viewport: (state.viewportWidth, state.viewportHeight))
        (state.panX, state.panY) = NetworkLayout.clampPan(pan, within: bounds)
        state.scale = scale
        state.fitted = false
    }
}
