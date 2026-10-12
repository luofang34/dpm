// How the network is being looked at: the filter, the zoom and pan, the cursor and the walk, and a link
// being dragged while editing. View state only; none of it is the plan, the selection or a schedule value.

import Foundation

// MARK: - View state

public struct NetworkFilter: Equatable, Sendable {
    public var text = ""
    /// The edge kinds drawn and listed; all three unless a person hides one.
    public var kinds: Set<NetworkEdgeKind> = Set(NetworkEdgeKind.allCases)

    public init() {}

    public var isActive: Bool { !text.trimmingCharacters(in: .whitespaces).isEmpty || kinds.count != NetworkEdgeKind.allCases.count }

    func matches(_ node: NetworkNode) -> Bool {
        let needle = text.trimmingCharacters(in: .whitespaces).lowercased()
        return needle.isEmpty || [node.key, node.title, node.status, node.kindWord].contains { $0.lowercased().contains(needle) }
    }

    public var words: String {
        var parts: [String] = []
        if !text.isEmpty { parts.append("text “\(text)”") }
        let hidden = NetworkEdgeKind.allCases.filter { !kinds.contains($0) }
        if !hidden.isEmpty { parts.append("hiding " + hidden.map(\.words).joined(separator: ", ")) }
        return parts.isEmpty ? "no filter" : parts.joined(separator: "; ")
    }
}

/// A dependency being drawn while editing: from the node pressed to where the pointer is, in canvas coordinates.
public struct NetworkLinkDrag: Equatable, Sendable {
    public let from: String
    public let origin: NetworkPoint
    public var point: NetworkPoint

    public init(from: String, origin: NetworkPoint, point: NetworkPoint) {
        self.from = from
        self.origin = origin
        self.point = point
    }
}

/// What a person changed about how the network is drawn. None of it is part of the project.
public struct NetworkViewState: Equatable, Sendable {
    public var filter = NetworkFilter()
    /// While editing, the link being drawn; view state only.
    public var linking: NetworkLinkDrag?
    public var zoomLevel = NetworkLayout.defaultZoom
    public var panX = 0.0
    public var panY = 0.0
    /// The node the keyboard cursor is on, by persistent identity.
    public var focus: String?
    /// Counts every applied view operation.
    public var sequence = 0
    /// Whether the filter field has the keyboard, so typed letters are text and not commands.
    public var editingFilter = false
    /// Where the walk keys last went, in words, for the region's accessibility value.
    public var walked: String?
    /// The walk in progress, so pressing its key again goes to the next node from the same origin.
    public var walk: NetworkWalk?
    /// The canvas size the window actually gives the graph (0 until measured): bounds the pan from both sides.
    public var viewportWidth = 0.0
    public var viewportHeight = 0.0
    /// Where the pointer rests on the canvas (canvas points from its top left), never a copied node: the node it
    /// inspects is hit again against the current pan, scale and shown nodes (`ObserverModel.networkHover`), so a pan,
    /// a zoom or a filter under a stationary pointer inspects what is under it now. A resize clears it.
    public var hoverPoint: NetworkPoint?
    /// The scale Fit derived from the canvas and the whole graph; while set it replaces the zoom step's scale.
    public var fitScale: Double?
    /// The honest note when Locate cannot show the selected node (the filter hides it); cleared by the next view change.
    public var locateNote: String?
    /// A request to bring the cursor node's row to the top of the text list, advanced (wrapping) only by an explicit
    /// Locate, so a Locate on the unchanged cursor node is seen too; never by a pan, a zoom, a filter or a refresh.
    public var listReveal = 0

    public init() {}

    /// The scale drawn now: the fitted one, else the zoom step's.
    public var scale: Double { fitScale ?? NetworkLayout.scale(zoomLevel) }
}

public struct NetworkPoint: Equatable, Sendable {
    public let x: Double
    public let y: Double

    public init(x: Double, y: Double) {
        self.x = x
        self.y = y
    }
}

public struct NetworkWalk: Equatable, Sendable {
    public let name: String
    public let origin: String
    public let targets: [String]
    public let index: Int
    public let target: String
}
