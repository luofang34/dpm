// GANTT-RELATION-LABEL-01: several emphasised relations ending at one target must not draw their labels at one
// point, and a label stays inside the chart and off the drawn bars when a free line exists. The relations are the real model's (the network fixture's TEST-E has two FS predecessors ending at its
// start and an FF ending at its finish); the anchors are worked out as the drawing works them out, and the
// placement is the one shared function the drawing calls. This checks the label geometry and the text; the
// pixels of the actual window are the UI verifier's.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func ganttRelationLabelsDoNotOverprint() async throws {
        print("gantt: the emphasised incoming relation labels of one target get distinct, non-overlapping places; the placement is deterministic; every relation stays in the row text")
        let database = try await newStore("gantt-relation-labels", plan: try requirePlan(tools.networkPlan, "--network-plan"))
        let model = try await openGanttModel(.database(database))
        let (found, inventory, outline, scale) = await MainActor.run { (model.snapshot.gantt, model.snapshot.inventory, model.outline, model.gantt.scale) }
        guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
        guard let position = outline.firstIndex(where: { $0.row.key == "TEST-E" }), let span = outline[position].row.span else {
            throw SuiteError(description: "the network fixture has no scheduled TEST-E row on the Gantt")
        }
        let target = outline[position]
        let names = GanttWords.namer(schedule: schedule, inventory: inventory)
        let incoming = inventory.relations(of: target.id).filter { $0.successor == target.id }
        let y = 30.0 + Double(position) * GanttLayout.rowHeight + GanttLayout.rowHeight / 2
        let requests = incoming.map { edge -> GanttRelationLabels.Request in
            let atFinish = edge.kind == "FinishFinish" || edge.kind == "StartFinish"
            let x = (atFinish ? span.finish : span.start) * scale
            return GanttRelationLabels.Request(id: edge.id, target: edge.successor, text: GanttRelationLabels.text(edge, from: names(edge.predecessor)), x: x + (atFinish ? 10 : -10), y: y, leading: atFinish)
        }
        let sharedPoint = Dictionary(grouping: requests) { "\($0.x),\($0.leading)" }.values.contains { $0.count >= 2 }
        check(incoming.count >= 3 && sharedPoint, "TEST-E has \(incoming.count) incoming relations, at least two ending at one point (the case that overprinted)")

        func disjoint(_ labels: [GanttRelationLabels.Label]) -> Bool {
            labels.indices.allSatisfy { i in labels.indices.allSatisfy { j in i == j || !labels[i].overlaps(labels[j]) } }
        }
        let placed = GanttRelationLabels.place(requests)
        check(placed.count == incoming.count && Set(placed.compactMap(\.id)) == Set(incoming.map(\.id)), "every incoming relation of TEST-E has its own label: \(placed.count) of \(incoming.count)")
        check(disjoint(placed), "no two of TEST-E's label rectangles overlap: \(placed.map { "\($0.text) @ \($0.minX)...\($0.maxX), \($0.minY)...\($0.maxY)" })")
        check(Set(placed.map { "\($0.x),\($0.y)" }).count == placed.count, "each label is drawn at its own point")
        check(placed.allSatisfy { label in incoming.contains { $0.id == label.id && label.text == GanttRelationLabels.text($0, from: names($0.predecessor)) && label.text.hasSuffix("from \(names($0.predecessor))") } },
              "each label is its relation's own words, naming its source: \(placed.map(\.text))")

        // Deterministic: the same relations in any order, and again, give the same placement.
        let again = GanttRelationLabels.place(requests)
        let reversed = GanttRelationLabels.place(requests.reversed())
        let rotated = GanttRelationLabels.place(Array(requests.dropFirst()) + Array(requests.prefix(1)))
        check(again == placed && reversed == placed && rotated == placed, "the placement is the same on every computation and for every input order")

        // More incoming relations than lines: the extra ones become one aggregate line, still nothing overlaps; a second
        // target one row below at the same x does not overlap the first target's labels either.
        if let edge = incoming.first {
            let many = (0..<7).map { GanttRelationLabels.Request(id: "\(edge.id)#\($0)", target: target.id, text: GanttRelationLabels.text(edge, from: "FEAT-\($0)"), x: 400, y: 300, leading: false) }
            let below = (0..<2).map { GanttRelationLabels.Request(id: "below#\($0)", target: "below", text: GanttRelationLabels.text(edge, from: "FEAT-9\($0)"), x: 400, y: 300 + GanttLayout.rowHeight, leading: false) }
            let crowded = GanttRelationLabels.place(many + below)
            let aggregate = crowded.filter { $0.id == nil }
            check(aggregate.count == 1 && aggregate[0].text == GanttRelationLabels.more(7 - (GanttRelationLabels.perTarget - 1)) && crowded.filter { $0.target == target.id }.count == GanttRelationLabels.perTarget,
                  "seven relations at one point give \(GanttRelationLabels.perTarget - 1) labels and one aggregate line: \(crowded.filter { $0.target == target.id }.map(\.text))")
            check(disjoint(crowded), "with two crowded targets no two label rectangles overlap (\(crowded.count) placed)")
            check(GanttRelationLabels.place((many + below).reversed()) == crowded, "the crowded placement is deterministic too")
        }

        // Inside the chart: two labels of a target near hour 0 run left of their arrowhead, past the chart's left edge; they
        // move right until they fit, still apart. A bar under the first line pushes a label to the next free line.
        if let edge = incoming.first {
            let edgeRequests = (0..<2).map { GanttRelationLabels.Request(id: "edge#\($0)", target: "early", text: GanttRelationLabels.text(edge, from: "TEST-\($0)"), x: 20, y: 200, leading: false) }
            let kept = GanttRelationLabels.place(edgeRequests, bounds: 2...600)
            check(kept.count == 2 && kept.allSatisfy { $0.minX >= 2 && $0.maxX <= 600 } && disjoint(kept),
                  "labels anchored near the chart's left edge stay inside it and apart: \(kept.map { "\($0.minX)...\($0.maxX), \($0.minY)" })")
            let lone = GanttRelationLabels.Request(id: "lone", target: "lone", text: GanttRelationLabels.text(edge, from: "TEST-A"), x: 300, y: 400, leading: false)
            let firstLine = 400 - GanttRelationLabels.firstLine
            let bar = GanttRelationLabels.Obstacle(minX: 0, minY: firstLine - 4, maxX: 600, maxY: firstLine + 4)
            let avoided = GanttRelationLabels.place([lone], bounds: 2...600, obstacles: [bar])
            check(avoided.count == 1 && !avoided[0].covers(bar), "a label moves off a bar to a free line: \(avoided.map { "\($0.minY)...\($0.maxY)" }) against the bar \(bar.minY)...\(bar.maxY)")
            let walled = (0..<GanttRelationLabels.lines).map { line -> GanttRelationLabels.Obstacle in
                let y = firstLine - Double(line) * GanttRelationLabels.lineHeight
                return GanttRelationLabels.Obstacle(minX: 0, minY: y - 4, maxX: 600, maxY: y + 4)
            }
            check(GanttRelationLabels.place([lone], bounds: 2...600, obstacles: walled).count == 1, "with bars on every line the label is still drawn, on its first free line")
        }

        // Nothing is lost: every relation of TEST-E is in the row's full text.
        let words = GanttWords.row(target, schedule: schedule, inventory: inventory, selected: nil).value
        let all = inventory.relations(of: target.id)
        check(all.count <= 8 && all.allSatisfy { words.contains($0.words(names: names)) }, "every relation of TEST-E is in its row text in full: \(all.count) relations")
        await closeGantt(model)
    }
}
