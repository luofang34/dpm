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

    func body(content: Content) -> some View {
        let hover = resting.flatMap { row(at: $0) }
        content
            .contentShape(Rectangle())
            .gesture(SpatialTapGesture(count: 1).onEnded { tap($0.location, open: GanttClicks.isDouble) })
            .simultaneousGesture(
                DragGesture(minimumDistance: 4)
                    .onChanged { value in
                        let dx = value.translation.width - dragged.width, dy = value.translation.height - dragged.height
                        dragged = value.translation
                        model.pan(dx: -dx, dy: -dy)
                    }
                    .onEnded { _ in
                        dragged = .zero
                        log.note("drag_end", ["pan": [model.gantt.panX, model.gantt.panY]])
                    }
            )
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
