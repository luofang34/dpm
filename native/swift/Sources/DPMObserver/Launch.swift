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
    /// Open with editing offered, as the Edit toggle would.
    var edit = false
    var selectKey: String?
    var selectRun: String?
    var selectDecision: String?
    /// Measurement, all inert unless `--measure-log` is given: the log, its identity, the surfaces whose
    /// draws are withheld (the negative control), and the scripted driver with its arguments.
    var measureLog: String?
    var measureId: String?
    var measureFreeze: Set<String> = []
    var measureScript: String?
    var measureKeys: [String] = []
    var measureSeconds = 5.0
    var measureDelay = 0.0
    /// The negative control of the coordinate check: points added to where a scripted scroll is drawn.
    var measureDisplace = 0.0
    /// Rendering of the Gantt window's own contents to PNG files, only when asked: where, under which
    /// name, and when ("loaded", "disconnected" or "stale"). Nothing else is written there.
    var renderTo: URL?
    var renderPrefix = "gantt"
    var renderWhen = "loaded"
    /// The window's content size at launch, for the suite's layout check at another size; the window's
    /// own minimum still applies. Without it the window opens at its default size.
    var windowSize: CGSize?
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
            case "--edit": edit = true
            case "--page":
                if let text = value() {
                    if let found = ObserverModel.Page.allCases.first(where: { $0.rawValue.lowercased() == text.lowercased() }) { page = found } else { problems.append("--page \(text) is not now, live, review, detail, gantt or network") }
                }
            case "--measure-log": measureLog = value()
            case "--measure-id": measureId = value()
            case "--measure-freeze": value().map { measureFreeze = Set($0.split(separator: ",").map(String.init)) }
            case "--measure-script": measureScript = value()
            case "--measure-keys": value().map { measureKeys = $0.split(separator: ",").map(String.init) }
            case "--measure-seconds":
                if let text = value() {
                    if let seconds = Double(text), seconds > 0 { measureSeconds = seconds } else { problems.append("--measure-seconds \(text) is not a positive number") }
                }
            case "--measure-delay":
                if let text = value() {
                    if let seconds = Double(text), seconds >= 0 { measureDelay = seconds } else { problems.append("--measure-delay \(text) is not a number of seconds") }
                }
            case "--measure-displace":
                if let text = value() {
                    if let points = Double(text), points.isFinite { measureDisplace = points } else { problems.append("--measure-displace \(text) is not a number of points") }
                }
            case "--window-size":
                if let text = value() {
                    let parts = text.split(separator: "x").compactMap { Double($0) }
                    if parts.count == 2, parts.allSatisfy({ $0 >= 100 }) { windowSize = CGSize(width: parts[0], height: parts[1]) } else { problems.append("--window-size \(text) is not WIDTHxHEIGHT") }
                }
            case "--render-to": value().map { renderTo = URL(fileURLWithPath: $0) }
            case "--render-prefix": value().map { renderPrefix = $0 }
            case "--render-when": value().map { renderWhen = $0 }
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
    /// The main window's layout measurements, kept only when a state is being reported.
    private(set) var layout: LayoutRegistry?
    /// What the main window's real views report about themselves, kept only when a state is being reported.
    private(set) var host: HostObservation?
    private var started = false
    private var signals: [DispatchSourceSignal] = []

    private init() {
        // The measurement log, if one was asked for, opens before anything else so that its first event
        // is the process's own entry.
        if let path = options.measureLog {
            let id = options.measureId ?? UUID().uuidString
            MeasureLog.shared = MeasureLog(path: path, id: id, freeze: options.measureFreeze, header: Self.measurementHeader())
            MeasureLog.shared?.expect("first", facts: ["connected": "true", "model_rows_positive": "true", "viewport_rows_drawn": "true"])
            Measure.coordinateFault = options.measureDisplace
            Measure.event("process_entry", gen: id, detail: ["arguments": Array(CommandLine.arguments.dropFirst())])
        }
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
        let reporter = options.stateFile.map { StateReporter(file: $0) }
        self.reporter = reporter
        layout = reporter == nil ? nil : LayoutRegistry()
        host = reporter == nil ? nil : HostObservation()
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
        // The window is sized, and editing offered, only once the titled main window is on screen: an inspector
        // presented while the window is still being created and sized can keep it from appearing at all.
        let size = options.windowSize, edit = options.edit
        if size != nil || edit {
            whenMainWindowIsShown { window in
                if let size { window.setContentSize(size) }
                if edit { self.model.setEditing(true) }
            }
        }
        reporter?.attach(model, layout: layout, host: host)
        GanttKeyboard.shared.install(model)
        NetworkPointer.model = model
        MeasureDriver.shared.start(model: model, options: options)
        GanttRenderer.shared.start(model: model, options: options)
        if let selection = options.selection { model.open(selection) }
    }

    /// Run `act` once with the titled main window as soon as it is on screen; other windows (the menu bar's, an
    /// inspector's) are visible too and are never it. Every pass of the event loop is checked until it is, so an app
    /// launched from a shell, which may never become active, still gets its size and editing.
    private func whenMainWindowIsShown(_ act: @escaping (NSWindow) -> Void) {
        func shown() -> NSWindow? { NSApp.windows.first { $0.isVisible && $0.styleMask.contains(.titled) } }
        if let window = shown() {
            DispatchQueue.main.async { act(window) }
            return
        }
        var token: NSObjectProtocol?
        token = NotificationCenter.default.addObserver(forName: NSApplication.didUpdateNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated {
                guard token != nil, let window = shown() else { return }
                if let token { NotificationCenter.default.removeObserver(token) }
                token = nil
                act(window)
            }
        }
    }

    /// The hardware line and the display's refresh rate, as the measurement log's header records them.
    private static func measurementHeader() -> [String: Any] {
        func text(_ name: String) -> String {
            var size = 0
            sysctlbyname(name, nil, &size, nil, 0)
            var bytes = [CChar](repeating: 0, count: max(size, 1))
            sysctlbyname(name, &bytes, &size, nil, 0)
            return String(cString: bytes)
        }
        return [
            "hardware_model": text("hw.model"), "cpu": text("machdep.cpu.brand_string"),
            "os": ProcessInfo.processInfo.operatingSystemVersionString,
            "display_max_fps": NSScreen.main?.maximumFramesPerSecond ?? 0,
        ]
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
