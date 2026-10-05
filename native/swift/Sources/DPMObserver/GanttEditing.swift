// Editing gestures on the Gantt timeline. Bars never move: dates are derived. Each gesture proposes an edit to the
// draft, or for progress sends the owner's execution report, and the chart shows what the draft would add.
//
// - Either end of a bar is a link handle: dragging from it draws an elbow connector, and dropping it on another bar's
//   left or right half joins that bar's start or finish, which names FS, SS, FF or SF.
// - Dragging the body of a task not yet started draws a link from its finish.
// - Dragging the body of a started task sets its reported progress, as a share of the bar, in steps of 5%.
// - Option-dragging the finish end of a task not yet started scales its own three-point estimate by the ratio of the
//   new bar length to the old; the review shows the exact hours.

import AppKit
import DPMObserverCore
import SwiftUI

struct GanttEditDrag: Equatable {
    enum Mode: Equatable {
        case link(from: String, fromEnd: PlanEnd)
        case resize(work: String, left: CGFloat, right: CGFloat)
        case progress(work: String, left: CGFloat, right: CGFloat)
    }
    let mode: Mode
    let origin: CGPoint
    var point: CGPoint

    static let handle: CGFloat = 7

    /// Where a bar of `row` is drawn on the timeline now, if it is a scheduled task or milestone.
    @MainActor static func bar(_ row: GanttOutlineRow, model: ObserverModel) -> (left: CGFloat, right: CGFloat, top: CGFloat)? {
        guard let span = row.row.span, !row.row.isPackage else { return nil }
        let top = GanttGeometry.axis + Double(row.position) * GanttLayout.rowHeight - model.gantt.panY
        // Verified work has no remaining bar; its mark at the origin is still a link end.
        if row.row.status == "Verified" { return (6 - model.gantt.panX, 12 - model.gantt.panX, top) }
        let (x, width) = GanttLayout.bar(span, scale: model.gantt.scale)
        let left = x - model.gantt.panX
        let half: CGFloat = row.row.isMilestone ? 8 : 0
        return (left - half, left + max(width, 3) + half, top)
    }

    /// The bar of the row with `identity`, if it is shown.
    @MainActor static func bar(of identity: String, model: ObserverModel) -> (left: CGFloat, right: CGFloat, mid: CGFloat)? {
        guard let position = model.outlinePositions[identity], position < model.outline.count,
              let bar = bar(model.outline[position], model: model) else { return nil }
        return (bar.left, bar.right, bar.top + GanttLayout.rowHeight / 2)
    }

    @MainActor static func begin(at point: CGPoint, model: ObserverModel) -> GanttEditDrag? {
        guard point.y >= GanttGeometry.axis, let row = model.ganttRow(atY: point.y - GanttGeometry.axis) else { return nil }
        if row.row.isPackage {
            model.setEditNotice("A work package's span comes from its tasks: link or change a task, or edit \(model.editName(row.id)) in the inspector.")
            return nil
        }
        guard let bar = bar(row, model: model), point.x >= bar.left - handle, point.x <= bar.right + handle else { return nil }
        let mid = bar.top + GanttLayout.rowHeight / 2
        let unstarted = ["Proposed", "Planned", "Claimed"].contains(row.row.status)
        // On a bar too narrow for two handles, the side of its middle pressed names the end.
        let narrow = bar.right - bar.left < 2 * handle
        let atStart = narrow ? point.x < (bar.left + bar.right) / 2 : abs(point.x - bar.left) <= handle
        let atFinish = narrow ? !atStart : abs(point.x - bar.right) <= handle
        if atFinish, NSEvent.modifierFlags.contains(.option) {
            guard row.row.kind == "Task", unstarted, model.snapshot.inventory.byIdentity[row.id]?.estimate != nil, bar.right - bar.left >= 12 else {
                model.setEditNotice("Only a task not yet started, with an estimate and a bar wide enough to drag, can have its estimate dragged; use the inspector.")
                return nil
            }
            return GanttEditDrag(mode: .resize(work: row.id, left: bar.left, right: bar.right), origin: point, point: point)
        }
        if atStart { return GanttEditDrag(mode: .link(from: row.id, fromEnd: .start), origin: CGPoint(x: bar.left, y: mid), point: point) }
        if atFinish { return GanttEditDrag(mode: .link(from: row.id, fromEnd: .finish), origin: CGPoint(x: bar.right, y: mid), point: point) }
        if row.row.status == "InProgress" {
            // Progress moves only from its own handle, the edge of the reported share, so a press elsewhere on the bar
            // never records a report.
            let reported = CGFloat(model.snapshot.inventory.byIdentity[row.id]?.reportedProgress ?? 0) / 100
            let edge = bar.left + (bar.right - bar.left) * reported
            guard abs(point.x - edge) <= handle else { return nil }
            return GanttEditDrag(mode: .progress(work: row.id, left: bar.left, right: bar.right), origin: CGPoint(x: edge, y: mid), point: point)
        }
        return GanttEditDrag(mode: .link(from: row.id, fromEnd: .finish), origin: CGPoint(x: bar.right, y: mid), point: point)
    }

