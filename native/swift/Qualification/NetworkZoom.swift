// NETWORK-ZOOM-01: the network zooms and pans like a free canvas (Freeform's board), qualified on the real model.
// A pinch or a ⌥⌘-scroll zooms continuously about the pointer, so the graph point under it stays under it; a
// scroll's zoom is exponential, so zooming in and back out returns exactly; the keys step through presets about
// the canvas centre; the scale stays within its range; a graph smaller than the canvas is centred, and a larger one
// pans to each edge with room but never out of reach. Nothing is written.

import DPMNative
import DPMObserverCore
import Foundation

extension Context {
    func networkZoomsLikeACanvas() async throws {
        print("network: pinch and ⌥⌘-scroll zoom continuously about the pointer, keys step presets about the centre, small graphs centre, large ones pan with room")
        let database = try await newStore("network-zoom", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database), page: .network)
        let before = try await readOnlyRecord(database)

        // The pure anchor arithmetic: the graph point under the anchor is the same before and after.
        let pan = NetworkLayout.anchoredPan((40, -12), from: 0.8, to: 1.9, at: (150, 90))
        check(abs((40 + 150) / 0.8 - (pan.x + 150) / 1.9) < 1e-9 && abs((-12 + 90) / 0.8 - (pan.y + 90) / 1.9) < 1e-9,
              "the anchored pan keeps the graph point under the anchor: \(pan)")
        let small = NetworkLayout.axisBounds(content: 300, viewport: 800), large = NetworkLayout.axisBounds(content: 2000, viewport: 600)
        check(small.lowerBound == -250 && small.upperBound == -250 && large.lowerBound == -120 && large.upperBound == 1520,
              "a 300 pt graph in an 800 pt canvas is centred (\(small)); a 2000 pt one in 600 pt pans from 120 pt before to 120 pt past it (\(large))")

        let pinch = await MainActor.run { () -> (anchored: [Double], continuous: Bool, back: [Double], bounded: [Double]) in
            // A canvas smaller than the graph, so a zoom about a point needs no clamp of the pan.
            model.setNetworkViewport(width: 320, height: 200)
            model.performNetwork(.resetView)
            model.panNetwork(dx: 60, dy: 30)
            let anchor = NetworkPoint(x: 140, y: 90)
            let graphPoint = { (state: NetworkViewState) in ((state.panX + anchor.x) / state.scale, (state.panY + anchor.y) / state.scale) }
            let start = model.network, origin = graphPoint(start)
            var drift = 0.0
            // A pinch is a stream of small magnifications; the point under the fingers stays put through all of them.
            for magnification in [0.04, 0.06, 0.05, 0.08, -0.03] {
                model.zoomNetwork(by: 1 + magnification, at: anchor)
                let now = graphPoint(model.network)
                drift = max(drift, abs(now.0 - origin.0), abs(now.1 - origin.1))
            }
            let pinched = model.network.scale
            let continuous = !NetworkLayout.zoomPresets.contains { abs($0 - pinched) < 1e-6 } && abs(pinched - start.scale * 1.04 * 1.06 * 1.05 * 1.08 * 0.97) < 1e-9
            // A ⌥⌘-scroll up and the same distance down returns to the same scale and pan.
            let held = model.network
            model.zoomNetwork(by: exp(0.01 * 37), at: anchor)
            model.zoomNetwork(by: exp(-0.01 * 37), at: anchor)
            let back = [abs(model.network.scale - held.scale), abs(model.network.panX - held.panX), abs(model.network.panY - held.panY)]
            model.zoomNetwork(by: 1000, at: anchor)
            let top = model.network.scale
            model.zoomNetwork(by: 1e-6, at: anchor)
            return ([drift], continuous, back, [top, model.network.scale])
        }
        check(pinch.anchored[0] < 1e-6, "through a pinch the graph point under the pointer stays under it (largest drift \(pinch.anchored[0]) graph points)")
        check(pinch.continuous, "a pinch scales by its own magnification, not by whole zoom steps")
        check(pinch.back.allSatisfy { $0 < 1e-9 }, "a ⌥⌘-scroll in and back the same distance returns to the same scale and pan: \(pinch.back)")
        check(pinch.bounded == [NetworkLayout.maxScale, NetworkLayout.minScale], "the zoom stays within \(NetworkLayout.minScale)...\(NetworkLayout.maxScale): \(pinch.bounded)")

        let keys = await MainActor.run { () -> (scale: Double, drift: Double) in
            model.setNetworkViewport(width: 320, height: 200)
            model.performNetwork(.resetView)
            model.panNetwork(dx: 80, dy: 40)
            let state = model.network
            let centre = ((state.panX + 160) / state.scale, (state.panY + 100) / state.scale)
            _ = self.pressNetwork(model, "=")
            let now = model.network
            return (now.scale, max(abs((now.panX + 160) / now.scale - centre.0), abs((now.panY + 100) / now.scale - centre.1)))
        }
        check(abs(keys.scale - NetworkLayout.preset(above: NetworkLayout.defaultScale)) < 1e-9 && keys.drift < 1e-6,
              "= steps to the next preset (\(keys.scale)) about the canvas centre (drift \(keys.drift))")

        let bounds = await MainActor.run { () -> (centred: [Double], left: Double, right: Double, margin: Double, far: Double) in
            // A canvas larger than the graph at actual size: the graph is centred and cannot be dragged off it.
            model.setNetworkViewport(width: 4000, height: 3000)
            model.performNetwork(.resetView)
            let size = NetworkLayout.extent(model.networkGraph)
            model.panNetwork(dx: 500, dy: -500)
            let centred = [model.network.panX + (4000 - size.width) / 2, model.network.panY + (3000 - size.height) / 2]
            // A canvas smaller than the graph: each edge is reachable with room past it, and no further.
            model.setNetworkViewport(width: 300, height: 200)
            model.panNetwork(dx: -1e9, dy: 0)
            let left = model.network.panX
            model.panNetwork(dx: 1e9, dy: 0)
            return (centred, left, model.network.panX, min(NetworkLayout.panMargin, 100), size.width * model.network.scale - 300)
        }
        check(bounds.centred.allSatisfy { abs($0) < 1e-9 }, "a graph smaller than the canvas stays centred under a pan: offsets \(bounds.centred)")
        check(abs(bounds.left + bounds.margin) < 1e-9 && abs(bounds.right - (bounds.far + bounds.margin)) < 1e-9,
              "a graph larger than the canvas pans from \(bounds.margin) pt before its left edge (\(bounds.left)) to \(bounds.margin) pt past its right edge (\(bounds.right))")

        let after = try await readOnlyRecord(database)
        check(after == before, "nothing was written: before \(before), after \(after)")
        await closeGantt(model)
    }
}
