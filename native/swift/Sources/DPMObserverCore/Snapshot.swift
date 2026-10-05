// What the observer shows at one moment, as one immutable value.
//
// The engine owns every blocking step and every decision about when to read again; the interface
// receives these values and presents them. Staleness, disconnection and source changes are part of
// the value, so no view can show data without also showing how old or uncertain it is.

import DPMNative
import Foundation

/// The boundary's instant, named here so interface code need not import the client library, whose
/// `View` would collide with SwiftUI's.
public typealias ObserverInstant = Instant

/// What an operator selected, by the persistent identity that survives every refresh: a task's
/// identity (its key may be renamed by a reviewed change) or a run's.
public enum Subject: Hashable, Sendable {
    case work(String)
    case run(String)
    /// A decision, by its identity: what it asks, what it gates, and the work it holds back.
    case decision(String)
}

/// How the connection to the application stands. Data is kept through every state but `idle`.
public enum ConnectionState: Equatable, Sendable {
    case idle
    case opening
    case connected
    /// The helper was lost; the engine is replacing it and the data shown is as old as the loss.
    case reconnecting(attempt: Int, cause: String)
    /// The locator now selects another source. Following stopped; nothing of the new source is shown.
    case sourceChanged(String)
    /// The workspace could not be opened, or the bridge cannot continue.
    case failed(String)
    case closed

    public var isLive: Bool { self == .connected }
}

/// How current what is displayed is. All of it is text-able: nothing relies on colour.
public struct Freshness: Equatable, Sendable {
    /// The one clock reading the time-dependent values were evaluated at.
    public var evaluatedAt: Instant?
    /// When this app last installed a refreshed view.
    public var receivedAt: Date?
    /// A project change was seen that the displayed views do not yet hold.
    public var projectStale = false
    /// A run change was seen that the displayed runs do not yet hold.
    public var runsStale = false
    /// The instant the application said the displayed answers change by themselves has passed.
    public var clockDue = false
    /// More feed pages are waiting than one pass reads.
    public var catchingUp = false
    /// The last failure of a read, kept until a later read succeeds.
    public var lastError: String?

    public var current: Bool { !projectStale && !runsStale && !clockDue && lastError == nil }
}

/// One titled block of readable text in a detail.
public struct DetailSection: Identifiable, Hashable, Sendable {
    public var id: String { title }
    public let title: String
    public let rows: [DetailRow]
}

public struct DetailRow: Identifiable, Hashable, Sendable {
    public let id: Int
    public let label: String?
    public let text: String
}

/// One task or run in full, as readable sections.
public struct SubjectDetail: Equatable, Sendable {
    public var subject: Subject
    public var title: String
    public var subtitle: String
    public var sections: [DetailSection]
    public var revision: UInt64?
    public var loading: Bool
    public var error: String?
}

/// A selected run's bounded activity window and lifecycle.
public struct RunWindow: Equatable, Sendable {
    public var run: String
    public var entries: [ActivityEntry]
    public var lifecycle: [LifecycleEntry]
    /// Records the application says it holds for this run, however few are shown.
    public var recorded: Int
    /// Entries older than the window kept were dropped from view: how many.
    public var droppedFromView: Int
    /// Older lifecycle entries not kept: how many.
    public var lifecycleDropped: Int
    /// Retention outran the cursor: the activity feed said so and went on.
    public var gap: String?
    /// The paging bound ended the read before the run's activity was exhausted.
    public var incomplete: String?
    /// The same for the lifecycle: what is shown is the part that was read, not the newest.
    public var lifecycleIncomplete: String?
    /// The feed position of the newest entry applied; a repeat is discarded by it.
    var last: UInt64
}

/// How much of a list the observer read. A list that reached its limit may have more behind it, so
/// nothing absent from it is claimed to be absent.
public struct Coverage: Equatable, Sendable {
    public var read = 0
    public var limit = 0

    public var truncated: Bool { limit > 0 && read >= limit }

    public var words: String {
        truncated ? "the newest \(read) read; older ones were not read" : "all \(read) read"
    }
}

/// The runs of one task, read by the per-task runs query, with how complete that reading is.
public struct WorkRuns: Equatable, Sendable {
    public var work: String
    public var runs: [RunSummary]
    public var coverage: Coverage
}

/// Where one displayed view was anchored, for the observation-basis disclosure.
public struct ViewBasisRow: Equatable, Sendable, Identifiable {
    public var id: String { slot }
    public let slot: String
    public let evaluatedAt: ObserverInstant
    public let refreshAt: ObserverInstant?
    public let project: String?
    public let runs: String?
}

/// The positions the observer follows and the basis of every view on display: what a person needs
/// to tell differently anchored answers apart. Technical, so it sits behind a disclosure.
public struct Inspection: Equatable, Sendable {
    public var views: [ViewBasisRow] = []
    public var cursors: [String] = []
}

/// What the observer has done, for diagnostics and tests; nothing decides from it.
public struct Counters: Equatable, Sendable {
    public var polls = 0
    public var projectReads = 0
    public var runReads = 0
    public var timeReads = 0
    public var activityApplied = 0
    public var activityDiscarded = 0
    public var projectOperations = 0
    public var resets = 0
    public var gaps = 0
    public var reconnects = 0
}

public struct Unsupported: Equatable, Sendable {
    public let control: String
    /// The application's own wording, which may name internal phases; kept for diagnostics.
    public let reason: String

    /// What a person is told: what this app does not do, plainly, and nothing about phases.
    public var plain: String {
        switch control {
        case "steer_run": return "Steering a run, which means sending it new instructions, is not available in this app."
        case "stop_run": return "Stopping a run is not available in this app."
        case "answer_input_request": return "Answering or approving what a run asks is not available in this app. What a run asked is shown as the run recorded it."
        case "provider_session_control": return "Controlling a provider's session is not available in this app."
        case "push_events": return "Updates are read by asking the application at short intervals; nothing is pushed to this app."
        case "remote_access": return "Only a workspace on this Mac can be observed; remote access is not available."
        default: return "\(control.replacingOccurrences(of: "_", with: " ")) is not available in this app."
        }
    }
}

/// Everything the interface may show.
public struct ObserverSnapshot: Equatable, Sendable {
    /// Which connection this belongs to; an answer of another generation is never installed.
    public var generation = 0
    public var connection = ConnectionState.idle
    public var identity: Identity?
    public var revision: UInt64?
    public var freshness = Freshness()
    public var status: StatusSummary?
    public var candidates: [Candidate] = []
    public var runs: [RunSummary] = []
    public var inventory = Inventory.empty
    public var operations: [OperationEntry] = []
    public var detail: SubjectDetail?
    public var window: RunWindow?
    public var unsupported: [Unsupported] = []
    public var reportedOnlyNote = ""
    /// How much of the application's newest runs the global list holds.
    public var runsCoverage = Coverage()
    /// The selected task's own runs, read for it and not taken from the global list.
    public var workRuns: WorkRuns?
    /// The selected run, read by identity: it stays inspectable when it leaves the global list.
    public var runDetail: RunSummary?
    public var inspection = Inspection()
    /// The shared schedule projection, held only while the Gantt is the page shown.
    public var gantt: GanttSchedule?
    /// The helper's process identifier, for measurement and fault injection; never decided from.
    public var helperProcess: Int32?
    public var counters = Counters()
    /// Strictly increasing with every published change, so a consumer can tell a repeat from news.
    public var sequence: UInt64 = 0

    public init() {}

    public var isPreview: Bool { identity?.source == "preview" }
}
