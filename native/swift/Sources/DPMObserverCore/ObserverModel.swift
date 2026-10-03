// The interface's one model: a main-actor projection of the engine's snapshots plus what only a
// person decides, which page is shown and what is selected.
//
// It owns no rule. Readiness, ranking, freshness and every refusal come through the snapshot, and
// the model has no way to write to a project: the engine it holds can only read. It lives with the
// engine, not with the views, so the ordering and selection rules below are exercised by the same
// suite that exercises the engine.

import Combine
import DPMNative
import Foundation

@MainActor
public final class ObserverModel: ObservableObject {
    public enum Page: String, CaseIterable, Identifiable, Sendable {
        case now = "Now", live = "Live", review = "Review", detail = "Detail"
        public var id: String { rawValue }
    }

    @Published public private(set) var snapshot = ObserverSnapshot()
    @Published public var page = Page.now
    @Published public private(set) var selection: Subject?
    @Published public private(set) var setupError: String?
    /// When the displayed snapshot was installed, for the freshness line and for measurement.
    @Published public private(set) var installedAt = Date()
    /// Raised when the operator asks to find work, so the search field takes focus.
    @Published public private(set) var searchRequest = 0
    /// Whether the search field has keyboard focus now, as the field itself reports it.
    @Published public private(set) var searchFocused = false
    private var searchFocusPending = false

    public private(set) var engine: ObserverEngine?
    private var chosen: WorkspaceSelection?
    /// The newest snapshot accepted, whether installed or still waiting for its turn. Anything
    /// older than it, from an earlier generation or an earlier publication, is dropped.
    private var newest: (generation: Int, sequence: UInt64)?
    private var pending: ObserverSnapshot?
    private var lastFlush = Date.distantPast
    private var flushScheduled = false
    private var selectKeyPending: String?
    private var selectRunPending: String?
    private var selectDecisionPending: String?
    /// The generation whose selection was last handed to the engine, so a reopened connection gets
    /// the operator's selection when it becomes connected, and not before.
    private var restoredGeneration = -1
    /// At most this often is the view invalidated, however fast the engine publishes.
    public var minimumInterval: TimeInterval = 0.05
    /// Called after every installed snapshot, on the main actor; used by the state reporter.
    public var onInstalled: (@MainActor () -> Void)?

    public init() {}

    /// Give the model its engine. With a setup error instead, the model only shows the error.
    public func configure(settings: ObserverEngine.Settings?, problem: String?, page: Page? = nil, selectKey: String? = nil, selectRun: String? = nil, selectDecision: String? = nil) {
        setupError = problem
        if let page = page { self.page = page }
        selectKeyPending = selectKey
        selectRunPending = selectRun
        selectDecisionPending = selectDecision
        guard let settings = settings else { return }
        engine = ObserverEngine(settings: settings) { [weak self] snapshot in
            Task { @MainActor in self?.receive(snapshot) }
        }
    }

    public var hasWorkspace: Bool {
        switch snapshot.connection {
        case .idle, .closed: return false
        default: return true
        }
    }

    // MARK: Operations

    public func open(_ choice: WorkspaceSelection) {
        guard let engine = engine else { return }
        if !Self.same(choice, chosen) { selection = nil }
        chosen = choice
        Task { await engine.open(choice) }
    }

    public func close() {
        guard let engine = engine else { return }
        selection = nil
        chosen = nil
        Task { await engine.close() }
    }

    /// Read everything again; from a lost or changed source, open it again. Never a mutation.
    public func reload() {
        guard let engine = engine else { return }
        Task { await engine.reload() }
    }

    public func select(_ subject: Subject?) {
        guard subject != selection else { return }
        selection = subject
        guard let engine = engine else { return }
        Task { await engine.select(subject) }
    }

    public func show(_ page: Page) { self.page = page }

    /// Go to the search over every task and decision, with the cursor in it. The request is kept
    /// until the search field takes it, so it is not lost when the field is created by this very
    /// change of page.
    public func findWork() {
        page = .detail
        searchFocusPending = true
        searchRequest &+= 1
    }

    /// Whether a request for focus is waiting; asking takes it, so one request focuses once.
    public func consumeSearchFocus() -> Bool {
        defer { searchFocusPending = false }
        return searchFocusPending
    }

    /// The search field reports when it gains or loses keyboard focus.
    public func searchFocus(_ focused: Bool) { searchFocused = focused }

    public func openDetail(for subject: Subject) {
        select(subject)
        page = .detail
    }

    static func same(_ left: WorkspaceSelection, _ right: WorkspaceSelection?) -> Bool {
        guard let right = right else { return false }
        switch (left, right) {
        case (.project(let a), .project(let b)), (.database(let a), .database(let b)), (.discover(let a), .discover(let b)): return a == b
        default: return false
        }
    }

    // MARK: Receiving snapshots

    /// Accept a publication unless something newer was already accepted. Callbacks can arrive out of
    /// order, so the comparison is against the newest accepted, installed or not.
    public func receive(_ new: ObserverSnapshot) {
        if let seen = newest {
            if new.generation < seen.generation { return }
            if new.generation == seen.generation, new.sequence <= seen.sequence { return }
        }
        newest = (new.generation, new.sequence)
        pending = new
        schedule()
    }

    private func schedule() {
        guard !flushScheduled else { return }
        flushScheduled = true
        let wait = max(0, minimumInterval - Date().timeIntervalSince(lastFlush))
        Task { @MainActor [weak self] in
            if wait > 0 { try? await Task.sleep(nanoseconds: UInt64(wait * 1_000_000_000)) }
            self?.flush()
        }
    }

    /// Install the newest accepted snapshot now.
    public func flush() {
        flushScheduled = false
        guard let new = pending else { return }
        pending = nil
        snapshot = new
        lastFlush = Date()
        installedAt = lastFlush
        restoreSelection(in: new)
        if let key = selectKeyPending, let item = new.inventory.items.first(where: { $0.key == key }) {
            selectKeyPending = nil
            select(.work(item.identity))
        }
        if let key = selectDecisionPending, let decision = new.inventory.decision(key: key) {
            selectDecisionPending = nil
            select(.decision(decision.identity))
        }
        if let run = selectRunPending, new.runs.contains(where: { $0.identity == run }) {
            selectRunPending = nil
            select(.run(run))
        }
        onInstalled?()
    }

    /// A connection starts with nothing selected. The operator's selection, kept by persistent
    /// identity, is handed to it once it is connected, once per generation, wherever the earlier
    /// snapshots of that generation were (opening, failed or reconnecting).
    private func restoreSelection(in new: ObserverSnapshot) {
        guard let held = selection, new.connection == .connected, new.detail == nil, restoredGeneration != new.generation, let engine = engine else { return }
        restoredGeneration = new.generation
        Task { await engine.select(held) }
    }
}
