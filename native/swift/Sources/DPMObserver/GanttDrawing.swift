// The Gantt as values and as drawing. A `GanttFrame` is everything one drawing needs, copied out of the
// model, so the window and the PNG renderer draw exactly the same thing. Nothing is derived here: a bar
// is the row's `span`, float and criticality are the projection's, a relation is an edge of the plan,
// and the only date-like value drawn is the instant the projection's hours are counted from.

import DPMObserverCore
import SwiftUI

func hoursText(_ value: Double) -> String { GanttWords.hours(value) }

func percentText(_ fraction: Double) -> String { GanttWords.percent(fraction) }

struct GanttFrame {
    var schedule: GanttSchedule?
    var outline: [GanttOutlineRow]
    var positions: [String: Int]
    var inventory: Inventory
    var state: GanttViewState
    /// The shared selection, by task identity; it may be a row that is not drawn.
    var selected: String?
    var connection: ConnectionState
    var current: Bool
    /// Whether the Gantt holds the keyboard now, so the cursor is drawn as focused.
    var keyboard: Bool
    /// Whether draw passes stamp the measurement log. Rendering to a file never does.
    var stamps: Bool

    @MainActor
    init(model: ObserverModel, keyboard: Bool, stamps: Bool = true) {
        let snapshot = model.snapshot
        schedule = snapshot.gantt
        outline = model.outline
        positions = model.outlinePositions
        inventory = snapshot.inventory
        state = model.gantt
        if case .work(let identity)? = model.selection { selected = identity } else { selected = nil }
        connection = snapshot.connection
        current = snapshot.freshness.current
        self.keyboard = keyboard
        self.stamps = stamps
    }

    var cursor: GanttOutlineRow? {
        guard let id = state.focus, let position = positions[id], position < outline.count else { return nil }
        return outline[position]
    }

    /// Whether what is drawn is the last reading, no longer current: marked, never discarded.
    var marked: String? {
        switch connection {
        case .reconnecting: return "CONNECTION LOST: the last schedule read is shown and is not current"
        case .sourceChanged: return "SOURCE CHANGED: the previous source's schedule is shown and is not current"
        case .failed: return "FAILED: the last schedule read is shown and is not current"
        default: return current ? nil : "NOT CURRENT: the plan changed or time moved on; this reading is being refreshed"
        }
    }
}

/// A scripted scroll as one draw placed it: what was requested and the offset the content was actually
/// placed at, with the extent and viewport that offset was worked out against.
struct DrivenScroll {
    let drive: GanttDrive
    let extent: Double
    let viewport: Double
    /// The offset the geometry placed the content at: what the draw stamp reports, never the request.
    let placed: Double
}

struct GanttGeometry {
    static let axis = 30.0
    static let labelWidth = 400.0
    let pointsPerHour: Double
    let zoomLevel: Int
    let offsetX: Double
    let offsetY: Double
    let firstRow: Int
    let rowsDrawn: Int
    /// The scripted scroll this draw carries out, if one is in force.
    let scroll: DrivenScroll?

    init(_ frame: GanttFrame, height: Double) {
        let state = frame.state
        zoomLevel = state.zoomLevel
        pointsPerHour = state.scale
        let rowCount = frame.outline.count
        // A scripted scroll request overrides one axis. The content is finite, so the distance it asks for
        // wraps over the scroll range; the offset the content is then placed at is the one this reports.
        var driven: DrivenScroll?
        if let drive = state.drive {
            let metrics = GanttLayout.scrollExtent(drive.axis, schedule: frame.schedule, rows: rowCount, state: state)
            let placed = ScrollGeometry.effectiveOffset(logical: drive.logicalDistance, extent: metrics.extent, viewport: metrics.viewport) + Measure.coordinateFault
            driven = DrivenScroll(drive: drive, extent: metrics.extent, viewport: metrics.viewport, placed: placed)
        }
        scroll = driven
        offsetX = driven.flatMap { $0.drive.axis == .horizontal ? $0.placed : nil } ?? state.panX
        offsetY = driven.flatMap { $0.drive.axis == .vertical ? $0.placed : nil } ?? state.panY
        // Never past the last row, however far the offset was asked to go (rows can shrink under it).
        firstRow = max(0, min(rowCount, Int(max(0, offsetY) / GanttLayout.rowHeight)))
        let visible = Int((height - Self.axis) / GanttLayout.rowHeight) + 2
        rowsDrawn = max(0, min(rowCount - firstRow, visible))
    }

