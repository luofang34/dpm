// Where the keyboard was in the inspector: each field reports when it takes the keyboard, and the one that last had
// it takes it back when a review opened from the inspector is cancelled.

import DPMObserverCore
import SwiftUI

struct InspectorFocus: ViewModifier {
    @ObservedObject var model: ObserverModel
    let name: String
    @FocusState private var focused: Bool

    func body(content: Content) -> some View {
        content
            .focused($focused)
            .onChange(of: focused) { _, now in if now { model.noteInspectorFocus(name) } }
            .onChange(of: model.editing.inspectorReturns) { if model.editing.inspectorFocus == name { focused = true } }
    }
}

/// The link row's choices by keyboard: Option-Command-1 to -4 pick FS, SS, FF or SF and Option-Command-X exchanges after
/// and before, so a link of any kind and direction is entered without the pointer, as Tab reaches only fields. The
/// buttons are no Tab stops and no accessibility elements; the chords are named in the row's help.
struct LinkKeys: View {
    @Binding var kind: String
    @Binding var asPredecessor: Bool
    private static let kinds: [(KeyEquivalent, String)] = [("1", "FinishStart"), ("2", "StartStart"), ("3", "FinishFinish"), ("4", "StartFinish")]

    var body: some View {
        ZStack {
            ForEach(Self.kinds, id: \.1) { key, value in
                Button("") { kind = value }.keyboardShortcut(key, modifiers: [.command, .option]).focusable(false)
            }
            Button("") { asPredecessor.toggle() }.keyboardShortcut("x", modifiers: [.command, .option]).focusable(false)
        }
        .opacity(0)
        .accessibilityHidden(true)
    }
}