    /// The bar and end a link dropped at `point` would join, if any.
    @MainActor static func target(at point: CGPoint, excluding from: String, model: ObserverModel) -> (id: String, end: PlanEnd, x: CGFloat, mid: CGFloat)? {
        guard point.y >= GanttGeometry.axis, let row = model.ganttRow(atY: point.y - GanttGeometry.axis), row.id != from,
              let bar = bar(row, model: model) else { return nil }
        let end: PlanEnd = point.x < (bar.left + bar.right) / 2 ? .start : .finish
        return (row.id, end, end == .start ? bar.left : bar.right, bar.top + GanttLayout.rowHeight / 2)
    }

    /// Add the proposed edit to the draft, or send the progress report; the words of what was done, or nil.
    @MainActor func finish(at point: CGPoint, model: ObserverModel) -> String? {
        switch mode {
        case let .link(from, fromEnd):
            if point.y >= GanttGeometry.axis, let row = model.ganttRow(atY: point.y - GanttGeometry.axis), row.row.isPackage {
                model.setEditNotice("A link joins tasks and milestones; \(model.editName(row.id)) is a work package.")
                return nil
            }
            guard let target = Self.target(at: point, excluding: from, model: model) else { return nil }
            model.edit(.link(id: UUID().uuidString.lowercased(), from: from, fromEnd: fromEnd, to: target.id, toEnd: target.end, lagHours: 0))
            return "\(PlanEdit.kind(from: fromEnd, to: target.end)) \(model.editName(from))>\(model.editName(target.id))"
        case let .resize(work, left, right):
            let ratio = Self.ratio(left: left, right: right, to: point.x)
            guard ratio != 1, let current = model.snapshot.inventory.byIdentity[work]?.estimate else { return nil }
            model.edit(.estimate(work: work, optimistic: Self.quarter(current.optimisticHours * ratio), likely: Self.quarter(current.likelyHours * ratio),
                                 pessimistic: Self.quarter(current.pessimisticHours * ratio)))
            return "estimate \(model.editName(work)) x\(ratio)"
        case let .progress(work, left, right):
            // Released off its row, or at the share already reported, the drag reports nothing.
            guard point.y >= GanttGeometry.axis, model.ganttRow(atY: point.y - GanttGeometry.axis)?.id == work else { return nil }
            let percent = Self.percent(left: left, right: right, at: point.x)
            guard percent != model.snapshot.inventory.byIdentity[work]?.reportedProgress else { return nil }
            model.reportProgress(work: work, percent: percent)
            return "progress \(model.editName(work)) \(percent)%"
        }
    }

    /// How much longer the bar was dragged, as a ratio of lengths rounded to a twentieth, at least a tenth.
    static func ratio(left: CGFloat, right: CGFloat, to x: CGFloat) -> Double {
        let ratio = Double(max(x - left, 1) / max(right - left, 1))
        return max(0.1, (ratio * 20).rounded() / 20)
    }

    /// The share of the bar left of `x`, in steps of 5%.
    static func percent(left: CGFloat, right: CGFloat, at x: CGFloat) -> Int {
        let share = Double((x - left) / max(right - left, 1))
        return Int((min(1, max(0, share)) * 20).rounded()) * 5
    }

    static func quarter(_ hours: Double) -> Double { (hours * 4).rounded() / 4 }

