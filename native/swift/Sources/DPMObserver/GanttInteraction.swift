// The Gantt timeline under the pointer, view-only: a click on a bar, a milestone or anywhere in a row's band
// selects that row's task (the row is the DISPLAYED one at the current scroll offset), a double click opens
// Detail, the pointer resting on a row shows a temporary inspection (never a selection), and a drag or a
// scroll pans the view. Nothing here moves, resizes or reschedules a plan item.

import AppKit
import DPMObserverCore
import SwiftUI

struct GanttTimelinePointer: ViewModifier {
    @ObservedObject var model: ObserverModel
    let log: GanttPointerLog
    /// Where the pointer rests on the timeline (its own coordinates), not the row it was over: the row inspected is
    /// worked out again from the current scroll offset, zoom and shown rows on every draw, so a pan, a zoom or a
    /// filter under a stationary pointer describes the row now under it, or none. A resize clears it.
    @State private var resting: CGPoint?
    @State private var dragged = CGSize.zero
    /// While editing, a drag that began on a bar: a link being drawn or a finish being moved.
    @State private var editDrag: GanttEditDrag?

    func body(content: Content) -> some View {
        let hover = resting.flatMap { row(at: $0) }
        content
            .contentShape(Rectangle())
            .gesture(SpatialTapGesture(count: 1).onEnded { tap($0.location, open: GanttClicks.isDouble) })
            .simultaneousGesture(
                DragGesture(minimumDistance: 4)
                    .onChanged { value in
                        if dragged == .zero, editDrag == nil, model.editing.enabled { editDrag = GanttEditDrag.begin(at: value.startLocation, model: model) }
                        if var drag = editDrag {
                            drag.point = value.location
                            editDrag = drag
                            dragged = value.translation
                            return
                        }
                        let dx = value.translation.width - dragged.width, dy = value.translation.height - dragged.height
                        dragged = value.translation
                        model.pan(dx: -dx, dy: -dy)
                    }
                    .onEnded { value in
                        dragged = .zero
                        if let drag = editDrag {
                            editDrag = nil
                            let proposed = drag.finish(at: value.location, model: model)
                            log.note("edit_drag", ["proposed": proposed ?? NSNull()])
                            return
                        }
                        log.note("drag_end", ["pan": [model.gantt.panX, model.gantt.panY]])
                    }
            )
            .overlay { if let editDrag { GanttEditDragView(drag: editDrag, model: model) } }
            .onContinuousHover { phase in
                switch phase {
                case .active(let point): resting = point
                case .ended: resting = nil
                }
            }
            // Any change of the timeline's actual size, a height change of less than one row included (the row count and
            // the model's viewport need not change), clears the resting point: it was measured against the old size.
            .onGeometryChange(for: CGSize.self) { $0.size } action: { size in
                guard resting != nil else { return }
                resting = nil
                log.note("hover_cleared", ["reason": "timeline_resized", "size": [size.width, size.height]])
            }
            .onChange(of: hover?.id) { _, now in
                log.note("hover", ["hit": hover?.row.key ?? NSNull(), "row": now ?? NSNull(), "pan": [model.gantt.panX, model.gantt.panY]])
            }
            .overlay(alignment: .bottomLeading) {
                if let hover {
                    Text("\(hover.row.key): \(hover.row.title)")
                        .font(.caption).lineLimit(2)
                        .padding(6)
                        .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 6))
                        .padding(8)
                        .allowsHitTesting(false)
                }
            }
    }

    private func row(at point: CGPoint) -> GanttOutlineRow? {
        point.y >= GanttGeometry.axis ? model.ganttRow(atY: point.y - GanttGeometry.axis) : nil
    }

    private func tap(_ point: CGPoint, open: Bool) {
        let row = row(at: point)
        if let row {
            // The region takes the keyboard for the row, as a row label's click does; the selection is shared.
            model.focusRow(row.id)
            if open { model.perform(.openDetail) }
        }
        log.note(open ? "double_click" : "click", ["point": [point.x, point.y], "hit": row?.row.key ?? NSNull(), "milestone": row?.row.isMilestone ?? false,
                                                  "selected": row != nil, "opened_detail": row != nil && open])
    }
}

