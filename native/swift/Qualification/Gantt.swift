// The Gantt, qualified through the real helper and the real model. The rows are compared with the shared
// `schedule` query read by the CLI at the same pinned clock; keys are real `NSEvent` values the suite
// builds, reduced by the same `KeyInput(event:)` and applied by the same `ObserverModel.handleKey` the
// window uses; and every view operation is shown read-only by the plan's revision, its history entry
// count (both read through the real CLI) and the bytes of the store or preview files on disk, each taken
// before and after. Each scenario asserts something that fails if the property it names breaks, and
// `gantt-negative-control` runs the key flow with key handling disabled, which must fail.

import AppKit
import CryptoKit
import DPMNative
import DPMObserverCore
import Foundation

// MARK: - Reading the query's numbers

private func number(_ json: JSON) -> Double? {
    switch json {
    case .number(let value): return value
    case .integer(let value): return Double(value)
    case .unsigned(let value): return Double(value)
    default: return nil
    }
}

/// The hierarchy order the Gantt must draw, worked out independently from the query's own rows: roots in
/// the projection's order, each followed by its children in that order.
func expectedOrder(_ work: [JSON]) -> [String] {
    let identities = Set(work.compactMap { $0["id"].string })
    var children: [String: [JSON]] = [:]
    var roots: [JSON] = []
    for item in work {
        if let parent = item["parent"].string, identities.contains(parent) { children[parent, default: []].append(item) } else { roots.append(item) }
    }
    var order: [String] = []
    func walk(_ item: JSON) {
        order.append(item["key"].string ?? "")
        for child in children[item["id"].string ?? ""] ?? [] { walk(child) }
    }
    roots.forEach(walk)
    return order
}

private func hex(_ url: URL) throws -> String {
    SHA256.hash(data: try Data(contentsOf: url)).map { String(format: "%02x", $0) }.joined()
}

private let isoDate = "[0-9]{4}-[0-9]{2}-[0-9]{2}"

/// What a read-only operation must leave as it found it, read by other means than the observer.
struct ReadOnlyRecord: Equatable, CustomStringConvertible {
    var revision: UInt64
    var history: Int
    var files: [String: String]
    var description: String { "revision \(revision), \(history) history entries, files \(files)" }
}

/// A count that can be read without being changed.
final class Tally: @unchecked Sendable {
    private let lock = NSLock()
    private var value = 0
    func bump() { lock.lock(); value += 1; lock.unlock() }
    var count: Int { lock.lock(); defer { lock.unlock() }; return value }
}

extension Context {
    // MARK: Opening the real model on the Gantt

    /// The real model with its real engine and helper, on the page asked for, once the rows are read.
    func openGanttModel(_ selection: WorkspaceSelection, page: ObserverModel.Page = .gantt, adjust: (inout ObserverEngine.Settings) -> Void = { _ in }) async throws -> ObserverModel {
        var settings = ObserverEngine.Settings(helper: helperURL)
        settings.pollInterval = 0.05
        settings.reconnectDelays = [0.05, 0.1]
        settings.runsRefreshInterval = 0.2
        settings.shutdownBound = 3
        settings.pinnedClock = pinned
        let audit = self.audit
        settings.onBlockingStep = { audit.record($0) }
        adjust(&settings)
        let fixed = settings
        let model = await MainActor.run { () -> ObserverModel in
            let model = ObserverModel()
            model.minimumInterval = 0
            model.configure(settings: fixed, problem: nil, page: page)
            model.open(selection)
            return model
        }
        do {
            _ = try await modelExpect(model, "the model to connect and read the schedule", 60) { $0.connection == .connected && ($0.gantt != nil || page != .gantt) }
        } catch {
            await MainActor.run { model.close() }
            throw error
        }
        return model
    }

    func closeGantt(_ model: ObserverModel) async {
        await MainActor.run { model.close() }
        _ = try? await modelExpect(model, "the workspace to close") { $0.connection == .closed }
    }

    func requirePlan(_ path: String, _ option: String) throws -> String {
        guard !path.isEmpty else { throw SuiteError(description: "the suite was not given \(option)") }
        return path
    }

