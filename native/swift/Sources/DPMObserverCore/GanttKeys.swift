// What a key press means on the Gantt, decided from the press alone: the reduced key input, the intents
// it can carry and the one decoder from one to the other. Whether the page accepts an intent, and what it
// does, is the model's `handleKey`; nothing here touches the model.

import Foundation

/// A key press as the keyboard delivers it, reduced to what decides its meaning. It is built from a
/// real `NSEvent` (see `KeyEvents.swift`), and the suite builds the same from events it constructs.
public struct KeyInput: Equatable, Sendable {
    /// The characters the key makes without modifiers; an arrow or paging key is its function-key character.
    public var characters: String
    public var shift: Bool
    public var option: Bool
    public var command: Bool
    public var control: Bool

    public init(characters: String, shift: Bool = false, option: Bool = false, command: Bool = false, control: Bool = false) {
        self.characters = characters
        self.shift = shift
        self.option = option
        self.command = command
        self.control = control
    }
}

public enum GanttIntent: String, CaseIterable, Sendable {
    case moveUp, moveDown, pageUp, pageDown, first, last
    case expand, collapse, expandAll, collapseAll
    case select, openDetail, closeDetail
    case zoomIn, zoomOut
    case panLeft, panRight, panUp, panDown
    case toggleCritical, cycleStatus, clearFilter, focusFilter
    /// Tab and Shift-Tab while editing: a proposed parent change, reviewed before it applies.
    case indent, outdent
}

public enum KeyOutcome: Equatable, Sendable {
    case ignored
    case handled(GanttIntent)
}

/// The pure part: which intent a key press means, if any. It applies nothing.
public enum KeyDecoder {
    public static let up = "\u{F700}", down = "\u{F701}", left = "\u{F702}", right = "\u{F703}"
    public static let home = "\u{F729}", end = "\u{F72B}", pageUp = "\u{F72C}", pageDown = "\u{F72D}"

    public static func intent(for input: KeyInput) -> GanttIntent? {
        if input.control { return nil }
        if input.command {
            switch input.characters {
            case "d": return .openDetail
            case "[": return .closeDetail
            default: return nil
            }
        }
        if input.option { return nil }
        switch input.characters {
        case up: return input.shift ? .panUp : .moveUp
        case down: return input.shift ? .panDown : .moveDown
        case left: return input.shift ? .panLeft : .collapse
        case right: return input.shift ? .panRight : .expand
        case pageUp: return .pageUp
        case pageDown: return .pageDown
        case home: return .first
        case end: return .last
        case " ": return .select
        case "\r", "\u{3}": return .openDetail
        case "\u{1B}": return .closeDetail
        case "=", "+": return .zoomIn
        case "-", "_": return .zoomOut
        case "/": return .focusFilter
        case "c": return .toggleCritical
        case "s": return .cycleStatus
        case "x": return .clearFilter
        case "\t": return input.shift ? .outdent : .indent
        case "\u{19}": return .outdent
        case ",": return .collapseAll
        case ".": return .expandAll
        default: return nil
        }
    }
}
