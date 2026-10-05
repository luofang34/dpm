// The network's and the Gantt's view geometry, qualified on the real model against disposable stores: a fit is
// whole when every node's transformed rectangle (and the whole extent) lies inside the viewport, the dense DS-D
// overview included; Locate is readable after a fit and honest when the filter hides the selected node; a hover
// is the node under the resting point in the CURRENT geometry and membership, and a resize ends it; the edge
// labels placed on the canvas touch no node and no other label. Nothing here writes: the read-only record is
// compared before and after.

import AppKit
import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    /// Every shown node's rectangle at the current scale and pan, as the canvas draws it.
    @MainActor
    private func transformedRects(_ model: ObserverModel) -> [(key: String, x: Double, y: Double, width: Double, height: Double)] {
        let state = model.network, graph = model.networkGraph
        return model.networkNodes.compactMap { node in
            guard let place = graph.place[node.identity] else { return nil }
            let r = NetworkLayout.rect(place)
            return (node.key, r.x * state.scale - state.panX, r.y * state.scale - state.panY, r.width * state.scale, r.height * state.scale)
        }
    }

    func networkFitIsWhole() async throws {
        print("network: Fit puts the whole dense DS-D graph (334 columns) inside the canvas, every node's rectangle included; Locate returns to a readable scale")
        let database = try await newStore("network-fit-dense", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        let before = try await readOnlyRecord(database)
        for viewport in [(706.0, 386.0), (499.0, 276.5)] {
            let fitted = await MainActor.run { () -> (Double, [Double], Int, Int, Bool) in
                model.setNetworkViewport(width: viewport.0, height: viewport.1)
                _ = self.pressNetwork(model, "f")
                let state = model.network, size = NetworkLayout.extent(model.networkGraph)
                let rects = self.transformedRects(model)
                let inside = rects.filter { $0.x >= -0.5 && $0.y >= -0.5 && $0.x + $0.width <= viewport.0 + 0.5 && $0.y + $0.height <= viewport.1 + 0.5 }.count
                return (state.scale, [-state.panX, -state.panY, size.width * state.scale, size.height * state.scale], inside, rects.count, state.fitScale != nil)
            }
            let extent = fitted.1
            check(fitted.4 && extent[0] == 0 && extent[1] == 0 && extent[2] <= viewport.0 + 0.5 && extent[3] <= viewport.1 + 0.5 && (extent[2] >= viewport.0 - 0.5 || extent[3] >= viewport.1 - 0.5),
                  "Fit in a \(viewport.0) x \(viewport.1) canvas: the whole transformed extent \(extent.map { ($0 * 10).rounded() / 10 }) lies inside it and fills one axis (scale \(fitted.0))")
            check(fitted.2 == fitted.3 && fitted.3 == 1100, "after Fit every one of the \(fitted.3) node rectangles lies inside the canvas (\(fitted.2) inside)")
        }
        let located = await MainActor.run { () -> (Double, Bool, String?) in
            if let task = model.networkGraph.nodes.first(where: { $0.key == "PERF-1000" }) { model.focusNode(task.identity, selecting: true) }
            _ = self.pressNetwork(model, "l")
            let rect = self.transformedRects(model).first { $0.key == "PERF-1000" }
            let visible = rect.map { $0.x >= 0 && $0.y >= 0 && $0.x + $0.width <= model.network.viewportWidth && $0.y + $0.height <= model.network.viewportHeight } ?? false
            return (model.network.scale, visible, model.networkFocusedNode?.key)
        }
        check(located.0 >= NetworkLayout.scale(NetworkLayout.defaultZoom - 1) && located.1 && located.2 == "PERF-1000",
              "Locate after Fit returns to a readable scale (\(located.0)) with PERF-1000 under the cursor and inside the canvas (\(located.1))")
        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }

    func networkHoverLocateAndLabels() async throws {
        print("network: a hover follows the current geometry and membership, a resize ends it; Locate is honest about a filtered-out selection; placed labels touch nothing")
        let database = try await newStore("network-hover", plan: try requirePlan(tools.networkPlan, "--network-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        _ = try await modelExpect(model, "the schedule to be read for the network", 60) { $0.gantt != nil }
        let before = try await readOnlyRecord(database)
        let hover = await MainActor.run { () -> (String?, String?, String?, String?, Bool) in
            model.setNetworkViewport(width: 706, height: 386)
            let rects = self.transformedRects(model)
            guard let target = rects.first(where: { $0.x > 0 && $0.y > 0 && $0.x + $0.width < 706 && $0.y + $0.height < 386 }) else { return (nil, nil, nil, nil, false) }
            let point = NetworkPoint(x: target.x + target.width / 2, y: target.y + target.height / 2)
            model.hoverNetwork(at: point)
            let first = model.networkHover?.key
            // A pan under the stationary pointer: the node inspected is the one under the point now (or none).
            model.panNetwork(dx: 0, dy: 0)
            _ = self.pressNetwork(model, "=")
            let afterZoom = model.networkHover?.key
            let drawnUnder = self.transformedRects(model).first { point.x >= $0.x && point.x <= $0.x + $0.width && point.y >= $0.y && point.y <= $0.y + $0.height }?.key
            // A filter that hides the node: a hidden node is never described.
            model.setNetworkFilterText("no-node-has-this-text")
            let filtered = model.networkHover?.key
            model.setNetworkFilterText("")
            model.hoverNetwork(at: point)
            model.setNetworkViewport(width: 600, height: 300)
            return (target.key, first, afterZoom == drawnUnder ? "same" : "\(afterZoom ?? "none") vs \(drawnUnder ?? "none")", filtered, model.network.hoverPoint == nil)
        }
        check(hover.0 != nil && hover.1 == hover.0, "a resting pointer inspects the node drawn under it: \(hover.1 ?? "none") (expected \(hover.0 ?? "none"))")
        check(hover.2 == "same", "after a zoom under the stationary pointer the node inspected is the one drawn under the point in the new geometry (\(hover.2 ?? "none"))")
        check(hover.3 == nil, "a filter that hides the node ends its inspection (inspected \(hover.3 ?? "none"))")
        check(hover.4, "a resize of the canvas ends the inspection until the pointer moves again")
        let locate = await MainActor.run { () -> (Subject?, Subject?, String?, [Double], [Double]) in
            model.setNetworkViewport(width: 706, height: 386)
            guard let node = model.networkGraph.nodes.first(where: { !$0.isDecision }) else { return (nil, nil, nil, [], []) }
            model.focusNode(node.identity, selecting: true)
            let selected = model.selection
            model.setNetworkFilterText("no-node-has-this-text")
            let pan = [model.network.panX, model.network.panY]
            _ = self.pressNetwork(model, "l")
            let note = model.network.locateNote
            let after = [model.network.panX, model.network.panY]
            let kept = model.selection
            model.setNetworkFilterText("")
            return (selected, kept, note, pan, after)
        }
        check(locate.0 != nil && locate.0 == locate.1 && (locate.2?.contains("filter") ?? false) && locate.3 == locate.4,
              "Locate with the selected node filtered out says so (\(locate.2 ?? "no note")), keeps the selection and does not pan to a hidden node")
        let labels = await MainActor.run { () -> (Int, Int, Int, Bool) in
            let graph = model.networkGraph
            var candidates: [NetworkLabels.Candidate] = []
            for edge in graph.edges where edge.kind == .temporal {
                guard let a = graph.place[edge.from], let b = graph.place[edge.to], let relation = edge.relation else { continue }
                let from = NetworkLayout.origin(column: a.column, row: a.row), to = NetworkLayout.origin(column: b.column, row: b.row)
                candidates.append(NetworkLabels.Candidate(edge: edge.id, text: "\(relation.abbreviation) \(relation.lagHours)", from: NetworkPoint(x: from.x + NetworkLayout.nodeWidth, y: from.y + NetworkLayout.nodeHeight / 2),
                                                          to: NetworkPoint(x: to.x, y: to.y + NetworkLayout.nodeHeight / 2), priority: false))
            }
            let boxes = graph.nodes.compactMap { graph.place[$0.identity].map(NetworkLayout.rect) }
            let result = NetworkLabels.place(candidates, nodes: boxes)
            var clean = true
            for (i, label) in result.placed.enumerated() {
                if boxes.contains(where: { label.touches($0) }) { clean = false }
                for other in result.placed[(i + 1)...] where label.touches((other.x, other.y, other.width, other.height)) { clean = false }
            }
            return (candidates.count, result.placed.count, result.omitted, clean)
        }
        check(labels.0 > 0 && labels.1 + labels.2 == labels.0 && labels.3,
              "the \(labels.0) temporal edge labels: \(labels.1) placed touching no node and no other label, \(labels.2) left off the canvas (their relations stay in the text list)")
        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }

    func ganttFitIsWhole() async throws {
        print("gantt: Fit timeline makes the whole plan exactly as wide as the measured timeline, at any width; a zoom from it steps on")
        let database = try await newStore("gantt-fit", plan: try requirePlan(tools.networkPlan, "--network-plan"))
        let model = try await openGanttModel(.database(database), page: .gantt)
        _ = try await modelExpect(model, "the schedule to be read", 60) { $0.gantt != nil }
        let before = try await readOnlyRecord(database)
        for width in [1400.0, 609.0, 329.0] {
            let fitted = await MainActor.run { () -> (Double, Double, Double) in
                model.setViewport(rows: 10, width: width)
                model.fitTimeline()
                let schedule = model.snapshot.gantt!
                return (GanttLayout.timelineWidth(schedule, scale: model.gantt.scale), model.gantt.panX, model.gantt.scale)
            }
            check(abs(fitted.0 - width) < 0.5 && fitted.1 == 0, "Fit timeline in \(width) pt: the whole timeline is \(fitted.0) pt wide from hour 0 (scale \(fitted.2) pt per hour)")
        }
        let zoomed = await MainActor.run { () -> (Double, Double, Bool) in
            let fitted = model.gantt.scale
            model.perform(.zoomIn)
            let now = model.gantt.scale
            model.fitTimelineInitially()
            return (fitted, now, model.gantt.scale == now)
        }
        let largest = GanttLayout.zoomSteps[GanttLayout.zoomSteps.count - 1]
        check((zoomed.0 >= largest ? zoomed.1 == zoomed.0 : zoomed.1 > zoomed.0) && zoomed.2,
              "a zoom in from the fitted scale steps to a larger one, never a smaller (\(zoomed.0) to \(zoomed.1)), and the automatic fit then leaves the chosen viewport alone")
        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }
}
