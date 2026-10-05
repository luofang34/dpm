// The integration suite's shared machinery: checks, an audit of which thread each blocking step
// ran on, a main-thread monitor, and helpers that drive the real CLI as another process.

import DPMNative
import Foundation

// MARK: - Checks

final class Checks {
    private let lock = NSLock()
    private(set) var failures: [String] = []
    private(set) var passed = 0

    func check(_ condition: Bool, _ message: @autoclosure () -> String) {
        let text = message()
        lock.lock()
        if condition { passed += 1 } else { failures.append(text) }
        lock.unlock()
        print(condition ? "  ok: \(text)" : "  FAIL: \(text)")
    }

    /// Expect the work to fail with a bridge error the predicate accepts.
    func fails(_ message: String, _ matches: (BridgeError) -> Bool, _ work: () async throws -> Void) async {
        do {
            try await work()
            check(false, "\(message): expected a failure, but it succeeded")
        } catch let error as BridgeError {
            check(matches(error), "\(message): \(error)")
        } catch {
            check(false, "\(message): failed with a non-bridge error \(error)")
        }
    }

    /// Expect the application's typed refusal with exactly this code.
    func refused(_ code: ErrorCode, _ message: String, _ work: () async throws -> Void) async {
        await fails(message, { error in
            if case .refused(let native) = error { return native.code == code }
            return false
        }, work)
    }
}

// MARK: - Which thread did each blocking step run on

final class BlockingAudit: @unchecked Sendable {
    private let lock = NSLock()
    private var all: [String] = []
    private var main: [String] = []

    func record(_ step: String) {
        lock.lock()
        all.append(step)
        // `admit` marks a request accepted on the caller's own thread and blocks nothing.
        if Thread.isMainThread && step != "admit" { main.append(step) }
        lock.unlock()
    }

    var steps: Set<String> { lock.lock(); defer { lock.unlock() }; return Set(all) }
    var count: Int { lock.lock(); defer { lock.unlock() }; return all.count }
    var mainThreadSteps: [String] { lock.lock(); defer { lock.unlock() }; return main }
}

/// Ticks on the main queue, so a blocked main thread shows up as a long gap between ticks.
final class MainThreadMonitor: @unchecked Sendable {
    private let timer = DispatchSource.makeTimerSource(queue: .main)
    private let lock = NSLock()
    private var last = DispatchTime.now()
    private var gap = 0.0
    private var count = 0

    func start() {
        timer.schedule(deadline: .now(), repeating: .milliseconds(2))
        timer.setEventHandler { [self] in
            let now = DispatchTime.now()
            lock.lock()
            gap = max(gap, Double(now.uptimeNanoseconds - last.uptimeNanoseconds) / 1_000_000)
            last = now
            count += 1
            lock.unlock()
        }
        timer.resume()
    }

    func reset() {
        lock.lock()
        last = DispatchTime.now()
        gap = 0
        count = 0
        lock.unlock()
    }

    var snapshot: (ticks: Int, maxGapMilliseconds: Double) {
        lock.lock()
        defer { lock.unlock() }
        return (count, gap)
    }
}

// MARK: - Processes as another party would run them

struct ProcessResult {
    let status: Int32
    let stdout: Data
    let stderr: Data
    var text: String { String(decoding: stdout, as: UTF8.self) }
}

/// Run a process to its end. Its exit is an event registered before launch, never a wait that can
/// miss it, and it runs off the caller's thread.
func runProcess(_ executable: String, _ arguments: [String], directory: URL? = nil, timeout: TimeInterval = 60) async throws -> ProcessResult {
    try await withCheckedThrowingContinuation { continuation in
        DispatchQueue.global().async {
            let process = Process()
            process.executableURL = URL(fileURLWithPath: executable)
            process.arguments = arguments
            if let directory = directory { process.currentDirectoryURL = directory }
            let out = Pipe(), err = Pipe()
            process.standardOutput = out
            process.standardError = err
            let exited = DispatchGroup()
            exited.enter()
            process.terminationHandler = { _ in exited.leave() }
            do { try process.run() } catch {
                exited.leave()
                continuation.resume(throwing: error)
                return
            }
            var collected = Data(), errors = Data()
            let drained = DispatchGroup()
            drained.enter()
            DispatchQueue.global().async { errors = err.fileHandleForReading.readDataToEndOfFile(); drained.leave() }
            collected = out.fileHandleForReading.readDataToEndOfFile()
            _ = drained.wait(timeout: .now() + timeout)
            guard exited.wait(timeout: .now() + timeout) == .success else {
                process.terminate()
                continuation.resume(throwing: BridgeError.shutdownFailed("\(executable) did not exit"))
                return
            }
            continuation.resume(returning: ProcessResult(status: process.terminationStatus, stdout: collected, stderr: errors))
        }
    }
}

struct SuiteError: Error, CustomStringConvertible {
    let description: String
}

// MARK: - The shared context

