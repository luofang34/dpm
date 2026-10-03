// DPM Observer: a local, read-only macOS window onto a DPM workspace.
//
// Quitting, or closing the window, ends the helper and nothing else: no run is stopped, no task is
// released and nothing is verified. Menu commands switch views and read again; none of them can
// write to a project.

import AppKit
import DPMObserverCore
import SwiftUI

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }

    /// The helper is ended before the process exits, so none is left running behind it. The ending
    /// runs off the main thread, and the answer is posted to the main run loop in every common mode,
    /// because the main queue is not served while AppKit waits for a deferred termination.
    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        let engine = MainActor.assumeIsolated { Launch.shared.model.engine }
        Task.detached {
            await engine?.close()
            CFRunLoopPerformBlock(CFRunLoopGetMain(), CFRunLoopMode.commonModes.rawValue) {
                NSApp.reply(toApplicationShouldTerminate: true)
            }
            CFRunLoopWakeUp(CFRunLoopGetMain())
        }
        return .terminateLater
    }
}

@main
struct ObserverApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @StateObject private var model = Launch.shared.model

    var body: some Scene {
        Window("DPM Observer", id: "main") {
            RootView(model: model)
                .frame(minWidth: 900, minHeight: 560)
                .onAppear { Launch.shared.start() }
        }
        .defaultSize(width: 1180, height: 760)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Open Project or Database…") { model.chooseWorkspace() }.keyboardShortcut("o", modifiers: .command)
                Button("Close Workspace") { model.close() }.keyboardShortcut("w", modifiers: [.command, .shift]).disabled(!model.hasWorkspace)
            }
            CommandMenu("Observe") {
                ForEach(ObserverModel.Page.allCases) { page in
                    Button("Show \(page.rawValue)") { model.show(page) }.keyboardShortcut(page.shortcut, modifiers: .command)
                }
                Button("Find Task or Decision") { model.findWork() }.keyboardShortcut("f", modifiers: .command).disabled(!model.hasWorkspace)
                Divider()
                Button("Read Again") { model.reload() }.keyboardShortcut("r", modifiers: .command).disabled(!model.hasWorkspace)
            }
        }
    }
}
