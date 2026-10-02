// Typed refusals cross the bridge unchanged, and a refused request writes nothing: invalid and
// stale commands, read-only previews, archives, and a source that is not the one attached to.

import DPMNative
import Foundation

extension Context {
    private func claim(_ work: String, base: UInt64, lineage: String? = nil) -> CommandRequest {
        CommandRequest(actor: workerActor, baseRevision: base, baseLineage: lineage, command: .object(["Claim": .object(["work": .string(work)])]))
    }

    private func workId(_ database: URL, _ key: String) async throws -> String {
        try await cliJSON(database, ["show", key])["data"]["id"].string ?? ""
    }

    func typedRefusals() async throws {
        print("typed refusals, with no partial writes")
        let database = try await newStore("refusals")
        let work = try await workId(database, "TEST-A")
        let connection = try await open(.database(database))

        // Invalid requests are the application's own refusals, equal to the CLI's.
        await checks.refused(.notFound, "an unknown work key") { _ = try await connection.query(query("show", ["key": .string("NO-SUCH-KEY")])) }
        do {
            _ = try await connection.query(query("show", ["key": .string("NO-SUCH-KEY")]))
        } catch BridgeError.refused(let native) {
            let cli = try await self.cli(database, ["show", "NO-SUCH-KEY"])
            check(native.json == (try? JSONDecoder().decode(JSON.self, from: cli.stdout)), "the refusal equals the CLI's error JSON, api version included")
        }
        await checks.refused(.invalidRequest, "a query with an unknown shape") { _ = try await connection.query(query("no_such_query")) }
        await checks.refused(.invalidRequest, "a command with an unknown field") {
            _ = try await connection.send(.command(.object(["actor": workerActor, "base_revision": .integer(0), "surprise": .bool(true), "command": .object([:])]), attached: nil)).result.map { _ in () }
            throw BridgeError.refused(NativeError(code: .invalidRequest, message: "", details: nil))
        }
        let before = try await historyCount(database)

        // Stale and wrong preconditions.
        await checks.refused(.revisionConflict, "a command at a stale revision") { _ = try await connection.execute(claim(work, base: 9)) }
        await checks.refused(.lineageMismatch, "a command from another lineage") { _ = try await connection.execute(claim(work, base: 0, lineage: UUIDv7.make())) }
        await checks.refused(.invalidCommand, "a command the domain refuses") {
            _ = try await connection.execute(CommandRequest(actor: workerActor, baseRevision: 0, command: .object(["Submit": .object(["work": .string(work), "note": .null])])))
        }
        let afterRefusals = try await historyCount(database)
        check(afterRefusals == before, "none of the refusals wrote anything")

        // Incorrect source identity.
        let attached = try await connection.attach()
        let wrong = Attachment(workspaceId: UUIDv7.make(), lineageId: nil)
        await checks.refused(.workspaceMismatch, "attaching expecting another workspace") { _ = try await connection.attach(expectWorkspace: wrong.workspaceId) }
        await checks.refused(.workspaceMismatch, "a query attached to another workspace") { _ = try await connection.query(statusQuery, attached: wrong) }
        await checks.refused(.lineageMismatch, "a query attached to another lineage") {
            _ = try await connection.query(statusQuery, attached: Attachment(workspaceId: attached.attachment.workspaceId, lineageId: UUIDv7.make()))
        }
        // A good command still works, and its answer is the application's recorded operation.
        let committed = try await connection.execute(claim(work, base: 0))
        check(committed.envelope.revision == 1, "a command at the observed revision commits")
        check(try await historyCount(database) == before + 1, "and writes exactly one operation")
        try await connection.close()

        try await previewAndArchive(database, work: work)
        try await repointedLocator(database)
    }

    private func previewAndArchive(_ database: URL, work: String) async throws {
        // A preview refuses every mutation and its file is untouched.
        let planURL = URL(fileURLWithPath: tools.plan)
        let plan = try JSONDecoder().decode(JSON.self, from: Data(contentsOf: planURL))
        let root = try project("refuse-preview", workspace: plan["workspace"]["id"].string ?? "", selects: "preview = 'plan.json'")
        let previewFile = root.appendingPathComponent(".dpm/plan.json")
        try FileManager.default.copyItem(at: planURL, to: previewFile)
        let digest = try Data(contentsOf: previewFile)
        let preview = try await open(.project(root))
        await checks.refused(.readOnlyProject, "a command against a preview") { _ = try await preview.execute(claim(work, base: 0)) }
        check(try Data(contentsOf: previewFile) == digest, "the preview file is byte for byte unchanged")
        try await preview.close()

        // A backup archive refuses writes and is not changed by the attempt.
        let archive = scratch("archive.sqlite")
        _ = try await cliJSON(database, ["backup", "--to", archive.path])
        let before = try Data(contentsOf: archive)
        let archived = try await open(.database(archive))
        check(archived.identity?.source == "archive", "an archive is attached as an archive")
        await checks.refused(.archivedStore, "a command against an archive") { _ = try await archived.execute(claim(work, base: 1)) }
        let status = try await archived.query(statusQuery)
        check(status.basis.project != nil, "an archive is still readable")
        try await archived.close()
        check(try Data(contentsOf: archive) == before, "the archive is byte for byte unchanged")
    }

    private func repointedLocator(_ database: URL) async throws {
        // The locator is repointed at another store while the connection stays open.
        let workspace = try await workspaceId(database)
        let root = try project("repoint", workspace: workspace, selects: "database = 'a.sqlite'")
        let first = root.appendingPathComponent(".dpm/a.sqlite"), second = root.appendingPathComponent(".dpm/b.sqlite")
        _ = try await cliJSON(first, ["import", tools.plan])
        let backup = scratch("repoint-backup.sqlite")
        _ = try await cliJSON(first, ["backup", "--to", backup.path])
        _ = try await cliJSON(second, ["restore", "--from", backup.path, "--to", second.path])
        let work = try await workId(first, "TEST-A")
        let connection = try await open(.project(root))
        let secondHistory = try await historyCount(second)
        try "version = 3\nworkspace = '\(workspace)'\ndatabase = 'b.sqlite'\n".write(to: root.appendingPathComponent(".dpm/project.toml"), atomically: true, encoding: .utf8)
        await checks.fails("a read after the locator was repointed", { error in
            if case .refused(let native) = error { return native.code == .sourceChanged }
            return false
        }) { _ = try await connection.query(statusQuery) }
        await checks.fails("a write after the locator was repointed", { error in
            if case .refused(let native) = error { return native.code == .sourceChanged }
            return false
        }) { _ = try await connection.execute(claim(work, base: 0)) }
        let firstAfter = try await historyCount(first)
        let secondAfter = try await historyCount(second)
        check(firstAfter == 0 && secondAfter == secondHistory, "neither store took the refused write")
        try await connection.close()
    }
}
