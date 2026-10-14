// The outline of a work package's summary bracket on the Gantt: a bar across the package's span whose ends point
// down at the work it holds. It is one simple polygon, never a bar plus end triangles: subpaths of opposite winding
// cancel where they overlap under the non-zero fill rule and notch an end. Pure view geometry.

import Foundation

public enum GanttBracket {
    /// The bar's thickness, and how far each end points down below the bar's top.
    public static let thickness = 6.0
    public static let reach = 13.0
    /// How far each end's point runs inward along the bar; a short package's ends meet in its middle and never cross.
    public static let inward = 7.0

    /// The bracket's corners in drawing order, from the top left clockwise on screen, for a span from `left` to
    /// `right` with the bar's top at `top`.
    public static func outline(left: Double, right: Double, top: Double) -> [(x: Double, y: Double)] {
        let inset = min(inward, max(right - left, 0) / 2)
        return [(left, top), (right, top), (right, top + reach), (right - inset, top + thickness),
                (left + inset, top + thickness), (left, top + reach)]
    }
}
