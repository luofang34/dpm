// A row's recorded execution: a hatched bar from its recorded start to its finish, or to the clock reading while it
// is outstanding, left of the remaining bar. It takes the colour remaining work would have: critical when on the
// as-built critical path, planned otherwise, neutral for a package's span. Colour is never the only cue: the bar is
// hatched, the row's words say "recorded" and "as-built critical path". Drawing only; the hours are the projection's.

import DPMObserverCore
import SwiftUI

enum GanttHistoryDrawing {
    /// Draw `row`'s recorded stretch with its top at `top`; the x of its right end, or nil when nothing was recorded.
    static func draw(_ row: GanttRow, top: Double, scale: Double, offsetX: Double, in context: inout GraphicsContext) -> Double? {
        guard let stretch = row.actual?.hours else { return nil }
        let left = stretch.from * scale - offsetX
        let right = max(stretch.to * scale - offsetX, left + 3)
        let rect = CGRect(x: left, y: top + 9, width: right - left, height: GanttLayout.rowHeight - 18)
        let shape = Path(roundedRect: rect, cornerRadius: 3)
        let tone: Color = row.isPackage ? Palette.recorded : row.actual?.critical == true ? Palette.critical : Palette.planned
        context.fill(shape, with: .color(tone.opacity(0.35)))
        var hatch = context
        hatch.clip(to: shape)
        var lines = Path()
        var x = rect.minX - rect.height
        while x < rect.maxX {
            lines.move(to: CGPoint(x: x, y: rect.maxY))
            lines.addLine(to: CGPoint(x: x + rect.height, y: rect.minY))
            x += 5
        }
        hatch.stroke(lines, with: .color(tone), lineWidth: 1)
        context.stroke(shape, with: .color(tone), lineWidth: 1)
        return right
    }
}
