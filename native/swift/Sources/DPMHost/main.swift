// The minimal macOS host: opens a DPM workspace through the packaged helper and prints what the
// shared application says, as the same JSON the CLI prints. It decides nothing: readiness, ranking
// and every refusal come from the application, and a typed refusal is printed in the shape the
// CLI uses. Rich screens are out of scope; this is the smallest real client of the bridge.
//
// Usage: dpm-host [--project DIR | --database PATH | --discover DIR] [--clock RFC3339]
//                 [--helper PATH] [--provenance] <command> [KEY]
// Commands: hello attach status next explain KEY show KEY history revision runs
//
// Exit status: 0 success, 1 the application refused (the refusal is on standard output),
// 2 usage, 3 the bridge failed (described on standard error).

import DPMNative
import Foundation

struct Usage: Error { let message: String }

struct Options {
    var selection: WorkspaceSelection?
    var clock: Instant?
    var helper: URL?
    var provenance = false
    var command: String?
    var key: String?
}

func parse(_ arguments: [String]) throws -> Options {
    var options = Options()
    var iterator = arguments.makeIterator()
    func value(_ flag: String) throws -> String {
        guard let next = iterator.next() else { throw Usage(message: "\(flag) needs a value") }
        return next
    }
    while let argument = iterator.next() {
        switch argument {
        case "--project": options.selection = .project(URL(fileURLWithPath: try value(argument)))
        case "--database": options.selection = .database(URL(fileURLWithPath: try value(argument)))
        case "--discover": options.selection = .discover(from: URL(fileURLWithPath: try value(argument)))
        case "--clock":
            let text = try value(argument)
            guard let instant = Instant(text) else { throw Usage(message: "--clock \(text) is not an RFC 3339 UTC instant") }
            options.clock = instant
        case "--helper": options.helper = URL(fileURLWithPath: try value(argument))
        case "--provenance": options.provenance = true
        default:
            if argument.hasPrefix("--") { throw Usage(message: "unknown option \(argument)") }
            if options.command == nil { options.command = argument } else if options.key == nil { options.key = argument } else { throw Usage(message: "unexpected argument \(argument)") }
        }
    }
    return options
}

func query(for options: Options) throws -> JSON? {
    func named(_ name: String, _ more: [String: JSON] = [:]) -> JSON { .object(["query": .string(name)].merging(more) { $1 }) }
    func requireKey() throws -> String {
        guard let key = options.key else { throw Usage(message: "\(options.command ?? "") needs a work key") }
        return key
    }
    switch options.command {
    // The defaults the CLI applies: forecasts on and five ranked tasks.
    case "status": return named("status", ["probabilistic": .bool(true)])
    case "next": return named("next", ["capabilities": .array([]), "probabilistic": .bool(true), "limit": .integer(5)])
    case "explain": return named("explain", ["key": .string(try requireKey())])
    case "show": return named("show", ["key": .string(try requireKey())])
    case "history": return named("history", ["after_sequence": .integer(0), "limit": .integer(100)])
    case "revision": return named("revision")
    case "runs": return named("runs")
    default: return nil
    }
}

func emit(_ value: JSON) {
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    if let data = try? encoder.encode(value) { FileHandle.standardOutput.write(data + Data([0x0A])) }
}

func fail(_ message: String) {
    FileHandle.standardError.write(Data((message + "\n").utf8))
}

/// What the host does with an open connection; every way out of it leaves the closing to the caller.
func perform(_ command: String, _ options: Options, on connection: NativeConnection) async throws {
    switch command {
    case "hello":
        emit(.object([
            "protocol": .integer(Int64(connection.protocolVersion ?? 0)),
            "views": .array((connection.capabilities?.views ?? []).map { .string($0.view) }),
            "commands": .bool(connection.capabilities?.commands ?? false),
        ]))
    case "attach":
        let attached = try await connection.attach()
        emit(.object([
            "workspace_id": .string(attached.attachment.workspaceId),
            "lineage_id": attached.attachment.lineageId.map { .string($0) } ?? .null,
            "source": .string(attached.source),
        ]))
    default:
        guard let request = try query(for: options) else { throw Usage(message: "unknown command \(command)") }
        let view = try await connection.query(request)
        if options.provenance {
            emit(.object([
                "provenance": .object([
                    "evaluated_at": .string(view.evaluatedAt.description),
                    "refresh_at": view.refreshAt.map { .string($0.description) } ?? .null,
                    "basis": (try? JSONDecoder().decode(JSON.self, from: JSONEncoder().encode(view.basis))) ?? .null,
                ]),
                "envelope": view.envelopeJSON,
            ]))
        } else {
            emit(view.envelopeJSON)
        }
    }
}

/// The exit status for a failure, after saying what it was.
func report(_ error: Error) -> Int32 {
    switch error {
    case let usage as Usage:
        fail("dpm-host: \(usage.message)")
        return 2
    case BridgeError.refused(let refusal), BridgeError.startupRefused(let refusal):
        emit(refusal.json)
        return 1
    default:
        fail("dpm-host: \(error)")
        return 3
    }
}

@main
struct DPMHost {
    static func main() async {
        var status: Int32 = 0
        var opened: NativeConnection?
        do {
            let options = try parse(Array(CommandLine.arguments.dropFirst()))
            guard let command = options.command else { throw Usage(message: "a command is required") }
            guard let helper = options.helper ?? HelperLocation.bundled() else {
                throw Usage(message: "no helper found beside this executable; pass --helper PATH")
            }
            guard let selection = options.selection else { throw Usage(message: "name the workspace with --project, --database or --discover") }
            var configuration = Configuration(helper: helper, selection: selection)
            configuration.pinnedClock = options.clock
            opened = try await NativeConnection.open(configuration)
            if let connection = opened { try await perform(command, options, on: connection) }
        } catch {
            status = report(error)
        }
        // Every path ends here: the helper is ended, and this process does not exit until it is,
        // so a failure above never leaves a helper running behind the exit.
        if let connection = opened {
            do { _ = try await connection.close() } catch {
                fail("dpm-host: the helper did not stop: \(error)")
                if status == 0 { status = 3 }
            }
        }
        exit(status)
    }
}