    func readOnlyRecord(_ database: URL, files extra: [URL] = []) async throws -> ReadOnlyRecord {
        // The CLI reads come first, so whatever opening the store for them does is in both records alike.
        let revision = try await currentRevision(database)
        let history = try await historyCount(database)
        var files: [String: String] = [:]
        for file in [database] + extra + [URL(fileURLWithPath: database.path + "-wal")] where FileManager.default.fileExists(atPath: file.path) {
            files[file.lastPathComponent] = try hex(file)
        }
        return ReadOnlyRecord(revision: revision, history: history, files: files)
    }

    /// One key press built as the keyboard would deliver it, reduced as the window reduces it, and handed
    /// to the one shared handler. With `disabled`, the handler is not called: the negative control.
    @MainActor
    func press(_ model: ObserverModel, _ characters: String, shift: Bool = false, command: Bool = false, disabled: Bool = false) -> KeyOutcome {
        var flags: NSEvent.ModifierFlags = []
        if shift { flags.insert(.shift) }
        if command { flags.insert(.command) }
        guard let event = NSEvent.keyEvent(with: .keyDown, location: .zero, modifierFlags: flags, timestamp: ProcessInfo.processInfo.systemUptime, windowNumber: 0, context: nil,
                                           characters: characters, charactersIgnoringModifiers: characters, isARepeat: false, keyCode: 0),
              let input = KeyInput(event: event) else { return .ignored }
        return disabled ? .ignored : model.handleKey(input)
    }

    // MARK: Rows equal the query

