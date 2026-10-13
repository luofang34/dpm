// The observer's motion and brand values in one place, so every animated part uses the same few durations and
// curves and every one of them stops when a person asks the system for reduced motion. Data colours stay in
// `Palette`; nothing here carries meaning that is not also said in words or symbols.

import AppKit
import SwiftUI

enum DPMMotion {
    /// Durations in seconds, from a hover acknowledgement to a slow reveal.
    static let fast = 0.06
    static let snap = 0.12
    static let base = 0.2
    static let sheet = 0.3
    static let slow = 0.45

    /// The curves, as cubic Bézier control points shared by SwiftUI animations and the Gantt's own drawing.
    enum Curve {
        static let easeOut = CubicBezier(0.165, 0.84, 0.44, 1)
        static let snap = CubicBezier(0.32, 0.72, 0, 1)
        static let overshoot = CubicBezier(0.34, 1.3, 0.64, 1)
        static let reveal = CubicBezier(0.215, 0.61, 0.355, 1)
    }

    /// The usual pairings: a reveal of new content, and a state change that settles quickly.
    static let reveal = Curve.reveal.animation(base)
    static let settle = Curve.easeOut.animation(base)

    /// Whether the system asks for reduced motion now; read where no view environment is at hand.
    static var systemReducesMotion: Bool { NSWorkspace.shared.accessibilityDisplayShouldReduceMotion }

    /// `animation`, or none when motion is reduced: the change then happens at once, as it would without animation.
    static func unlessReduced(_ animation: Animation, reduceMotion: Bool) -> Animation? {
        reduceMotion || systemReducesMotion ? nil : animation
    }
}

/// A CSS-style cubic Bézier timing curve from (0, 0) to (1, 1), evaluated without SwiftUI so drawing code can ease its
/// own progress. `value(at:)` maps elapsed time to progress, both from 0 to 1.
struct CubicBezier {
    let x1: Double, y1: Double, x2: Double, y2: Double

    init(_ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double) {
        self.x1 = x1
        self.y1 = y1
        self.x2 = x2
        self.y2 = y2
    }

    func animation(_ duration: Double) -> Animation {
        Animation.timingCurve(x1, y1, x2, y2, duration: duration)
    }

    private func coordinate(_ t: Double, _ p1: Double, _ p2: Double) -> Double {
        // B(t) = 3(1-t)²t·p1 + 3(1-t)t²·p2 + t³, the end points being 0 and 1.
        let u = 1 - t
        return 3 * u * u * t * p1 + 3 * u * t * t * p2 + t * t * t
    }

    private func slope(_ t: Double, _ p1: Double, _ p2: Double) -> Double {
        let u = 1 - t
        return 3 * u * u * p1 + 6 * u * t * (p2 - p1) + 3 * t * t * (1 - p2)
    }

    /// The progress at elapsed fraction `x`: the curve's parameter is found by Newton's method, falling back to
    /// bisection where the slope is too flat, so the result is monotonic in `x` for every curve whose x is.
    func value(at x: Double) -> Double {
        if x <= 0 { return 0 }
        if x >= 1 { return 1 }
        var t = x
        for _ in 0..<8 {
            let error = coordinate(t, x1, x2) - x
            if abs(error) < 1e-6 { return coordinate(t, y1, y2) }
            let d = slope(t, x1, x2)
            if abs(d) < 1e-6 { break }
            t -= error / d
        }
        var low = 0.0, high = 1.0
        t = min(1, max(0, t))
        for _ in 0..<30 {
            let current = coordinate(t, x1, x2)
            if abs(current - x) < 1e-6 { break }
            if current < x { low = t } else { high = t }
            t = (low + high) / 2
        }
        return coordinate(t, y1, y2)
    }
}

enum DPMColor {
    /// The brand accent: Sokoly primary, a deeper blue on light backgrounds and a lighter one on dark, so controls keep
    /// their contrast in both appearances. Applied as the window's tint; the system accent still drives what SwiftUI
    /// leaves to it.
    static let accent = Color(nsColor: NSColor(name: NSColor.Name("DPMAccent")) { appearance in
        let dark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
        return dark ? NSColor(srgbRed: 0x4D / 255.0, green: 0x94 / 255.0, blue: 0xFF / 255.0, alpha: 1)
                    : NSColor(srgbRed: 0x00 / 255.0, green: 0x57 / 255.0, blue: 0xB7 / 255.0, alpha: 1)
    })
}