    /// An elbow connector from an end of a bar to `to`, as links are drawn: out from the bar's end, and into the target
    /// from the side its end faces, so the last run never crosses the target bar. Ends facing each other with room
    /// between turn once; ends facing the same way turn beyond both; a link running back turns between the rows.
    static func elbow(from: CGPoint, outward: CGFloat, to: CGPoint, inward: CGFloat) -> Path {
        var path = Path()
        let out = from.x + outward, entry = to.x + inward
        path.move(to: from)
        let facing = (outward > 0 && inward < 0 && entry >= out) || (outward < 0 && inward > 0 && entry <= out)
        let sameWay = (outward > 0 && inward > 0) || (outward < 0 && inward < 0) || inward == 0
        if facing {
            path.addLine(to: CGPoint(x: entry, y: from.y))
            path.addLine(to: CGPoint(x: entry, y: to.y))
        } else if sameWay {
            let turn = inward == 0 ? out : (outward > 0 ? max(out, entry) : min(out, entry))
            path.addLine(to: CGPoint(x: turn, y: from.y))
            path.addLine(to: CGPoint(x: turn, y: to.y))
            if inward != 0 { path.addLine(to: CGPoint(x: entry, y: to.y)) }
        } else {
            let between = (from.y + to.y) / 2
            path.addLine(to: CGPoint(x: out, y: from.y))
            path.addLine(to: CGPoint(x: out, y: between))
            path.addLine(to: CGPoint(x: entry, y: between))
            path.addLine(to: CGPoint(x: entry, y: to.y))
        }
        path.addLine(to: to)
        return path
    }

    static func arrowhead(at tip: CGPoint, pointingRight: Bool) -> Path {
        let back: CGFloat = pointingRight ? -6 : 6
        var head = Path()
        head.move(to: tip)
        head.addLine(to: CGPoint(x: tip.x + back, y: tip.y - 4))
        head.addLine(to: CGPoint(x: tip.x + back, y: tip.y + 4))
        head.closeSubpath()
        return head
    }
}

/// The drag being made: an elbow connector for a link with the end it would join, the new finish for an estimate, or
/// the progress share, and what a drop does.
struct GanttEditDragView: View {
    let drag: GanttEditDrag
    @ObservedObject var model: ObserverModel

    var body: some View {
        ZStack(alignment: .topLeading) {
            Canvas { context, _ in
                switch drag.mode {
                case let .link(from, fromEnd):
                    let outward: CGFloat = fromEnd == .finish ? 10 : -10
                    if let target = GanttEditDrag.target(at: drag.point, excluding: from, model: model) {
                        let tip = CGPoint(x: target.x, y: target.mid)
                        let inward: CGFloat = target.end == .start ? -10 : 10
                        context.stroke(GanttEditDrag.elbow(from: drag.origin, outward: outward, to: tip, inward: inward), with: .color(Palette.relationEmphasis), lineWidth: 2)
                        context.fill(GanttEditDrag.arrowhead(at: tip, pointingRight: target.end == .start), with: .color(Palette.relationEmphasis))
                        context.stroke(Path(ellipseIn: CGRect(x: tip.x - 5, y: tip.y - 5, width: 10, height: 10)), with: .color(Palette.relationEmphasis), lineWidth: 2)
                    } else {
                        context.stroke(GanttEditDrag.elbow(from: drag.origin, outward: outward, to: drag.point, inward: 0), with: .color(Palette.relationEmphasis),
                                       style: StrokeStyle(lineWidth: 2, dash: [5, 3]))
                    }
                    context.fill(Path(ellipseIn: CGRect(x: drag.origin.x - 4, y: drag.origin.y - 4, width: 8, height: 8)), with: .color(Palette.relationEmphasis))
                case .resize:
                    var path = Path()
                    path.move(to: CGPoint(x: drag.point.x, y: drag.origin.y - 12))
                    path.addLine(to: CGPoint(x: drag.point.x, y: drag.origin.y + 12))
                    context.stroke(path, with: .color(Palette.relationEmphasis), lineWidth: 2)
                case let .progress(_, left, right):
                    let share = CGFloat(GanttEditDrag.percent(left: left, right: right, at: drag.point.x)) / 100
                    let x = left + (right - left) * share
                    var path = Path()
                    path.move(to: CGPoint(x: x, y: drag.origin.y - 11))
                    path.addLine(to: CGPoint(x: x, y: drag.origin.y + 11))
                    context.stroke(path, with: .color(.primary), lineWidth: 2)
                }
            }
            Text(hint).font(.caption).padding(5)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 5))
                .offset(x: drag.point.x + 14, y: drag.point.y + 14)
        }
        .allowsHitTesting(false)
    }

    private var hint: String {
        switch drag.mode {
        case let .link(from, fromEnd):
            guard let target = GanttEditDrag.target(at: drag.point, excluding: from, model: model) else {
                return "Link from \(model.editName(from))'s \(fromEnd.rawValue): drop on a bar's left half for its start, right half for its finish"
            }
            return "\(GanttRelation.abbreviation(PlanEdit.kind(from: fromEnd, to: target.end))) \(model.editName(from)) → \(model.editName(target.id)); set a lag in the inspector"
        case let .resize(work, left, right):
            let ratio = GanttEditDrag.ratio(left: left, right: right, to: drag.point.x)
            return "Scale \(model.editName(work))'s three-point estimate by \(String(format: "%.2f", ratio))×; the review shows the exact hours"
        case let .progress(work, left, right):
            return "Report \(GanttEditDrag.percent(left: left, right: right, at: drag.point.x))% for \(model.editName(work)) as \(model.editing.actor)"
        }
    }
}