    func y(of position: Int) -> Double { Self.axis + Double(position) * GanttLayout.rowHeight - offsetY }
}

// MARK: - Words

func kindWords(_ row: GanttRow) -> String { GanttWords.kind(row) }

/// What a row says to assistive technology: the full title, its kind and status, and everything else
/// the row shows, including each relation in words (the text is `GanttWords.row`, shared with the suite).
func speech(_ item: GanttOutlineRow, frame: GanttFrame) -> (label: String, value: String) {
    GanttWords.row(item, schedule: frame.schedule, inventory: frame.inventory, selected: frame.selected)
}

// MARK: - Drawing the timeline

enum GanttDrawing {
    static func draw(_ context: inout GraphicsContext, size: CGSize, frame: GanttFrame) {
        // The gate of the negative control: closed, this surface paints nothing new and stamps nothing.
        guard Measure.gateOpen("gantt") else { return }
        let geometry = GanttGeometry(frame, height: size.height)
        let rowHeight = GanttLayout.rowHeight
        let accent = Color.accentColor
        context.clip(to: Path(CGRect(origin: .zero, size: size)))
        guard let schedule = frame.schedule else {
            context.draw(Text("Reading the schedule from the application…").font(.callout), at: CGPoint(x: 16, y: 48), anchor: .leading)
            return
        }
        let rows = frame.outline
        let last = geometry.firstRow + geometry.rowsDrawn

        // Row bands, the selection and the cursor.
        for position in geometry.firstRow..<max(geometry.firstRow, last) {
            let item = rows[position]
            let rect = CGRect(x: 0, y: geometry.y(of: position), width: size.width, height: rowHeight)
            if position % 2 == 0 { context.fill(Path(rect), with: .color(.gray.opacity(0.07))) }
            if frame.selected == item.id { context.fill(Path(rect), with: .color(accent.opacity(0.16))) }
            if frame.state.focus == item.id {
                context.stroke(Path(rect.insetBy(dx: 1, dy: 1)), with: .color(accent), style: StrokeStyle(lineWidth: frame.keyboard ? 2 : 1, dash: frame.keyboard ? [] : [3, 2]))
            }
        }

        // The axis: elapsed hours from the instant the projection was evaluated at.
        let scale = geometry.pointsPerHour
        let steps: [Double] = [1, 2, 5, 10, 20, 50, 100, 200, 500, 1000, 2000, 5000, 10000, 20000, 50000]
        let step = steps.first { $0 * scale >= 64 } ?? 50000
        var tick = (geometry.offsetX / scale / step).rounded(.down) * step
        while tick * scale - geometry.offsetX < size.width {
            let x = tick * scale - geometry.offsetX
            if x >= 0 {
                var line = Path()
                line.move(to: CGPoint(x: x, y: GanttGeometry.axis - 6))
                line.addLine(to: CGPoint(x: x, y: size.height))
                context.stroke(line, with: .color(.gray.opacity(0.25)), lineWidth: 0.5)
                context.draw(Text(tick == tick.rounded() ? "\(Int(tick)) h" : hoursText(tick)).font(.system(size: 9)).foregroundColor(.secondary), at: CGPoint(x: x + 3, y: 8), anchor: .leading)
            }
            tick += step
        }
        var origin = Path()
        origin.addRect(CGRect(x: 0, y: GanttGeometry.axis - 1, width: size.width, height: 1))
        context.fill(origin, with: .color(.gray.opacity(0.5)))

        // The finish forecast, as the query gives it: the deterministic finish and p50, p80, p95.
        // The markers are lines; their texts share one line under the time axis labels, and a text that would
        // touch one already placed is left off the chart (the facts stay in the Schedule basis and Detail).
        var markers: [(hours: Double, text: String, dash: [CGFloat])] = [(schedule.projectFinishHours, "finish \(hoursText(schedule.projectFinishHours))", [])]
        if let spread = schedule.uncertainty {
            markers += [(spread.p50, "p50", [2, 3]), (spread.p80, "p80", [6, 3]), (spread.p95, "p95", [10, 2, 2, 2])]
        }
        for marker in markers {
            let x = marker.hours * scale - geometry.offsetX
            guard x >= 0, x <= size.width else { continue }
            var line = Path()
            line.move(to: CGPoint(x: x, y: GanttGeometry.axis - 14))
            line.addLine(to: CGPoint(x: x, y: size.height))
            context.stroke(line, with: .color(.orange.opacity(0.8)), style: StrokeStyle(lineWidth: 1, dash: marker.dash))
        }
        for label in GanttAxisLabels.place(markers.map { (x: $0.hours * scale - geometry.offsetX, text: $0.text) }, width: size.width) {
            context.draw(Text(label.text).font(.system(size: 9, weight: .medium)).foregroundColor(.orange), at: CGPoint(x: label.x, y: GanttAxisLabels.lineY), anchor: .leading)
        }

        // Relations, under the bars. Every drawn row's incoming relations are drawn lightly; those of
        // the selected row and of the cursor row are drawn strongly and labelled. The labels are placed
        // together, so several ending at one point never overprint, and drawn over the bars.
        let emphasised = Set([frame.selected, frame.state.focus].compactMap { $0 })
        var seen = Set<String>()
        var labels: [GanttRelationLabels.Request] = []
        func relation(_ edge: GanttRelation, strong: Bool) {
            guard seen.insert(edge.id).inserted, let successorAt = frame.positions[edge.successor] else { return }
            let successor = rows[successorAt].row
            guard let target = successor.span else { return }
            let atFinish = edge.kind == "FinishFinish" || edge.kind == "StartFinish"
            let to = CGPoint(x: (atFinish ? target.finish : target.start) * scale - geometry.offsetX, y: geometry.y(of: successorAt) + rowHeight / 2)
            let dash: [CGFloat] = edge.policy == "Soft" ? [4, 3] : []
            let color: Color = strong ? .indigo : .gray
            var path = Path()
            if let predecessorAt = frame.positions[edge.predecessor], let source = rows[predecessorAt].row.span {
                let fromFinish = edge.kind == "FinishStart" || edge.kind == "FinishFinish"
                let from = CGPoint(x: (fromFinish ? source.finish : source.start) * scale - geometry.offsetX, y: geometry.y(of: predecessorAt) + rowHeight / 2)
                let bend = max(from.x, to.x) + 10
                path.move(to: from)
                path.addLine(to: CGPoint(x: bend, y: from.y))
                path.addLine(to: CGPoint(x: bend, y: to.y))
                path.addLine(to: to)
            } else {
                // The other end is not drawn (collapsed, filtered, or not scheduled): a stub says so.
                path.move(to: CGPoint(x: to.x - 18, y: to.y))
                path.addLine(to: to)
            }
            context.stroke(path, with: .color(color.opacity(strong ? 0.95 : 0.4)), style: StrokeStyle(lineWidth: strong ? 1.6 : 0.8, dash: dash))
            var head = Path()
            head.move(to: to)
            head.addLine(to: CGPoint(x: to.x + (atFinish ? 6 : -6), y: to.y - 3.5))
            head.addLine(to: CGPoint(x: to.x + (atFinish ? 6 : -6), y: to.y + 3.5))
            head.closeSubpath()
            context.fill(head, with: .color(color.opacity(strong ? 0.95 : 0.4)))
            if strong {
                let names = frame.schedule?.row(edge.predecessor)?.key ?? "?"
                labels.append(GanttRelationLabels.Request(id: edge.id, target: edge.successor, text: GanttRelationLabels.text(edge, from: names),
                                                          x: to.x + (atFinish ? 10 : -10), y: to.y, leading: atFinish))
            }
        }
        for position in geometry.firstRow..<max(geometry.firstRow, last) {
            let id = rows[position].id
            for edge in frame.inventory.relations(of: id) where edge.successor == id { relation(edge, strong: emphasised.contains(id)) }
        }
        for id in emphasised {
            for edge in frame.inventory.relations(of: id) { relation(edge, strong: true) }
        }

        // The bars, milestones and float.
        for position in geometry.firstRow..<max(geometry.firstRow, last) {
            let item = rows[position]
            let row = item.row
            let top = geometry.y(of: position)
            let mid = top + rowHeight / 2
            guard let span = row.span else {
                context.draw(Text("not scheduled: \(row.applicability)").font(.system(size: 10).italic()).foregroundColor(.secondary), at: CGPoint(x: 8, y: mid), anchor: .leading)
                continue
            }
            let (x, width) = GanttLayout.bar(span, scale: geometry.pointsPerHour)
            let left = x - geometry.offsetX
            guard left + width > -400, left < size.width + 40 else { continue }
            let critical = span.critical
            let fill: Color = critical ? .red : accent
            if row.isMilestone {
                var diamond = Path()
                diamond.move(to: CGPoint(x: left, y: mid - 8))
                diamond.addLine(to: CGPoint(x: left + 8, y: mid))
                diamond.addLine(to: CGPoint(x: left, y: mid + 8))
                diamond.addLine(to: CGPoint(x: left - 8, y: mid))
                diamond.closeSubpath()
                context.fill(diamond, with: .color(fill.opacity(0.9)))
                context.stroke(diamond, with: .color(.primary), lineWidth: 1.5)
                context.draw(Text("◆ \(row.key) milestone, \(hoursText(span.start))").font(.system(size: 10, weight: .semibold)).foregroundColor(.primary), at: CGPoint(x: left + 12, y: mid), anchor: .leading)
                continue
            }
            if let times = row.times, times.totalFloat > 0 {
                let reach = times.latestFinish * scale - geometry.offsetX
                if reach > left + width {
                    let slack = CGRect(x: left + width, y: mid - 3, width: reach - left - width, height: 6)
                    context.stroke(Path(slack), with: .color(.gray), style: StrokeStyle(lineWidth: 1, dash: [3, 2]))
                }
            }
            if row.isPackage {
                // A work package spans its descendants: a bracket, not a bar.
                var bracket = Path()
                bracket.move(to: CGPoint(x: left, y: mid + 6))
                bracket.addLine(to: CGPoint(x: left, y: mid - 5))
                bracket.addLine(to: CGPoint(x: left + width, y: mid - 5))
                bracket.addLine(to: CGPoint(x: left + width, y: mid + 6))
                context.stroke(bracket, with: .color(fill), style: StrokeStyle(lineWidth: critical ? 3 : 2, lineCap: .butt))
            } else {
                let bar = CGRect(x: left, y: top + 6, width: width, height: rowHeight - 12)
                context.fill(Path(roundedRect: bar, cornerRadius: 3), with: .color(fill.opacity(critical ? 0.8 : 0.5)))
                context.stroke(Path(roundedRect: bar, cornerRadius: 3), with: .color(fill), lineWidth: critical ? 2.5 : 1)
            }
            let tag = "\(row.key)\(critical ? " · critical path" : "") · \(hoursText(span.hours))"
            context.draw(Text(tag).font(.system(size: 10)).foregroundColor(.primary), at: CGPoint(x: left + width + 6, y: mid), anchor: .leading)
        }

        // The emphasised relations' labels, each on its own line and on a plate, so what is under it does not cross it.
        for label in GanttRelationLabels.place(labels) {
            let plate = CGRect(x: label.minX - 2, y: label.minY, width: label.maxX - label.minX + 4, height: label.maxY - label.minY)
            context.fill(Path(roundedRect: plate, cornerRadius: 3), with: .style(.background.opacity(0.88)))
            context.draw(Text(label.text).font(.system(size: 9, weight: .semibold)).foregroundColor(.indigo), at: CGPoint(x: label.x, y: label.y), anchor: label.leading ? .leading : .trailing)
        }

        if let mark = frame.marked {
            context.draw(Text(mark).font(.system(size: 11, weight: .bold)).foregroundColor(.red), at: CGPoint(x: size.width - 8, y: 8), anchor: .trailing)
        }

        stamp(frame, geometry: geometry, schedule: schedule)
    }

