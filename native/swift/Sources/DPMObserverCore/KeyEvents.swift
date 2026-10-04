// The reduction of a real keyboard event to a `KeyInput`. The window's event monitor and the
// qualification suite both build their input here, from an `NSEvent`, so a key reaches the model by
// one route: event, `KeyInput`, `ObserverModel.handleKey`.

import AppKit

extension KeyInput {
    /// The key press an `NSEvent` of type `keyDown` stands for; nil for any other event.
    public init?(event: NSEvent) {
        guard event.type == .keyDown, let characters = event.charactersIgnoringModifiers else { return nil }
        let flags = event.modifierFlags.intersection(.deviceIndependentFlagsMask)
        self.init(characters: characters, shift: flags.contains(.shift), option: flags.contains(.option), command: flags.contains(.command), control: flags.contains(.control))
    }
}