/// While editing, the draft's links drawn on the chart as dashed elbow connectors, and the handles of the bar under the
/// pointer, so what a drop proposed is visible where it was made.
struct GanttEditMarks: View {
    @ObservedObject var model: ObserverModel
    let hover: CGPoint?

    var body: some View {
        Canvas { context, _ in
            for edit in model.editing.draft?.edits ?? [] {
                guard case let .link(_, from, fromEnd, to, toEnd, _) = edit,
                      let source = GanttEditDrag.bar(of: from, model: model), let target = GanttEditDrag.bar(of: to, model: model) else { continue }
                let start = CGPoint(x: fromEnd == .finish ? source.right : source.left, y: source.mid)
                let tip = CGPoint(x: toEnd == .start ? target.left : target.right, y: target.mid)
                let path = GanttEditDrag.elbow(from: start, outward: fromEnd == .finish ? 10 : -10, to: tip, inward: toEnd == .start ? -10 : 10)
                context.stroke(path, with: .color(Palette.relationEmphasis), style: StrokeStyle(lineWidth: 2, dash: [5, 3]))
                context.fill(GanttEditDrag.arrowhead(at: tip, pointingRight: toEnd == .start), with: .color(Palette.relationEmphasis))
                // The label sits on the connector's vertical run, clear of the bars' own labels.
                let run = path.boundingRect
                let plate = Text("draft \(GanttRelation.abbreviation(PlanEdit.kind(from: fromEnd, to: toEnd)))").font(.system(size: 9, weight: .semibold))
                    .foregroundColor(Palette.relationEmphasis)
                let at = CGPoint(x: (fromEnd == .finish ? max(start.x, tip.x) : min(start.x, tip.x)) + (fromEnd == .finish ? 14 : -14), y: run.midY)
                context.fill(Path(roundedRect: CGRect(x: at.x - (fromEnd == .finish ? 2 : 50), y: at.y - 7, width: 52, height: 14), cornerRadius: 3),
                             with: .color(Color(nsColor: .windowBackgroundColor).opacity(0.9)))
                context.draw(plate, at: at, anchor: fromEnd == .finish ? .leading : .trailing)
            }
            guard let hover, hover.y >= GanttGeometry.axis, let row = model.ganttRow(atY: hover.y - GanttGeometry.axis),
                  let bar = GanttEditDrag.bar(row, model: model), hover.x >= bar.left - 12, hover.x <= bar.right + 12 else { return }
            let mid = bar.top + GanttLayout.rowHeight / 2
            for x in [bar.left, bar.right] {
                let ring = Path(ellipseIn: CGRect(x: x - 4.5, y: mid - 4.5, width: 9, height: 9))
                context.fill(ring, with: .color(Color(nsColor: .windowBackgroundColor)))
                context.stroke(ring, with: .color(Palette.relationEmphasis), lineWidth: 1.5)
            }
        }
        .allowsHitTesting(false)
    }
}
