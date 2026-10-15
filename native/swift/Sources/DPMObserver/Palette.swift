// Colour roles of the observer's charts, named by the job each does so another client can use the
// same names and values. Status roles (what state a bar is in) and relation roles (what kind of link a
// line is) are separate sets; no colour is ever the only cue, since every role is paired with a shape,
// a dash or words. The values are the Sokoly workstation palette (Sokoly-Brand digital/color.md), whose checker
// keeps the selection colour and every data colour apart under each colour-vision deficiency and readable on every
// surface: ordinary work is neutral, so colour marks only the selection and what needs attention.

import AppKit
import SwiftUI

enum Palette {
    // Status roles: bars, milestones and nodes.
    /// Scheduled work with float: a neutral, so the selection and every state stand out from it.
    static let planned = dynamic(light: 0x758192, dark: 0x818D9F)
    /// Work on the critical path; always also said in words ("critical path").
    static let critical = dynamic(light: 0xAD274D, dark: 0xE85854)
    /// A package's bracket and a milestone's outline: structure, not state.
    static let structure = Color.primary.opacity(0.62)
    /// Recorded execution before the clock reading; also hatched and said in words ("recorded").
    static let recorded = Color.secondary.opacity(0.5)
    /// Float after the earliest finish, up to the latest finish.
    static let float = Color.secondary.opacity(0.75)

    // Relation roles: lines between work.
    /// A dependency that does not set its successor's start.
    static let relation = Color.secondary.opacity(0.45)
    /// The relations of the selected or cursor row, with their labels: they belong to the selection, so they take its
    /// colour.
    static let relationEmphasis = Color.accentColor
    /// A driving relation between critical work: the critical path's own links.
    static let relationCritical = critical
    /// A decision that gates work, which waits on a person: a heavy line ending in a bar.
    static let gate = dynamic(light: 0x4E1E61, dark: 0xDCBCF1)

    // Chart furniture.
    static let grid = Color.secondary.opacity(0.22)
    static let band = Color.secondary.opacity(0.06)
    /// The finish and percentile markers; their labels use the text colour.
    static let marker = Color.secondary.opacity(0.8)

    private static func dynamic(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil) { appearance in
            let isDark = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua
            return rgb(isDark ? dark : light)
        })
    }

    private static func rgb(_ value: UInt32) -> NSColor {
        NSColor(srgbRed: CGFloat((value >> 16) & 0xFF) / 255, green: CGFloat((value >> 8) & 0xFF) / 255,
                blue: CGFloat(value & 0xFF) / 255, alpha: 1)
    }
}