    func ganttRowsEqualTheQuery() async throws {
        print("gantt: rows, bars and values are the shared schedule query's, in plan hierarchy order, with nothing computed in Swift")
        let database = try await newStore("gantt-rows", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let data = try await cliJSON(database, ["--clock", tools.clock, "schedule"])["data"]
        let work = data["work"].array ?? []
        let model = try await openGanttModel(.database(database))
        let (found, outline, inventory) = await MainActor.run { (model.snapshot.gantt, model.outline, model.snapshot.inventory) }
        guard let schedule = found else { throw SuiteError(description: "the Gantt page read no schedule") }
        check(!work.isEmpty && schedule.rows.count == work.count, "one row per work item of the query: \(schedule.rows.count) rows, \(work.count) in the query")
        var differences: [String] = []
        for (row, json) in zip(schedule.rows, work) {
            if row.key != json["key"].string || row.identity != json["id"].string { differences.append("\(row.key): identity or key") }
            if row.title != json["title"].string || row.status != json["status"].string || row.kind != json["kind"].string || row.priority != json["priority"].string { differences.append("\(row.key): title, status, kind or priority") }
            if row.parent != json["parent"].string { differences.append("\(row.key): parent") }
            if row.span?.start != number(json["span"]["start_hours"]) || row.span?.finish != number(json["span"]["finish_hours"]) { differences.append("\(row.key): span") }
            if row.times?.totalFloat != number(json["times"]["total_float_hours"]) || row.times?.freeFloat != number(json["times"]["free_float_hours"]) || row.times?.latestFinish != number(json["times"]["latest_finish_hours"]) { differences.append("\(row.key): float") }
            if row.criticality != number(json["criticality"]) { differences.append("\(row.key): criticality") }
        }
        check(differences.isEmpty, "every row's span, float, criticality, priority and status equal the query's: \(differences)")
        check(schedule.projectFinishHours == number(data["project_finish_hours"]), "the project finish is the query's \(schedule.projectFinishHours)")
        let spread = data["uncertainty"]
        check(schedule.uncertainty != nil && schedule.uncertainty?.p50 == number(spread["p50_finish_hours"]) && schedule.uncertainty?.p80 == number(spread["p80_finish_hours"]) && schedule.uncertainty?.p95 == number(spread["p95_finish_hours"]),
              "the p50, p80 and p95 finish hours are the query's, not computed: \(String(describing: schedule.uncertainty))")
        let order = expectedOrder(work)
        check(outline.map { $0.row.key } == order, "rows follow the plan hierarchy (a package, then its children): \(outline.map { $0.row.key })")
        check(outline.contains { $0.depth > 0 } && outline.contains { $0.row.isPackage && $0.children > 0 } && outline.contains { $0.row.isMilestone }, "the plan has a nested task, a package with children and a milestone, and the outline carries them")
        // A bar is sized from the query's elapsed hours at the zoom in force, and nothing else.
        let level = GanttLayout.defaultZoom
        let scale = GanttLayout.pointsPerHour(level)
        let tasks = schedule.rows.compactMap { row -> (GanttRow, GanttSpan)? in row.span.map { (row, $0) } }.filter { !$0.0.isMilestone && !$0.0.isPackage && $0.1.hours > 0 }
        let sized = tasks.allSatisfy { abs(GanttLayout.bar($0.1, level: level).width - $0.1.hours * scale) < 1e-9 && abs(GanttLayout.bar($0.1, level: level).x - $0.1.start * scale) < 1e-9 }
        check(tasks.count >= 2 && sized, "each bar's width and position are its span's hours times the zoom's points per hour (\(tasks.count) bars)")
        if let a = tasks.first, let b = tasks.first(where: { $0.1.hours != a.1.hours }) {
            let ratio = GanttLayout.bar(a.1, level: level).width / GanttLayout.bar(b.1, level: level).width
            check(abs(ratio - a.1.hours / b.1.hours) < 1e-9, "bar lengths are in the ratio of the hours: \(a.0.key) \(a.1.hours) h to \(b.0.key) \(b.1.hours) h")
            let zoomed = GanttLayout.bar(a.1, level: level + 1).width
            check(abs(zoomed - a.1.hours * GanttLayout.pointsPerHour(level + 1)) < 1e-9 && zoomed > GanttLayout.bar(a.1, level: level).width, "zooming in widens the same bar by the zoom's factor and changes no hour")
        } else {
            check(false, "the plan has no two tasks of different length to compare")
        }
        // Priority is the human's and is shown apart from criticality.
        if let item = outline.first(where: { $0.row.criticality != nil && !$0.row.priority.isEmpty }) {
            let words = GanttWords.row(item, schedule: schedule, inventory: inventory, selected: nil)
            check(words.value.range(of: "human priority \(item.row.priority)\\. ", options: .regularExpression) != nil && words.value.contains("criticality \(GanttWords.percent(item.row.criticality ?? 0)) of simulated schedules"),
                  "priority and criticality are separate statements: \(words.value)")
        } else {
            check(false, "no row carries both a priority and a criticality, so their separation cannot be shown")
        }
        await closeGantt(model)
    }

    // MARK: Relations, milestones and Detail text

    func ganttRelationsAndMilestonesAreText() async throws {
        print("gantt: FS, SS, FF and SF with lead or lag and Hard or Soft policy, and milestones, are drawn from the plan and listed as text")
        let database = try await newStore("gantt-relations", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let edges = try await cliJSON(database, ["export"])["data"]["dependencies"].array ?? []
        let model = try await openGanttModel(.database(database))
        let (found, snapshot, outline) = await MainActor.run { (model.snapshot.gantt, model.snapshot, model.outline) }
        guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
        let inventory = snapshot.inventory
        check(inventory.relations.count == edges.count && edges.count >= 10, "the observer holds every edge of the plan: \(inventory.relations.count) of \(edges.count)")
        var differences: [String] = []
        let byId = Dictionary(inventory.relations.map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        for edge in edges {
            guard let id = edge["id"].string, let held = byId[id] else { differences.append("an edge is missing"); continue }
            if held.kind != edge["kind"].string || held.lagHours != number(edge["lag_hours"]) || held.policy != (edge["policy"].string ?? "Hard") || held.predecessor != edge["predecessor"].string || held.successor != edge["successor"].string { differences.append("edge \(id)") }
        }
        check(differences.isEmpty, "each relation's kind, lead or lag, policy and ends equal the plan's: \(differences)")
        let kinds = Set(inventory.relations.map(\.abbreviation))
        check(kinds == ["FS", "SS", "FF", "SF"], "all four relation kinds are present: \(kinds.sorted())")
        let policies = Set(inventory.relations.map(\.policy))
        check(policies == ["Hard", "Soft"], "both policies are present: \(policies.sorted())")
        let names = GanttWords.namer(schedule: schedule, inventory: inventory)
        let texts = inventory.relations.map { $0.words(names: names) }
        check(texts.contains { $0.hasPrefix("SS (start to start)") && $0.contains("lag 2.0 h") && $0.contains("Soft") }, "a start-to-start relation reads its kind, its lag and its Soft policy: \(texts.filter { $0.hasPrefix("SS") })")
        check(texts.contains { $0.hasPrefix("FF (finish to finish)") && $0.contains("lead 1.0 h") && $0.contains("Hard") }, "a finish-to-finish relation reads its lead and its Hard policy: \(texts.filter { $0.hasPrefix("FF") })")
        check(texts.contains { $0.hasPrefix("SF (start to finish)") && $0.contains("lag 3.0 h") }, "a start-to-finish relation reads its lag: \(texts.filter { $0.hasPrefix("SF") })")
        check(texts.contains { $0.hasPrefix("FS (finish to start)") && $0.contains("no lag") }, "a finish-to-start relation with no lag says so")
        // Milestones are distinct in words as well as in the drawing.
        if let milestone = outline.first(where: { $0.row.isMilestone }), let task = outline.first(where: { !$0.row.isMilestone && !$0.row.isPackage }) {
            let said = GanttWords.row(milestone, schedule: schedule, inventory: inventory, selected: nil)
            let other = GanttWords.row(task, schedule: schedule, inventory: inventory, selected: nil)
            check(said.label.hasPrefix("Milestone \(milestone.row.key): ") && said.value.contains("milestone at") && other.label.hasPrefix("Task \(task.row.key): ") && !other.value.contains("milestone at"), "a milestone and a task read differently: \(said.label) / \(other.label)")
            check(milestone.row.span?.hours == 0, "a milestone is a point: zero hours long as the query says")
        } else {
            check(false, "the plan has no milestone and a task to compare")
        }
        // The same relations are listed in Detail, from the application's own explain, with kind, lead or lag and policy.
        guard let identity = identity(of: "TEST-A", in: snapshot) else { throw SuiteError(description: "TEST-A has no identity") }
        await MainActor.run { model.select(.work(identity)) }
        let shown = try await modelExpect(model, "TEST-A's detail", 30) { $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
        let listed = shown.detail?.sections.first { $0.title == "Dependencies" }?.rows.map(\.text) ?? []
        let hasSS = listed.contains { $0.contains("SS (start to start)") && $0.contains("lag 2.0 h") && $0.contains("Soft") }
        let hasFF = listed.contains { $0.contains("FF (finish to finish)") && $0.contains("lead 1.0 h") && $0.contains("Hard") }
        check(hasSS && hasFF, "Detail lists each relation of the selected task with kind, lead or lag and policy: \(listed)")
        await closeGantt(model)
    }

    // MARK: Dates

    func ganttDatesOnlyWhereSupplied() async throws {
        print("gantt: a calendar date is shown only where a query supplies one; the projection supplies none, so hours are elapsed")
        let plain = try requirePlan(tools.ganttPlan, "--gantt-plan")
        let withCalendars = try requirePlan(tools.calendarPlan, "--calendar-plan")
        for (name, plan) in [("plain", plain), ("calendar", withCalendars)] {
            let database = try await newStore("gantt-dates-\(name)", plan: plan)
            let data = try await cliJSON(database, ["--clock", tools.clock, "schedule"])["data"]
            let encoded = String(decoding: try JSONEncoder().encode(data["work"]), as: UTF8.self)
            check(encoded.range(of: isoDate, options: .regularExpression) == nil, "\(name) plan: no row of the schedule query carries a date")
            var keys = Set<String>()
            for row in data["work"].array ?? [] {
                if case .object(let span) = row["span"] { keys.formUnion(span.keys) }
                if case .object(let times) = row["times"] { keys.formUnion(times.keys) }
            }
            let known: Set<String> = ["start_hours", "finish_hours", "critical", "earliest_start_hours", "earliest_finish_hours", "latest_start_hours", "latest_finish_hours", "total_float_hours", "free_float_hours"]
            check(keys.isSubset(of: known), "\(name) plan: every time the query gives a row is elapsed hours (fields \(keys.sorted()))")
            let model = try await openGanttModel(.database(database))
            let (found, inventory, rows) = await MainActor.run { (model.snapshot.gantt, model.snapshot.inventory, model.outline) }
            guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
            let spoken = rows.map { GanttWords.row($0, schedule: schedule, inventory: inventory, selected: nil) }
            check(spoken.allSatisfy { ($0.label + " " + $0.value).range(of: isoDate, options: .regularExpression) == nil && $0.value.contains("no calendar date is supplied") }, "\(name) plan: no row's text carries a date, and each says none is supplied")
            check(schedule.evaluatedAt == pinned, "the one instant shown is when the schedule was evaluated, the pinned clock \(schedule.evaluatedAt)")
            if name == "calendar" { check(schedule.rows.contains { $0.calendar != nil }, "a plan with calendars shows each row's calendar placement in words, still with no date") }
            await closeGantt(model)
        }
    }

    // MARK: Long titles

    func ganttLongTitlesStayWhole() async throws {
        print("gantt: a 500-character title is read in full by assistive technology, never shortened")
        let database = try await newStore("gantt-long", plan: try requirePlan(tools.longPlan, "--long-plan"))
        let model = try await openGanttModel(.database(database))
        let (found, inventory, outline) = await MainActor.run { (model.snapshot.gantt, model.snapshot.inventory, model.outline) }
        guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
        let long = outline.filter { $0.row.title.count == 500 }
        check(long.count == 10, "ten rows carry a 500-character title: \(long.count)")
        let spoken = long.map { GanttWords.row($0, schedule: schedule, inventory: inventory, selected: nil) }
        let whole = zip(long, spoken).allSatisfy { $0.1.label.hasSuffix($0.0.row.title) && $0.1.label.count > 500 && $0.1.label.hasPrefix("Task \($0.0.row.key): ") }
        check(!spoken.isEmpty && whole, "each such row's label is its kind, key and the whole title: label lengths \(spoken.map { $0.label.count })")
        check(outline.allSatisfy { schedule.row($0.id)?.title == $0.row.title }, "no title in the outline differs from the query's")
        await closeGantt(model)
    }

    // MARK: A preview is read-only too

    func ganttPreviewIsReadOnly() async throws {
        print("gantt: on a read-only preview every view operation leaves the plan file and the locator byte for byte as they were")
        let plan = try requirePlan(tools.ganttPlan, "--gantt-plan")
        let decoded = try JSONDecoder().decode(JSON.self, from: Data(contentsOf: URL(fileURLWithPath: plan)))
        let root = try project("gantt-preview", workspace: decoded["workspace"]["id"].string ?? "", selects: "preview = 'plan.json'")
        let preview = root.appendingPathComponent(".dpm/plan.json")
        try FileManager.default.copyItem(at: URL(fileURLWithPath: plan), to: preview)
        let locator = root.appendingPathComponent(".dpm/project.toml")
        let before = [try hex(preview), try hex(locator)]
        let model = try await openGanttModel(.project(root))
        let source = await MainActor.run { model.snapshot.identity?.source }
        check(source == "preview", "the workspace is a read-only preview")
        let (rows, held) = await MainActor.run { () -> (Int, Int) in
            model.setViewport(rows: 3, width: 200)
            for keys in [",", ".", "=", "=", "-", "c", "x", "s", "x", " "] { _ = self.press(model, keys) }
            _ = self.press(model, KeyDecoder.right, shift: true)
            _ = self.press(model, KeyDecoder.down, shift: true)
            _ = self.press(model, "\r")
            return (model.outline.count, model.snapshot.gantt?.rows.count ?? 0)
        }
        check(rows > 0 && held > 0, "the preview's rows were drawn and operated (\(rows) of \(held))")
        let after = [try hex(preview), try hex(locator)]
        check(after == before, "the plan file and the locator are byte for byte what they were: \(before) and \(after)")
        await closeGantt(model)
    }
}
