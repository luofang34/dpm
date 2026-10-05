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
private func expectedOrder(_ work: [JSON]) -> [String] {
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

/// Everything a step of a flow asserts, read from the model in one turn of the main actor.
struct GanttSnap: Sendable {
    var focus: String?
    var selection: Subject?
    var page: ObserverModel.Page
    var rows: Int
    var zoom: Int
    var panX: Double
    var panY: Double
    var criticalOnly: Bool
    var status: String?
    var editing: Bool
    var origin: ObserverModel.DetailReturn?
}

@MainActor
private func snap(_ model: ObserverModel) -> GanttSnap {
    GanttSnap(focus: model.focusTarget, selection: model.selection, page: model.page, rows: model.outline.count, zoom: model.gantt.zoomLevel, panX: model.gantt.panX, panY: model.gantt.panY,
              criticalOnly: model.gantt.filter.criticalOnly, status: model.gantt.filter.status, editing: model.gantt.editingFilter, origin: model.detailReturn)
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

    // MARK: The keyboard flow, through the shared handler

    func ganttKeysFlow() async throws { try await keyFlow(disabled: false) }

    /// Must fail: the same flow with key handling disabled. The packaged qualification runs it by name and
    /// requires a nonzero exit, so a flow that passes without keys cannot return unnoticed.
    func ganttNegativeControl() async throws { try await keyFlow(disabled: true) }

    private func keyFlow(disabled: Bool) async throws {
        print(disabled ? "gantt: negative control, the key flow with key handling disabled must fail" : "gantt: real key events through the shared handler: move, expand, collapse, select, Detail, return, zoom, pan, filter, and nothing written")
        let database = try await newStore(disabled ? "gantt-control" : "gantt-keys", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let work = try await cliJSON(database, ["--clock", tools.clock, "schedule"])["data"]["work"].array ?? []
        let order = expectedOrder(work)
        guard let packageAt = order.firstIndex(of: "TEST-PKG"), packageAt + 3 < order.count else { throw SuiteError(description: "the plan has no package with children: \(order)") }
        let model = try await openGanttModel(.database(database))
        let before = try await readOnlyRecord(database)
        let first = await MainActor.run { model.snapshot }
        guard let nested = identity(of: order[packageAt + 1], in: first), let sibling = identity(of: order[packageAt + 2], in: first) else { throw SuiteError(description: "no identity for the nested tasks") }
        let nestedKey = order[packageAt + 1]
        func key(_ characters: String, shift: Bool = false, command: Bool = false) async -> KeyOutcome {
            await MainActor.run { self.press(model, characters, shift: shift, command: command, disabled: disabled) }
        }
        func now() async -> GanttSnap { await MainActor.run { snap(model) } }
        await MainActor.run { model.setViewport(rows: 3, width: 100) }
        var state = await now()
        check(state.focus == "gantt.row.\(order[0])", "the cursor starts on the first row, \(order[0]): \(state.focus ?? "none")")
        // Arrow movement down to the package, then in: a package that is open moves the cursor to its first child.
        var moves: [KeyOutcome] = []
        for _ in 0..<packageAt { moves.append(await key(KeyDecoder.down)) }
        state = await now()
        check(state.focus == "gantt.row.TEST-PKG" && moves.allSatisfy { $0 == .handled(.moveDown) }, "down arrow moves the cursor to the package: \(state.focus ?? "none")")
        var outcome = await key(KeyDecoder.right)
        state = await now()
        check(outcome == .handled(.expand) && state.focus == "gantt.row.\(nestedKey)", "right arrow on an open package goes in to its first child, a nested task: \(state.focus ?? "none")")
        // Select it: the shared selection, and its detail is read from the application.
        outcome = await key(" ")
        state = await now()
        check(outcome == .handled(.select) && state.selection == .work(nested), "space selects the nested task in the shared selection")
        if state.selection == .work(nested) {
            _ = try await modelExpect(model, "the selected task's detail", 30) { $0.detail?.subject == .work(nested) && $0.detail?.loading == false }
        }
        // Left on a leaf goes to its parent; left on an open package collapses it, and the selection stays on a hidden row.
        let full = state.rows
        outcome = await key(KeyDecoder.left)
        state = await now()
        check(outcome == .handled(.collapse) && state.focus == "gantt.row.TEST-PKG", "left arrow on a leaf returns to its package: \(state.focus ?? "none")")
        outcome = await key(KeyDecoder.left)
        state = await now()
        check(outcome == .handled(.collapse) && state.rows == full - 3 && state.focus == "gantt.row.TEST-PKG" && state.selection == .work(nested),
              "left arrow on an open package collapses it: its three children are hidden, the cursor stays, and the selection persists on the hidden row (\(state.rows) of \(full) rows)")
        outcome = await key(KeyDecoder.right)
        state = await now()
        check(outcome == .handled(.expand) && state.rows == full && state.selection == .work(nested), "right arrow on a collapsed package expands it and the selection is as it was")
        // Open Detail from the row, change the selection as a followed link does, and close: focus returns to the row.
        _ = await key(KeyDecoder.down)
        let rowBefore = (await now()).focus
        outcome = await key("\r")
        state = await now()
        check(outcome == .handled(.openDetail) && state.page == .detail && state.origin?.element == rowBefore && state.origin?.page == .gantt, "return opens Detail and it remembers the row that opened it: \(String(describing: state.origin))")
        if state.page == .detail {
            _ = try await modelExpect(model, "the opened row's detail", 30) { $0.detail?.loading == false && $0.detail != nil }
            await MainActor.run { model.select(.work(sibling)) }
        }
        outcome = await key("\u{1B}")
        state = await now()
        check(outcome == .handled(.closeDetail) && state.page == .gantt && state.focus == rowBefore, "escape closes Detail and returns focus to the row that opened it, after a followed link: page \(state.page), focus \(state.focus ?? "none")")
        check(state.selection == .work(sibling), "the selection is the followed link's, not reset by returning")
        // The schedule was not displayed while Detail was; once it is read again the cursor is still on the row.
        if state.page == .gantt { _ = try await modelExpect(model, "the rows to be read again after Detail closed", 30) { $0.gantt != nil } }
        state = await now()
        check(state.focus == rowBefore, "the cursor is still on the row that opened Detail once the rows are back: \(state.focus ?? "none")")
        // Zoom and pan: only the view changes.
        let zoom0 = state.zoom
        for _ in 0..<3 { _ = await key("=") }
        state = await now()
        let zoomed = state.zoom
        check(zoomed == min(GanttLayout.zoomSteps.count - 1, zoom0 + 3) && zoomed > zoom0, "the plus key zooms in by one level each press: \(zoom0) to \(zoomed)")
        _ = await key("-")
        state = await now()
        check(state.zoom == zoomed - 1, "the minus key zooms out")
        // Back to the origin first, so the pan has room to move on both axes whatever the cursor did to it.
        for _ in 0..<3 {
            _ = await key(KeyDecoder.up, shift: true)
            _ = await key(KeyDecoder.left, shift: true)
        }
        state = await now()
        let panBefore = (state.panX, state.panY)
        _ = await key(KeyDecoder.right, shift: true)
        _ = await key(KeyDecoder.down, shift: true)
        state = await now()
        check(state.panX > panBefore.0 && state.panY > panBefore.1, "shift with the arrows pans along both axes: \(panBefore) to \((state.panX, state.panY))")
        check(state.selection == .work(sibling), "zoom and pan left the selection alone")
        // Filter: critical only, status, clear; the selection survives a filter that hides its row.
        let all = state.rows
        outcome = await key("c")
        state = await now()
        check(outcome == .handled(.toggleCritical) && state.criticalOnly && state.rows <= all, "c toggles the critical-only filter, which keeps no more rows than there are (\(state.rows) of \(all))")
        outcome = await key("x")
        state = await now()
        check(outcome == .handled(.clearFilter) && !state.criticalOnly && state.rows == all, "x clears the filter and every row is back")
        outcome = await key("s")
        state = await now()
        check(outcome == .handled(.cycleStatus) && state.status != nil, "s cycles the status filter: \(state.status ?? "none")")
        _ = await key("x")
        outcome = await key("/")
        state = await now()
        check(outcome == .handled(.focusFilter) && state.focus == "gantt.filter" && state.editing, "slash moves focus to the filter field: \(state.focus ?? "none")")
        outcome = await key("c")
        state = await now()
        check(outcome == .ignored && !state.criticalOnly, "while the filter field has the keyboard, a letter is text and not a command")
        outcome = await key("\u{1B}")
        state = await now()
        check(outcome == .handled(.closeDetail) && state.focus?.hasPrefix("gantt.row.") == true && !state.editing, "escape leaves the filter field and focus returns to a row: \(state.focus ?? "none")")
        check(state.selection == .work(sibling), "the selection persisted through every filter")
        // Collapse and expand every package.
        _ = await key(",")
        let collapsed = (await now()).rows
        _ = await key(".")
        let reopened = (await now()).rows
        check(collapsed < reopened && reopened == full, "comma collapses every package and period expands them again: \(collapsed) then \(reopened) rows")
        // Nothing was written, as the real helper and the disk say.
        let after = try await readOnlyRecord(database)
        check(after == before, "no revision, history entry or store byte changed through any of it: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: Selection through page changes and reopening

    func ganttSelectionPersists() async throws {
        print("gantt: the one shared selection persists through page changes, collapse, filter and a reopened workspace")
        let database = try await newStore("gantt-selection", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database))
        let first = await MainActor.run { model.snapshot }
        guard let identity = identity(of: "TEST-B", in: first) else { throw SuiteError(description: "TEST-B has no identity") }
        await MainActor.run { model.focusRow(identity) }
        var state = await MainActor.run { snap(model) }
        check(state.selection == .work(identity) && state.focus == "gantt.row.TEST-B", "choosing a row selects it and puts the cursor on it")
        _ = try await modelExpect(model, "the detail of TEST-B", 30) { $0.detail?.subject == .work(identity) && $0.detail?.loading == false }
        for page in [ObserverModel.Page.now, .detail, .live, .gantt] {
            await MainActor.run { model.show(page) }
            state = await MainActor.run { snap(model) }
            check(state.selection == .work(identity), "the selection survives showing \(page.rawValue)")
        }
        _ = try await modelExpect(model, "the rows to be read again after the page returned", 30) { $0.gantt != nil }
        await MainActor.run {
            _ = self.press(model, ",")
            _ = self.press(model, "c")
        }
        state = await MainActor.run { snap(model) }
        check(state.selection == .work(identity), "collapsing every package and filtering to the critical path leave the selection")
        await MainActor.run { model.open(.database(database)) }
        _ = try await modelExpect(model, "the reopened workspace with the selection restored", 60) { $0.connection == .connected && $0.detail?.subject == .work(identity) && $0.detail?.loading == false && $0.gantt != nil }
        state = await MainActor.run { snap(model) }
        check(state.selection == .work(identity), "reopening the same workspace restores the selection by its persistent identity, with the Gantt read again")
        await closeGantt(model)
    }

    // MARK: The schedule is displayed only while the Gantt is shown

    func ganttScheduleLeavesTheDisplayedSet() async throws {
        print("gantt: the schedule is a displayed view, judged for freshness, only while the Gantt is the page shown")
        let database = try await newStore("gantt-slots", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let reads = Tally()
        try await withObserver(.database(database), clock: pinned, adjust: { settings in
            settings.fault = { query in
                if query["query"].string == "schedule" { reads.bump() }
                return nil
            }
        }) { engine, recorder, first in
            check(first.gantt == nil && !first.inspection.views.contains { $0.slot == "Schedule (Gantt)" } && reads.count == 0, "a page that is not the Gantt reads no schedule and displays none")
            await engine.showSchedule(true)
            let shown = try await recorder.expect("the schedule to be displayed") { $0.gantt != nil && $0.inspection.views.contains { $0.slot == "Schedule (Gantt)" } }
            check((shown.gantt?.rows.count ?? 0) > 0 && reads.count >= 1, "shown, the schedule is read and listed under the observation basis (\(shown.gantt?.rows.count ?? 0) rows, \(reads.count) reads)")
            let settled = try await recorder.expect("the client to be current with the schedule shown") { $0.gantt != nil && $0.freshness.current }
            check(settled.freshness.current, "with the schedule shown the client is current")
            await engine.showSchedule(false)
            let hidden = try await recorder.expect("the schedule to leave the displayed set") { $0.gantt == nil && !$0.inspection.views.contains { $0.slot == "Schedule (Gantt)" } }
            check(hidden.gantt == nil, "not shown, it leaves the displayed set")
            let beforeCommit = reads.count
            try await worker(database, ["claim", "TEST-A"])
            _ = try await recorder.expect("the external commit to be seen and read", 30) { ($0.revision ?? 0) > (first.revision ?? 0) && $0.freshness.current && $0.inventory.byIdentity.values.contains { $0.key == "TEST-A" && $0.status == "Claimed" } }
            check(reads.count == beforeCommit, "an external commit while the Gantt is hidden causes no schedule read (\(beforeCommit) reads before, \(reads.count) after)")
            await engine.showSchedule(true)
            let back = try await recorder.expect("the schedule to be read again at the new revision", 30) { $0.gantt != nil && $0.gantt?.revision == $0.revision }
            let claimed = back.gantt?.rows.first { $0.key == "TEST-A" }?.status
            check(claimed == "Claimed" && reads.count > beforeCommit, "shown again, it is read again and carries the committed change: TEST-A is \(claimed ?? "missing")")
        }
    }

    // MARK: Stale and disconnected, with the last view kept

    func ganttKeepsItsViewWhenStaleOrLost() async throws {
        print("gantt: a lost helper and a changed revision mark the data and keep the last view, the selection and the cursor")
        let database = try await newStore("gantt-stale", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database)) { $0.reconnectDelays = [0.4, 0.6] }
        let (rows, found) = await MainActor.run { () -> (Int, String?) in (model.outline.count, model.snapshot.inventory.items.first { $0.key == "TEST-B" }?.identity) }
        guard let identity = found else { throw SuiteError(description: "TEST-B has no identity") }
        await MainActor.run { model.focusRow(identity) }
        guard let pid = await MainActor.run(body: { model.snapshot.helperProcess }) else { throw SuiteError(description: "the snapshot names no helper process") }
        kill(pid, SIGKILL)
        let lost = try await modelExpect(model, "the lost helper to be shown", 30) { if case .reconnecting = $0.connection { return true } else { return false } }
        check(lost.gantt != nil && lost.gantt?.rows.count == rows && !lost.freshness.current, "while the helper is lost the last schedule is still held, and the client is not current (\(lost.gantt?.rows.count ?? -1) rows)")
        var state = await MainActor.run { snap(model) }
        check(state.rows == rows && state.selection == .work(identity) && state.focus == "gantt.row.TEST-B", "the drawn rows, the selection and the cursor are kept through the loss")
        _ = try await modelExpect(model, "the helper to be replaced and the view to be current", 60) { $0.connection == .connected && $0.freshness.current && $0.gantt != nil && $0.counters.reconnects >= 1 }
        state = await MainActor.run { snap(model) }
        check(state.rows == rows && state.selection == .work(identity), "after recovery the marks clear and the rows and selection are as they were")
        try await worker(database, ["claim", "TEST-A"])
        let changed = try await modelExpect(model, "the claimed task to appear in the schedule", 30) { $0.gantt?.rows.first { $0.key == "TEST-A" }?.status == "Claimed" && $0.freshness.current }
        check(changed.gantt?.revision == changed.revision, "an external commit is followed by a schedule read at the same revision")
        await closeGantt(model)
    }

    // MARK: A dense plan and a 1000-task plan

    func ganttDenseAndLargePlans() async throws {
        print("gantt: a 1000-task plan with the dense-DAG overlay stays complete and operable")
        let database = try await newStore("gantt-dense", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        let before = try await readOnlyRecord(database)
        let (found, inventory, outline) = await MainActor.run { (model.snapshot.gantt, model.snapshot.inventory, model.outline) }
        guard let schedule = found else { throw SuiteError(description: "no schedule was read") }
        let packages = schedule.rows.filter(\.isPackage).count
        check(schedule.rows.count == 1100 && packages == 100, "1000 tasks and 100 packages are 1100 rows: \(schedule.rows.count) rows, \(packages) packages")
        let soft = inventory.relations.filter { $0.policy == "Soft" }.count
        check(inventory.relations.count == 1994 && soft == 249, "the overlay's edges are all here: \(inventory.relations.count) relations, \(soft) Soft")
        let incoming = Dictionary(grouping: inventory.relations, by: \.successor).mapValues(\.count)
        check(incoming.count == 997 && incoming.values.allSatisfy { $0 == 2 }, "997 tasks have two predecessors each, as the dense overlay defines (\(incoming.count) tasks have any)")
        check(outline.count == 1100 && outline.filter { $0.depth == 1 }.count == 1000, "expanded, every package and task is a row, with the tasks one level in: \(outline.count)")
        if let item = outline.first(where: { $0.depth == 1 && inventory.relations(of: $0.id).count >= 3 }) {
            let said = GanttWords.row(item, schedule: schedule, inventory: inventory, selected: nil)
            check(said.value.contains("relations: ") && said.value.components(separatedBy: " → ").count >= 3, "a task in the dense graph reads its relations in words: \(said.value.suffix(200))")
        } else {
            check(false, "no task with several relations was found in the dense plan")
        }
        // Operate it by keys: collapse everything, page through, jump to the end, expand.
        let collapsed = await MainActor.run { () -> Int in
            model.setViewport(rows: 20, width: 600)
            _ = self.press(model, ",")
            return model.outline.count
        }
        check(collapsed == 100, "collapsing every package leaves the 100 packages: \(collapsed)")
        let (paged, last) = await MainActor.run { () -> (String?, String?) in
            _ = self.press(model, KeyDecoder.pageDown)
            let paged = model.focusTarget
            _ = self.press(model, KeyDecoder.end)
            return (paged, model.focusTarget)
        }
        check(paged == "gantt.row.PERF-P20" && last == "gantt.row.PERF-P100", "page down moves one page of rows and end goes to the last: \(paged ?? "none"), \(last ?? "none")")
        let expanded = await MainActor.run { () -> Int in
            _ = self.press(model, ".")
            return model.outline.count
        }
        check(expanded == 1100, "expanding every package restores all 1100 rows: \(expanded)")
        // F20-UI-02, additive: after collapse-all, End, expand-all the cursor row is inside the visible range and among
        // the rows the view renders (the same range `GanttGeometry` gives the labels that carry the accessibility text:
        // from Int(panY / rowHeight), viewport rows + 2). A model-path check; the hosted view is not instantiated here.
        let (cursorIndex, firstRendered, renderedCount, visibleRows) = await MainActor.run { () -> (Int, Int, Int, Int) in
            let at = model.gantt.focus.flatMap { id in model.outline.firstIndex { $0.id == id } } ?? -1
            let first = Int(max(0, model.gantt.panY) / GanttLayout.rowHeight)
            let drawn = max(0, min(model.outline.count - first, model.gantt.viewportRows + 2))
            return (at, first, drawn, model.gantt.viewportRows)
        }
        check(cursorIndex >= 0 && cursorIndex >= firstRendered && cursorIndex < firstRendered + min(renderedCount, visibleRows),
              "after collapse-all, End and expand-all the cursor row (index \(cursorIndex)) is inside the visible rows \(firstRendered)..<\(firstRendered + visibleRows) and among the \(renderedCount) rendered rows")
        let (criticalRows, sound) = await MainActor.run { () -> (Int, Bool) in
            _ = self.press(model, "c")
            let rows = model.outline
            let parents = Set(rows.compactMap(\.row.parent))
            return (rows.count, rows.allSatisfy { $0.row.span?.critical == true || parents.contains($0.id) })
        }
        check(criticalRows <= 1100 && sound, "critical-only keeps the critical rows and the packages that contain them (\(criticalRows) rows)")
        let after = try await readOnlyRecord(database)
        check(after == before, "none of it wrote anything: before \(before), after \(after)")
        await closeGantt(model)
    }

    // MARK: The filter field's focus (model path only)

    /// F20-UI-01. MODEL-PATH test: it drives `filterFieldFocus` and `handleKey` on the model. It is NOT real keyboard
    /// coverage: no hosted SwiftUI view, key window, event monitor or `FocusState` transition is involved; that
    /// real-path regression is NOT ASSESSED here.
    func ganttFilterFocusLossDoesNotRequestTheRows() async throws {
        print("gantt: [model path] leaving the filter field without Escape names no row; Escape and a commit still do")
        let database = try await newStore("gantt-filter-focus", plan: try requirePlan(tools.densePlan, "--dense-plan"))
        let model = try await openGanttModel(.database(database))
        let (typing, left, afterLoss) = await MainActor.run { () -> (String?, String?, String?) in
            _ = self.press(model, "/")
            let typing = model.focusTarget
            model.filterFieldFocus(false)   // Tab or Shift-Tab: focus lost, not Escape, not a commit
            return (typing, model.focusTarget, model.focusTarget)
        }
        check(typing == "gantt.filter" && left == nil && afterLoss == nil && !(left ?? "").hasPrefix("gantt.row."), "focus lost from the filter by Tab names no row: \(typing ?? "none") then \(left ?? "none")")
        let escaped = await MainActor.run { () -> String? in
            _ = self.press(model, "/")
            _ = self.press(model, "\u{1B}")
            return model.focusTarget
        }
        check(escaped?.hasPrefix("gantt.row.") == true, "Escape from the filter returns the keyboard to the cursor row: \(escaped ?? "none")")
        await closeGantt(model)
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

    // MARK: The measurement instrument refuses to complete a withheld draw

    func ganttMeasurementNegativeControl() async throws {
        print("gantt: the measurement log completes a generation only when it is drawn, and refuses when the draw is withheld")
        let directory = scratch("measure-control")
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        func events(_ url: URL) throws -> [[String: Any]] {
            try String(contentsOf: url, encoding: .utf8).split(separator: "\n").compactMap { try? JSONSerialization.jsonObject(with: Data($0.utf8)) as? [String: Any] }
        }
        func scenario(_ name: String, frozen: Set<String>) throws -> [[String: Any]] {
            let url = directory.appendingPathComponent("\(name).jsonl")
            guard let log = MeasureLog(path: url.path, id: "control-\(name)", freeze: frozen, header: ["hardware_model": "suite"]) else { throw SuiteError(description: "the log could not be opened") }
            for number in 0..<20 { log.emit("tick", detail: ["n": number]) }
            log.expect("view_op:1", facts: ["sequence": "1", "rows": "5"])
            log.emit("view_op_call", gen: 1, detail: ["op": "collapse_all", "expected_rows": 5])
            log.emit("state_set", gen: 1, detail: ["kind": "view_op"])
            // A draw of other content (an older state) completes nothing, whatever the gate.
            log.stamp("view_op_drawn", key: "view_op:1", gen: 1, facts: ["sequence": "0", "rows": "5"], surface: "gantt")
            log.stamp("view_op_drawn", key: "view_op:1", gen: 1, facts: ["sequence": "1", "rows": "5"], surface: "gantt", detail: ["rows_drawn_model": 5])
            log.stamp("first_draw", key: "never-expected", gen: "x", facts: [:], surface: "gantt")
            log.close()
            return try events(url)
        }
        let open = try scenario("open", frozen: [])
        let withheld = try scenario("withheld", frozen: ["gantt"])
        func count(_ list: [[String: Any]], _ name: String) -> Int { list.filter { $0["event"] as? String == name }.count }
        check(count(open, "view_op_drawn") == 1 && count(open, "view_op_call") == 1 && count(open, "tick") == 20 && count(open, "first_draw") == 0, "NC-0, the gate open: the generation completes once, and only for the content it expected")
        let drawn = open.first { $0["event"] as? String == "view_op_drawn" }
        let called = open.first { $0["event"] as? String == "view_op_call" }
        let drawnAt = drawn?["t_ns"] as? Int ?? 0
        let calledAt = called?["t_ns"] as? Int ?? Int.max
        check(drawnAt > calledAt && (drawn?["detail"] as? [String: Any])?["rows_drawn_model"] as? Int == 5, "its latency exists, is positive, and was measured on the app's own monotonic clock (\(drawnAt - calledAt) ns)")
        check(count(withheld, "view_op_drawn") == 0 && count(withheld, "first_draw") == 0, "NC-1, the gate closed: no drawn event exists")
        check(count(withheld, "tick") == 20 && count(withheld, "state_set") == 1 && count(withheld, "view_op_call") == 1, "yet the callbacks fired and the model advanced: 20 ticks, a state_set and the call are all there")
        let closing = withheld.last
        check(closing?["event"] as? String == "log_closed" && closing?["dropped"] as? Int == 0, "the log says how many events it dropped: none")
        // A full buffer drops and says so: such a log is invalid and never short in silence.
        let url = directory.appendingPathComponent("full.jsonl")
        guard let full = MeasureLog(path: url.path, id: "control-full", freeze: [], header: [:], capacity: 0) else { throw SuiteError(description: "the log could not be opened") }
        for number in 0..<10 { full.emit("tick", detail: ["n": number]) }
        full.close()
        let dropped = try events(url).last
        check(dropped?["event"] as? String == "log_closed" && dropped?["dropped"] as? Int == 10, "events beyond the bounded buffer are counted as dropped (\(dropped?["dropped"] as? Int ?? -1)), which makes a run invalid")
        // The model's own events: a collapse advances the model and logs, and completes nothing without a draw.
        let database = try await newStore("gantt-measure", plan: try requirePlan(tools.ganttPlan, "--gantt-plan"))
        let model = try await openGanttModel(.database(database))
        let modelLog = directory.appendingPathComponent("model.jsonl")
        guard let live = MeasureLog(path: modelLog.path, id: "control-model", freeze: [], header: [:]) else { throw SuiteError(description: "the log could not be opened") }
        MeasureLog.shared = live
        let sequence = await MainActor.run { () -> Int in
            _ = self.press(model, ",")
            _ = self.press(model, " ")
            return model.gantt.sequence
        }
        MeasureLog.shared = nil
        live.close()
        let seen = try events(modelLog)
        let completed = live.isComplete("view_op:\(sequence)")
        check(count(seen, "view_op_call") == 1 && count(seen, "state_set") >= 1 && count(seen, "select_call") == 1 && count(seen, "view_op_drawn") == 0 && !completed,
              "the model logs a view operation and a selection as called and applied, and with no draw pass neither is ever completed")
        await closeGantt(model)
    }
}