/// The receipts of the pointer events one Gantt body's timeline really received, bounded. One per concrete body,
/// reported through the host and for the window of the region that registered it; inert without a host.
@MainActor final class GanttPointerLog {
    private weak var host: HostObservation?
    private weak var region: NSView?
    private var events: [[String: Any]] = []
    private var sequence = 0
    static let limit = 24

    func attach(host: HostObservation, region: NSView) {
        self.host = host
        self.region = region
    }

    func note(_ event: String, _ facts: [String: Any]) {
        guard let host, region?.window != nil else { return }
        sequence &+= 1
        var entry = facts
        entry["seq"] = sequence
        entry["event"] = event
        entry["uptime"] = ProcessInfo.processInfo.systemUptime
        events.append(entry)
        if events.count > Self.limit { events.removeFirst(events.count - Self.limit) }
        host.noteChange()
    }

    /// Nil once the region left its window: the host then drops the sampler.
    func traced() -> [String: Any]? {
        guard let window = region?.window else { return nil }
        return ["window": window.windowNumber, "events": events, "sequence": sequence, "limit": Self.limit]
    }
}

/// A click selects at once; the second click of a double click also opens Detail. A double-click gesture
/// given priority over the single click would hold every single click for the system's double-click
/// interval before selecting.
enum GanttClicks {
    @MainActor static var isDouble: Bool {
        // Only a mouse event has a click count (reading it from any other event raises); a press made by keyboard or
        // assistive technology is a single selection.
        guard let event = NSApp.currentEvent, [.leftMouseDown, .leftMouseUp].contains(event.type) else { return false }
        return event.clickCount >= 2
    }
}

/// A drag that began on a bar while editing. It never moves the bar: dates are derived. From the bar's body it
/// draws a dependency to the bar it is dropped on. From the finish edge of a task not yet started it proposes
/// the task's own three-point estimate scaled by how much longer or shorter the bar was dragged: a ratio of
/// lengths, not a conversion of points to hours, so calendars and remaining time stay the application's; the
/// review shows the exact hours before anything applies.
struct GanttEditDrag: Equatable {
    enum Mode: Equatable {
        case link(from: String, fromEnd: PlanEnd)
        case resize(work: String, left: CGFloat, right: CGFloat)
    }
    let mode: Mode
    let origin: CGPoint
    var point: CGPoint

    /// Where a bar of `row` is drawn on the timeline now, if it is a scheduled task or milestone.
    @MainActor static func bar(_ row: GanttOutlineRow, model: ObserverModel) -> (left: CGFloat, right: CGFloat, top: CGFloat)? {
        guard let span = row.row.span, !row.row.isPackage else { return nil }
        let (x, width) = GanttLayout.bar(span, scale: model.gantt.scale)
        let left = x - model.gantt.panX
        return (left, left + max(width, 3), GanttGeometry.axis + Double(row.position) * GanttLayout.rowHeight - model.gantt.panY)
    }

    @MainActor static func begin(at point: CGPoint, model: ObserverModel) -> GanttEditDrag? {
        guard point.y >= GanttGeometry.axis, let row = model.ganttRow(atY: point.y - GanttGeometry.axis) else { return nil }
        if row.row.isPackage {
            model.setEditNotice("A work package's span comes from its tasks: link or resize a task, or edit \(model.editName(row.id)) in the inspector.")
            return nil
        }
        guard let bar = bar(row, model: model), point.x >= bar.left - 6, point.x <= bar.right + 6 else { return nil }
        let unstarted = ["Proposed", "Planned", "Claimed"].contains(row.row.status)
        if abs(point.x - bar.right) <= 6, row.row.kind == "Task", unstarted, model.snapshot.inventory.byIdentity[row.id]?.estimate != nil,
           bar.right - bar.left > 6 {
            return GanttEditDrag(mode: .resize(work: row.id, left: bar.left, right: bar.right), origin: point, point: point)
        }
        let end: PlanEnd = point.x < bar.left + (bar.right - bar.left) / 3 ? .start : .finish
        return GanttEditDrag(mode: .link(from: row.id, fromEnd: end), origin: point, point: point)
    }