struct Tools {
    var dpm = "", helper = "", host = "", plan = "", laggedPlan = "", proxy = "", scratch = "", clock = "", writer = ""
    /// The plans of the Gantt scenarios, written by scripts/measure_gantt.py: the nested fixture, the same
    /// with calendars, a 1000-task plan with the dense overlay, and one with 500-character titles.
    var ganttPlan = "", calendarPlan = "", densePlan = "", longPlan = ""
}

final class Context {
    let tools: Tools
    let checks = Checks()
    let audit = BlockingAudit()
    let monitor: MainThreadMonitor
    private var counter = 0
    private let lock = NSLock()

    init(tools: Tools, monitor: MainThreadMonitor) {
        self.tools = tools
        self.monitor = monitor
    }

    var helperURL: URL { URL(fileURLWithPath: tools.helper) }
    var pinned: Instant { Instant(tools.clock) ?? Instant(seconds: 0) }

    func scratch(_ name: String) -> URL {
        lock.lock()
        counter += 1
        let number = counter
        lock.unlock()
        return URL(fileURLWithPath: tools.scratch).appendingPathComponent("\(number)-\(name)")
    }

    func configuration(_ selection: WorkspaceSelection, helper: URL? = nil, clock: Instant? = nil) -> Configuration {
        var configuration = Configuration(helper: helper ?? helperURL, selection: selection)
        configuration.pinnedClock = clock
        configuration.shutdownBound = 3
        let audit = self.audit
        configuration.onBlockingStep = { audit.record($0) }
        return configuration
    }

    func open(_ selection: WorkspaceSelection, helper: URL? = nil, clock: Instant? = nil, adjust: (inout Configuration) -> Void = { _ in }) async throws -> NativeConnection {
        var configuration = self.configuration(selection, helper: helper, clock: clock)
        adjust(&configuration)
        return try await NativeConnection.open(configuration)
    }

    /// The real CLI, `--json`, against one store.
    func cli(_ database: URL, _ arguments: [String]) async throws -> ProcessResult {
        try await runProcess(tools.dpm, ["--database", database.path, "--json"] + arguments)
    }

    func cliJSON(_ database: URL, _ arguments: [String]) async throws -> JSON {
        let result = try await cli(database, arguments)
        guard result.status == 0 else { throw SuiteError(description: "dpm \(arguments.joined(separator: " ")) exited \(result.status): \(result.text)") }
        return try JSONDecoder().decode(JSON.self, from: result.stdout)
    }

    /// A disposable writable store imported from a synthetic plan.
    func newStore(_ name: String, plan: String? = nil) async throws -> URL {
        let database = scratch("\(name).sqlite")
        _ = try await cliJSON(database, ["import", plan ?? tools.plan])
        return database
    }

    func historyCount(_ database: URL) async throws -> Int {
        try await cliJSON(database, ["history"])["data"]["entries"].array?.count ?? -1
    }

    func worker(_ database: URL, _ words: [String]) async throws {
        _ = try await cliJSON(database, words + ["--actor", "agent:worker"])
    }

    /// A project directory whose locator selects `database` or `preview`.
    func project(_ name: String, workspace: String, selects line: String) throws -> URL {
        let root = scratch(name)
        try FileManager.default.createDirectory(at: root.appendingPathComponent(".dpm"), withIntermediateDirectories: true)
        try "version = 3\nworkspace = '\(workspace)'\n\(line)\n".write(to: root.appendingPathComponent(".dpm/project.toml"), atomically: true, encoding: .utf8)
        return root
    }

    func workspaceId(_ database: URL) async throws -> String {
        try await cliJSON(database, ["export"])["data"]["workspace"]["id"].string ?? ""
    }

    /// An executable wrapper that runs the real helper behind the fault-injecting proxy.
    func faultyHelper(_ mode: String, after: Int = 2) throws -> URL {
        let script = scratch("helper-\(mode)")
        let text = "#!/bin/sh\nexec python3 '\(tools.proxy)' --mode \(mode) --after \(after) --real '\(tools.helper)' \"$@\"\n"
        try text.write(to: script, atomically: true, encoding: .utf8)
        try FileManager.default.setAttributes([.posixPermissions: 0o755], ofItemAtPath: script.path)
        return script
    }

    /// Whether any process still has this store's path on its command line: a helper left behind.
    func helpersRunning(on database: URL) async throws -> Bool {
        try await runProcess("/usr/bin/pgrep", ["-f", database.path]).status == 0
    }

    func fileDigest(_ url: URL) throws -> Data { try Data(contentsOf: url) }
}

let statusQuery: JSON = .object(["query": .string("status"), "probabilistic": .bool(false)])
let runsQuery: JSON = .object(["query": .string("runs")])

func query(_ name: String, _ more: [String: JSON] = [:]) -> JSON {
    .object(["query": .string(name)].merging(more) { $1 })
}

let workerActor: JSON = .object(["kind": .string("Agent"), "name": .string("worker")])
