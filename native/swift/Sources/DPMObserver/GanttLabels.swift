// The Gantt's rows beside the timeline: real accessibility elements carrying each row's full text, placed at the
// rows' drawn positions, so a fold's band and the timeline agree on where every row is.

import AppKit
import DPMObserverCore
import SwiftUI

struct GanttLabels: View {
    let frame: GanttFrame
    let geometry: GanttGeometry
    var model: ObserverModel?

    var body: some View {
        VStack(spacing: 0) {
            HStack {
                Text("Work (\(frame.outline.count) rows)").font(.caption.weight(.semibold)).accessibilityAddTraits(.isHeader)
                Spacer()
            }
            .padding(.horizontal, 8)
            .frame(height: GanttGeometry.axis)
            .reportFrame("axis")
            let band = frame.band(geometry)
            let shown = Array(frame.outline[geometry.firstRow..<(geometry.firstRow + geometry.rowsDrawn)])
            let revealing = band == nil ? [] : shown.filter { frame.motion?.entering.contains($0.id) == true }
            ZStack(alignment: .topLeading) {
                ForEach(band == nil ? shown : shown.filter { frame.motion?.entering.contains($0.id) != true }) { item in
                    GanttLabel(item: item, frame: frame, model: model)
                        .frame(height: GanttLayout.rowHeight)
                        .offset(y: frame.top(of: item.id, at: item.position, geometry) - GanttGeometry.axis)
                        .background { if item.position == geometry.firstRow { ObservedProbe(name: "first_row") } }
                }
                // Rows being hidden or revealed stay in place and are seen only through the band under their package.
                if let band, let motion = frame.motion {
                    ZStack(alignment: .topLeading) {
                        ForEach(revealing) { item in
                            GanttLabel(item: item, frame: frame, model: model)
                                .frame(height: GanttLayout.rowHeight)
                                .offset(y: frame.top(of: item.id, at: item.position, geometry) - GanttGeometry.axis)
                        }
                        ForEach(motion.visibleLeaving(geometry, progress: frame.progress), id: \.item.id) { leaving in
                            GanttLabel(item: leaving.item, frame: frame, model: nil)
                                .frame(height: GanttLayout.rowHeight)
                                .offset(y: leaving.top - GanttGeometry.axis)
                                .allowsHitTesting(false)
                                .accessibilityHidden(true)
                        }
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                    .mask(alignment: .topLeading) {
                        Rectangle().frame(height: max(band.height, 0)).offset(y: band.top - GanttGeometry.axis)
                    }
                }
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
            .clipped()
            .reportFrame("rows")
        }
        .accessibilityElement(children: .contain)
        .accessibilityLabel("Gantt rows")
        .accessibilityIdentifier("gantt.rows")
    }
}

struct GanttLabel: View {
    let item: GanttOutlineRow
    let frame: GanttFrame
    var model: ObserverModel?

    var body: some View {
        let row = item.row
        let said = speech(item, frame: frame)
        HStack(spacing: 4) {
            Color.clear.frame(width: Double(item.depth) * 14)
            if item.children > 0 {
                Image(systemName: item.expanded ? "chevron.down" : "chevron.right").font(.system(size: 9, weight: .bold)).frame(width: 12)
                    .accessibilityHidden(true)
                    .onTapGesture { model?.toggleDisclosure(item.id) }
            } else {
                Color.clear.frame(width: 12)
            }
            Image(systemName: GanttGlyph.symbol(row)).font(.system(size: 10)).foregroundStyle(row.status == "Verified" ? .secondary : .primary)
                .help(row.status).accessibilityHidden(true)
            Text(row.key).font(.system(size: 11, design: .monospaced))
            Text(row.title).font(.system(size: 12, weight: row.isPackage ? .semibold : .regular)).lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 2)
            Text(row.priority).font(.system(size: 10)).foregroundStyle(.secondary)
        }
        .padding(.horizontal, 6)
        // The row's band and selection continue across the name, as on the timeline beside it.
        .background(frame.selected == row.id ? Color.accentColor.opacity(0.16) : (item.position % 2 == 0 ? Palette.band : Color.clear))
        .contentShape(Rectangle())
        .help("\(row.key): \(row.title)")
        .onTapGesture {
            model?.focusRow(item.id)
            if GanttClicks.isDouble { model?.perform(.openDetail) }
        }
        .spoken(said.label, value: said.value)
        .accessibilityAddTraits(frame.selected == row.id ? [.isButton, .isSelected] : .isButton)
        .accessibilityAction(named: "Open in Detail") {
            model?.focusRow(item.id)
            model?.perform(.openDetail)
        }
        .accessibilityIdentifier(ObserverModel.element(forRow: row.key))
    }
}
