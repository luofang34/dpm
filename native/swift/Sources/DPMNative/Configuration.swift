// How a connection finds its helper and which workspace it opens.
//
// The workspace is selected exactly as the command line selects it: an explicit project directory
// or database path overrides discovery, and otherwise the nearest project locator at or above a
// directory is used. The helper never creates one. Where the helper lives never depends on the
// working directory of whoever launched the host.

import Foundation

/// Which workspace the helper opens.
public enum WorkspaceSelection: Sendable {
    /// A project directory containing `.dpm/project.toml`.
    case project(URL)
    /// An explicit store path.
    case database(URL)
    /// The nearest project locator at or above this directory.
    case discover(from: URL)
}

public struct Configuration: Sendable {
    public var helper: URL
    public var selection: WorkspaceSelection
    /// Pins the helper's query clock for the whole session, as `dpm --clock` does; nil uses the system clock.
    public var pinnedClock: Instant?
    /// Longest answer this client accepts, in bytes. Keep it above the helper's own response bound.
    public var maxResponseBytes: Int = 33 * 1024 * 1024
    /// Bounds passed to the helper; nil keeps its defaults.
    public var helperMaxRequestBytes: Int?
    public var helperMaxResponseBytes: Int?
    /// Longest a request may wait for its answer, in seconds. Past it the connection is closed.
    public var requestTimeout: TimeInterval = 60
    /// The longest each step of ending the helper may take, in seconds.
    public var shutdownBound: TimeInterval = 5
    public var supportedProtocols: [Int] = [1]
    public var supportedApiVersions: [Int] = [12]
    /// Called with `launch`, `exchange`, `shutdown` and `abort` on the thread that performs each
    /// blocking step, so a host or a test can show none of them ran on its main thread, and with
    /// `admit` on the caller's thread when a request is accepted, which blocks nothing.
    public var onBlockingStep: (@Sendable (String) -> Void)?

    public init(helper: URL, selection: WorkspaceSelection) {
        self.helper = helper
        self.selection = selection
    }

    /// Refuse a configuration that could not run: a timeout or bound that is not a finite positive
    /// number would otherwise reach time arithmetic that traps, or never fire.
    func validate() throws {
        func positive(_ name: String, _ value: TimeInterval) throws {
            guard value.isFinite, value > 0, value <= 86_400 else {
                throw BridgeError.invalidConfiguration("\(name) must be a finite number of seconds between 0 and 86400, not \(value)")
            }
        }
        try positive("requestTimeout", requestTimeout)
        try positive("shutdownBound", shutdownBound)
        guard maxResponseBytes >= 1024, maxResponseBytes <= 1 << 30 else {
            throw BridgeError.invalidConfiguration("maxResponseBytes must be between 1024 and 2^30, not \(maxResponseBytes)")
        }
        for (name, bound) in [("helperMaxRequestBytes", helperMaxRequestBytes), ("helperMaxResponseBytes", helperMaxResponseBytes)] {
            if let bound = bound, bound < 1 || bound > 1 << 30 {
                throw BridgeError.invalidConfiguration("\(name) must be between 1 and 2^30, not \(bound)")
            }
        }
        // The protocol list is sent in the opening request, so it is as bounded as any request.
        guard (1...Self.maxVersions).contains(supportedProtocols.count), (1...Self.maxVersions).contains(supportedApiVersions.count) else {
            throw BridgeError.invalidConfiguration("supportedProtocols and supportedApiVersions must each name between 1 and \(Self.maxVersions) versions")
        }
        guard !Call.hello(protocols: supportedProtocols).exceeds(maxRequestBytes) else {
            throw BridgeError.invalidConfiguration("the opening request is longer than the \(maxRequestBytes)-byte request bound")
        }
    }

    /// The most protocol or api versions a configuration may list.
    static let maxVersions = 16

    /// The most a request frame may be: what the helper was told, or its own default.
    var maxRequestBytes: Int { helperMaxRequestBytes ?? 4 * 1024 * 1024 }

    var arguments: [String] {
        var arguments: [String] = []
        switch selection {
        case .project(let directory): arguments += ["--project", directory.path]
        case .database(let path): arguments += ["--database", path.path]
        case .discover: break
        }
        if let clock = pinnedClock { arguments += ["--clock", clock.description] }
        if let bound = helperMaxRequestBytes { arguments += ["--max-request-bytes", String(bound)] }
        if let bound = helperMaxResponseBytes { arguments += ["--max-response-bytes", String(bound)] }
        return arguments
    }

    var workingDirectory: URL? {
        if case .discover(let directory) = selection { return directory }
        return nil
    }
}

public enum HelperLocation {
    /// The helper that ships with the running host: `Contents/Helpers/dpm-native` inside an app
    /// bundle, or `dpm-native` beside a plain executable. It is resolved from where the host
    /// executable is, never from the working directory, so launching from anywhere finds the same one.
    public static func bundled() -> URL? {
        guard let executable = Bundle.main.executableURL?.resolvingSymlinksInPath() else { return nil }
        let directory = executable.deletingLastPathComponent()
        let candidates = [
            directory.deletingLastPathComponent().appendingPathComponent("Helpers/dpm-native"),
            directory.appendingPathComponent("dpm-native"),
        ]
        return candidates.first { FileManager.default.isExecutableFile(atPath: $0.path) }
    }
}
