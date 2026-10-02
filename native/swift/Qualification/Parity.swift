// Payloads through the bridge equal what the CLI prints, at one pinned clock, for a live store and a
// preview, and through the packaged host launched from another directory.

import DPMNative
import Foundation

extension Context {
    private var queries: [(String, JSON, [String])] {
        [
            ("status", query("status", ["probabilistic": .bool(true)]), ["status"]),
            ("next", query("next", ["capabilities": .array([]), "probabilistic": .bool(true), "limit": .integer(5)]), ["next"]),
            ("explain", query("explain", ["key": .string("TEST-B")]), ["explain", "TEST-B"]),
            ("show", query("show", ["key": .string("TEST-A")]), ["show", "TEST-A"]),
            ("history", query("history", ["after_sequence": .integer(0), "limit": .integer(100)]), ["history"]),
            ("revision", query("revision"), ["revision"]),
            ("runs", runsQuery, ["run", "list"]),
        ]
    }

    func payloadParity() async throws {
        print("payload parity with the CLI at a pinned clock")
        let database = try await newStore("parity")
        try await worker(database, ["claim", "TEST-A"])
        try await worker(database, ["start", "TEST-A"])
        let connection = try await open(.database(database), clock: pinned)
        for (name, request, arguments) in queries {
            let view = try await connection.query(request)
            let expected = try await cliJSON(database, ["--clock", tools.clock] + arguments)
            check(view.envelopeJSON == expected, "\(name): the bridge's envelope equals the CLI --json output")
            check(view.evaluatedAt == pinned, "\(name): evaluated at the pinned instant")
            check(view.basis.project != nil, "\(name): carries its project basis (provenance survives)")
        }
        let runs = try await connection.query(runsQuery)
        check(runs.basis.runs != nil, "a run query carries its run basis")
        try await connection.close()

        // A read-only preview through its locator.
        let plan = try JSONDecoder().decode(JSON.self, from: Data(contentsOf: URL(fileURLWithPath: tools.plan)))
        let root = try project("parity-preview", workspace: plan["workspace"]["id"].string ?? "", selects: "preview = 'plan.json'")
        try FileManager.default.copyItem(at: URL(fileURLWithPath: tools.plan), to: root.appendingPathComponent(".dpm/plan.json"))
        let preview = try await open(.project(root), clock: pinned)
        for (name, request, arguments) in queries {
            let view = try await preview.query(request)
            let result = try await runProcess(tools.dpm, ["--project", root.path, "--json", "--clock", tools.clock] + arguments)
            let expected = try JSONDecoder().decode(JSON.self, from: result.stdout)
            check(view.envelopeJSON == expected, "preview \(name): equals the CLI --json output")
        }
        try await preview.close()
    }

    func packagedHost() async throws {
        print("the packaged host, launched from another directory, finds its own helper")
        let database = try await newStore("host")
        let elsewhere = URL(fileURLWithPath: "/")
        for (name, _, arguments) in queries where ["status", "next", "history", "revision"].contains(name) {
            let host = try await runProcess(tools.host, ["--database", database.path, "--clock", tools.clock] + arguments.map { $0 == "run" ? "runs" : $0 }.filter { $0 != "list" }, directory: elsewhere)
            let expected = try await cliJSON(database, ["--clock", tools.clock] + arguments)
            let printed = try? JSONDecoder().decode(JSON.self, from: host.stdout)
            check(host.status == 0 && printed == expected, "dpm-host \(name) from / prints the CLI's payload (exit \(host.status))")
        }
        let refused = try await runProcess(tools.host, ["--database", database.path, "show", "NO-SUCH-KEY"], directory: elsewhere)
        let printed = try? JSONDecoder().decode(JSON.self, from: refused.stdout)
        check(refused.status == 1 && printed?["error"]["code"].string == "not_found", "a typed refusal is printed in the CLI's shape with exit 1")
        let cli = try await cli(database, ["show", "NO-SUCH-KEY"])
        let cliJSON = try? JSONDecoder().decode(JSON.self, from: cli.stdout)
        check(printed == cliJSON, "and equals the CLI's own refusal")
        let missing = try await runProcess(tools.host, ["--database", scratch("none.sqlite").path, "status"], directory: elsewhere)
        check(missing.status == 1 && (try? JSONDecoder().decode(JSON.self, from: missing.stdout))?["error"]["code"].string != nil, "a missing workspace is a structured refusal, not text")
        let usage = try await runProcess(tools.host, ["--database", database.path], directory: elsewhere)
        check(usage.status == 2, "a missing command is a usage error (exit 2)")
        try await hostLeavesNoHelper(database, elsewhere)
    }

    /// Every way the host can end, with a real helper already running, ends the helper first: the
    /// host does not exit until the helper has, so none is found running once it is gone.
    private func hostLeavesNoHelper(_ database: URL, _ directory: URL) async throws {
        let ways: [(String, [String], Int32)] = [
            ("success", ["status"], 0),
            ("an application refusal", ["show", "NO-SUCH-KEY"], 1),
            ("a usage error found after the helper started", ["show"], 2),
        ]
        for (name, words, status) in ways {
            let result = try await runProcess(tools.host, ["--database", database.path] + words, directory: directory)
            check(result.status == status, "the host ends with status \(status) on \(name) (got \(result.status))")
            let left = try await helpersRunning(on: database)
            check(!left, "no helper is left running after \(name)")
        }
    }
}