    /// The draw pass reports what it drew. This is the app-owned draw the measurement record counts as a
    /// completed draw of the generation: a first draw, a view operation, a commit or a scroll frame.
    private static func stamp(_ frame: GanttFrame, geometry: GanttGeometry, schedule: GanttSchedule) {
        guard frame.stamps, let log = MeasureLog.shared else { return }
        let drawn = geometry.rowsDrawn
        log.stamp("first_draw", key: "first", gen: log.id, facts: [
            "connected": frame.connection == .connected ? "true" : "false",
            "model_rows_positive": schedule.rows.isEmpty ? "false" : "true",
            "viewport_rows_drawn": drawn >= 1 ? "true" : "false",
        ], surface: "gantt", detail: ["view": "gantt", "model_rows": schedule.rows.count, "outline_rows": frame.outline.count, "viewport_rows_drawn": drawn, "revision": schedule.revision.map { Int($0) } ?? -1])
        if let revision = schedule.revision {
            // The task this draw marks as selected, by key, read from what was drawn.
            let selected = frame.selected.map { frame.inventory.key(of: $0) ?? $0 } ?? "none"
            log.stamp("commit_drawn", key: "commit:\(revision)", gen: Int(revision), facts: ["revision": String(revision), "selected": selected], surface: "gantt",
                      detail: ["selected_kept": frame.selected != nil, "rows_drawn": drawn], mismatch: "commit_mismatch")
        }
        // A scripted scroll draws over the pan, so the geometry no longer shows the view state a view
        // operation set: only an undriven draw can say what that operation drew.
        if frame.state.drive == nil {
            let sequence = frame.state.sequence
            let shown = GanttFingerprint.facts(zoom: geometry.zoomLevel, offsetX: geometry.offsetX, offsetY: geometry.offsetY, filter: frame.state.filter, collapsed: frame.state.collapsed.count)
            log.stamp("view_op_drawn", key: "view_op:\(sequence)", gen: sequence, facts: shown.merging(["sequence": String(sequence), "rows": String(frame.outline.count)]) { first, _ in first },
                      surface: "gantt", detail: ["rows_drawn_model": frame.outline.count, "rows_drawn": drawn], mismatch: "view_op_mismatch")
        }
        if let scroll = geometry.scroll {
            let drive = scroll.drive
            // The offset is the one the geometry placed the content at, never the one requested.
            log.stamp("frame", key: drive.key, gen: drive.generation, facts: [
                "axis": drive.axis.rawValue, "window_id": String(drive.window), "seq": String(drive.sequence),
                "logical_distance": Measure.format(drive.logicalDistance), "extent": Measure.format(scroll.extent), "viewport": Measure.format(scroll.viewport),
                "offset": Measure.format(scroll.placed),
            ], surface: "gantt", detail: ["geometry_drawn_offset": scroll.placed, "rows_drawn": drawn], mismatch: "frame_mismatch")
        }
    }
}