    /// Add the proposed edit to the draft; the words of what was proposed, or nil when the drop proposes nothing.
    @MainActor func finish(at point: CGPoint, model: ObserverModel) -> String? {
        switch mode {
        case let .link(from, fromEnd):
            guard point.y >= GanttGeometry.axis, let target = model.ganttRow(atY: point.y - GanttGeometry.axis), target.id != from,
                  let bar = Self.bar(target, model: model) else { return nil }
            let toEnd: PlanEnd = point.x < (bar.left + bar.right) / 2 ? .start : .finish
            let edit = PlanEdit.link(id: UUID().uuidString.lowercased(), from: from, fromEnd: fromEnd, to: target.id, toEnd: toEnd, lagHours: 0)
            model.edit(edit)
            return "\(PlanEdit.kind(from: fromEnd, to: toEnd)) \(model.editName(from))>\(model.editName(target.id))"
        case let .resize(work, left, right):
            let ratio = Self.ratio(left: left, right: right, to: point.x)
            guard ratio != 1, let current = model.snapshot.inventory.byIdentity[work]?.estimate else { return nil }
            model.edit(.estimate(work: work, optimistic: Self.quarter(current.optimisticHours * ratio), likely: Self.quarter(current.likelyHours * ratio),
                                 pessimistic: Self.quarter(current.pessimisticHours * ratio)))
            return "estimate \(model.editName(work)) x\(ratio)"
        }
    }

    /// How much longer the bar was dragged, as a ratio of lengths rounded to a twentieth, at least a tenth.
    static func ratio(left: CGFloat, right: CGFloat, to x: CGFloat) -> Double {
        let ratio = Double(max(x - left, 1) / max(right - left, 1))
        return max(0.1, (ratio * 20).rounded() / 20)
    }

    static func quarter(_ hours: Double) -> Double { (hours * 4).rounded() / 4 }
}

/// The drag being made: a line to the pointer for a link, the new finish for an estimate, and what a drop does.
struct GanttEditDragView: View {
    let drag: GanttEditDrag
    @ObservedObject var model: ObserverModel

    var body: some View {
        ZStack(alignment: .topLeading) {
            Canvas { context, _ in
                var path = Path()
                switch drag.mode {
                case .link:
                    path.move(to: drag.origin)
                    path.addLine(to: drag.point)
                    context.stroke(path, with: .color(Palette.relationEmphasis), style: StrokeStyle(lineWidth: 2, dash: [5, 3]))
                case .resize:
                    path.move(to: CGPoint(x: drag.point.x, y: drag.origin.y - 12))
                    path.addLine(to: CGPoint(x: drag.point.x, y: drag.origin.y + 12))
                    context.stroke(path, with: .color(Palette.relationEmphasis), lineWidth: 2)
                }
            }
            Text(hint).font(.caption).padding(5)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 5))
                .offset(x: drag.point.x + 12, y: drag.point.y + 12)
        }
        .allowsHitTesting(false)
    }

    private var hint: String {
        switch drag.mode {
        case let .link(from, fromEnd):
            return "Link from \(model.editName(from)) (\(fromEnd.rawValue)): drop on a bar's left half for its start, right half for its finish. Bars never move; dates are derived. Set a lag or lead on the link in the inspector."
        case let .resize(work, left, right):
            let ratio = GanttEditDrag.ratio(left: left, right: right, to: drag.point.x)
            return "Scale \(model.editName(work))'s three-point estimate by \(String(format: "%.2f", ratio))×; the review shows the exact hours"
        }
    }
}
