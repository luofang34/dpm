// How the app starts: the arguments it was given, the model and engine they configure, and the
// operator's own choice of a workspace. With no arguments it waits; it never opens, creates or
// selects a workspace by itself.

import AppKit
import DPMNative
import DPMObserverCore
import Foundation

struct LaunchOptions {
    var selection: WorkspaceSelection?
    var helper: URL?
    var clock: Instant?
    var stateFile: URL?
    var page: ObserverModel.Page?
    var selectKey: String?
    var selectRun: String?
    var selectDecision: String?
    var problems: [String] = []

    init(_ arguments: [String]) {
        var iterator = arguments.dropFirst().makeIterator()
        while let flag = iterator.next() {
            func value() -> String? {
                if let next = iterator.next() { return next }
                problems.append("\(flag) needs a value")
                return nil
            }
            switch flag {
            case "--project": value().map { selection = .project(URL(fileURLWithPath: $0)) }
            case "--database": value().map { selection = .database(URL(fileURLWithPath: $0)) }
            case "--discover": value().map { selection = .discover(from: URL(fileURLWithPath: $0)) }
            case "--helper": value().map { helper = URL(fileURLWithPath: $0) }
            case "--state-file": value().map { stateFile = URL(fileURLWithPath: $0) }
            case "--select-key": selectKey = value()
            case "--select-run": selectRun = value()
            case "--select-decision": selectDecision = value()
            case "--clock":
                if let text = value() {
                    if let instant = Instant(text) { clock = instant } else { problems.append("--clock \(text) is not an RFC 3339 UTC instant") }
                }
            case "--page":
                if let text = value() {
                    if let found = ObserverModel.Page.allCases.first(where: { $0.rawValue.lowercased() == text.lowercased() }) { page = found } else { problems.append("--page \(text) is not now, live, review or detail") }
                }
            default: break
            }
        }
    }
}

/// The one model of the running app, with its optional state reporter.
@MainActor
final class Launch {
    static let shared = Launch()

    let options = LaunchOptions(CommandLine.arguments)
    let model = ObserverModel()
    private(set) var reporter: StateReporter?
    private var started = false
    private var signals: [DispatchSourceSignal] = []

    private init() {
        var problem: String? = options.problems.isEmpty ? nil : options.problems.joined(separator: "\n")
        var settings: ObserverEngine.Settings?
        if problem == nil {
            if let helper = options.helper ?? HelperLocation.bundled() {
                var found = ObserverEngine.Settings(helper: helper)
                found.pinnedClock = options.clock
                settings = found
            } else {
                problem = "No helper was found inside this app, so no workspace can be opened. Reinstall the app bundle, or pass --helper PATH."
            }
        }
        model.configure(settings: settings, problem: problem, page: options.page, selectKey: options.selectKey, selectRun: options.selectRun, selectDecision: options.selectDecision)
        reporter = options.stateFile.map { StateReporter(file: $0) }
    }

    /// Once, when the window first appears: report, and open the workspace the arguments named.
    func start() {
        guard !started else { return }
        started = true
        // A terminate request from outside quits the way the menu does, so the helper is ended first.
        for number in [SIGTERM, SIGINT] {
            signal(number, SIG_IGN)
            let source = DispatchSource.makeSignalSource(signal: number, queue: .main)
            source.setEventHandler { NSApp.terminate(nil) }
            source.resume()
            signals.append(source)
        }
        reporter?.attach(model)
        if let selection = options.selection { model.open(selection) }
    }
}

extension ObserverModel {
    /// The operator's own choice: a project directory or a store file. Nothing is created or changed.
    func chooseWorkspace() {
        let panel = NSOpenPanel()
        panel.title = "Open a DPM project or database"
        panel.message = "Choose a project directory containing .dpm/project.toml, or a DPM store file. Nothing is created or changed."
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = false
        panel.showsHiddenFiles = true
        guard panel.runModal() == .OK, let url = panel.url else { return }
        var directory: ObjCBool = false
        FileManager.default.fileExists(atPath: url.path, isDirectory: &directory)
        open(directory.boolValue ? .project(url) : .database(url))
    }
}
