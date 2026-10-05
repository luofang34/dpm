// Launching the packaged helper, selecting the workspace the way the CLI does, and failing clearly.

import DPMNative
import Foundation

extension Context {
    func check(_ condition: Bool, _ message: @autoclosure () -> String) { checks.check(condition, message()) }

    func launchAndNegotiation() async throws {
        print("launch, workspace selection and negotiation")
        let database = try await newStore("launch")
        let connection = try await open(.database(database))
        check(connection.protocolVersion == 1, "the helper and client agree on protocol 1")
        let capabilities = connection.capabilities
        check(capabilities?.views.map { $0.view } == ["now", "live", "review", "detail", "gantt", "network"], "the six macOS views are mapped")
        check(capabilities?.commands == true, "a live store accepts commands")
        let controls = capabilities?.unsupported.map { $0.control } ?? []
        check(["steer_run", "stop_run", "answer_input_request", "provider_session_control"].allSatisfy(controls.contains), "unsupported controls are stated")
        check(connection.identity?.source == "live" && connection.isOpen, "attached to a live workspace")
        let outcome = try await connection.close()
        if case .exitedOnClose(let exit)? = outcome {
            check(exit.status == 0, "a clean close ends the helper by itself with status 0")
        } else {
            check(false, "a clean close should end the helper by itself, got \(String(describing: outcome))")
        }
        let again = try await connection.close()
        check(again == nil, "closing a closed connection is a no-op, not an error")

        // The same selection the CLI offers: a project directory, and discovery from a directory.
        let plan = try JSONDecoder().decode(JSON.self, from: Data(contentsOf: URL(fileURLWithPath: tools.plan)))
        let workspace = plan["workspace"]["id"].string ?? ""
        let root = try project("preview-project", workspace: workspace, selects: "preview = 'plan.json'")
        try FileManager.default.copyItem(at: URL(fileURLWithPath: tools.plan), to: root.appendingPathComponent(".dpm/plan.json"))
        let byProject = try await open(.project(root))
        check(byProject.identity?.source == "preview" && byProject.identity?.workspaceId == workspace, "a project locator opens its read-only preview")
        check(byProject.capabilities?.commands == false, "a preview does not offer commands")
        try await byProject.close()
        let discovered = try await open(.discover(from: root))
        check(discovered.identity == byProject.identity, "discovery from a directory finds the same workspace as naming it")
        try await discovered.close()
        let nested = root.appendingPathComponent("nested/deeper")
        try FileManager.default.createDirectory(at: nested, withIntermediateDirectories: true)
        // A directory with no locator of its own falls back to the nearest one above, as the CLI does.
        let viaParent = try await open(.discover(from: nested))
        check(viaParent.identity == byProject.identity, "the nearest locator at or above the directory wins")
        try await viaParent.close()

        await failures()
    }

    private func failures() async {
        let missing = scratch("absent.sqlite")
        await checks.fails("a workspace that does not exist is refused at startup", { error in
            if case .startupRefused(let native) = error { return native.code != .unknown("") && !native.message.isEmpty }
            return false
        }) { _ = try await open(.database(missing)) }
        check(!FileManager.default.fileExists(atPath: missing.path), "nothing is initialized for a missing workspace")

        await checks.fails("a directory with no locator is refused, never initialized", { error in
            if case .startupRefused = error { return true }
            return false
        }) {
            let bare = scratch("empty-dir-parent")
            try FileManager.default.createDirectory(at: bare, withIntermediateDirectories: true)
            _ = try await open(.discover(from: bare))
        }

        await checks.fails("arguments the helper cannot run are a typed startup refusal", { error in
            if case .startupRefused(let native) = error { return native.code == .invalidOptions }
            return false
        }) { _ = try await open(.database(missing), adjust: { $0.helperMaxResponseBytes = 5 }) }

        await checks.fails("a helper that is not there is a launch failure", { error in
            if case .launchFailed = error { return true }
            return false
        }) { _ = try await open(.database(missing), helper: URL(fileURLWithPath: "/nonexistent/dpm-native")) }

        let database = (try? await newStore("versions")) ?? missing
        await checks.fails("a protocol this client cannot speak is refused as incompatible", { error in
            if case .incompatible = error { return true }
            return false
        }) { _ = try await open(.database(database), adjust: { $0.supportedProtocols = [7] }) }
        await checks.fails("envelopes of another api version are refused as incompatible", { error in
            if case .incompatible = error { return true }
            return false
        }) { _ = try await open(.database(database), adjust: { $0.supportedApiVersions = [99] }) }
    }
}
