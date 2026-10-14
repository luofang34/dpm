// GANTT-BRACKET-01: a work package's summary bracket is filled as one simple polygon, so no part of it cancels another
// under the non-zero rule and neither end is notched. Both ends point down the same distance, and a short package's
// ends never cross. This checks the shared outline the drawing fills; the pixels are the UI verifier's.

import DPMObserverCore
import Foundation

extension Context {
    func ganttPackageBracketIsOneSimpleOutline() async throws {
        print("gantt: a package bracket is one simple, consistently wound outline at every width, with both ends pointing down alike")
        typealias Point = (x: Double, y: Double)
        func crosses(_ a: Point, _ b: Point, _ c: Point, _ d: Point) -> Bool {
            func side(_ p: Point, _ q: Point, _ r: Point) -> Double { (q.x - p.x) * (r.y - p.y) - (q.y - p.y) * (r.x - p.x) }
            let (d1, d2, d3, d4) = (side(c, d, a), side(c, d, b), side(a, b, c), side(a, b, d))
            return ((d1 > 0 && d2 < 0) || (d1 < 0 && d2 > 0)) && ((d3 > 0 && d4 < 0) || (d3 < 0 && d4 > 0))
        }
        for width in [1.0, 5.0, 14.0, 15.0, 80.0, 600.0] {
            let corners = GanttBracket.outline(left: 40, right: 40 + width, top: 10)
            let n = corners.count
            let edges = (0..<n).map { (corners[$0], corners[($0 + 1) % n]) }
            let crossing = edges.indices.contains { i in edges.indices.contains { j in
                abs(i - j) > 1 && abs(i - j) != n - 1 && crosses(edges[i].0, edges[i].1, edges[j].0, edges[j].1)
            } }
            let area = (0..<n).reduce(0.0) { sum, i in sum + corners[i].x * corners[(i + 1) % n].y - corners[(i + 1) % n].x * corners[i].y } / 2
            // The bar alone covers width × thickness; the ends add to it, so a cancelled overlap would show as less.
            check(!crossing && area > 0 && area >= width * GanttBracket.thickness - 1e-9,
                  "a \(width) pt bracket is one simple outline wound one way (area \(area), bar \(width * GanttBracket.thickness)): \(corners)")
            let tips = corners.filter { $0.y == 10 + GanttBracket.reach }
            check(tips.count == 2 && tips.contains { $0.x == 40 } && tips.contains { $0.x == 40 + width },
                  "both ends of a \(width) pt bracket point down \(GanttBracket.reach) pt at its edges: \(tips)")
        }
    }
}
