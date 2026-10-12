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
