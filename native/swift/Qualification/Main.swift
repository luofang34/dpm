// The integration suite for the native bridge: the real helper, the real library and the real CLI,
// each as its own process, against disposable synthetic stores. It is built by scripts/build_native.py
// and run by scripts/smoke_native.py, which supplies the tools it needs.
//
// Usage: qualify --dpm PATH --helper PATH --host PATH --plan PATH --lagged-plan PATH --proxy PATH
//                --scratch DIR --clock RFC3339 [--only NAME]
// Exit status: 0 when every check held, 1 when one did not, 2 for usage.

import DPMNative
import DPMObserverCore
import Foundation

@main
struct Qualify {
    static func main() async {
        var tools = Tools()
        var only: String?
        var arguments = CommandLine.arguments.dropFirst().makeIterator()
        while let flag = arguments.next() {
            guard let value = arguments.next() else { usage("\(flag) needs a value") }
            switch flag {
            case "--dpm": tools.dpm = value
            case "--helper": tools.helper = value
            case "--host": tools.host = value
            case "--plan": tools.plan = value
            case "--lagged-plan": tools.laggedPlan = value
            case "--proxy": tools.proxy = value
            case "--scratch": tools.scratch = value
            case "--clock": tools.clock = value
            case "--writer": tools.writer = value
            case "--gantt-plan": tools.ganttPlan = value
            case "--calendar-plan": tools.calendarPlan = value
            case "--dense-plan": tools.densePlan = value
            case "--long-plan": tools.longPlan = value
            case "--network-plan": tools.networkPlan = value
            case "--only": only = value
            default: usage("unknown option \(flag)")
            }
        }
        let required = [tools.dpm, tools.helper, tools.host, tools.plan, tools.laggedPlan, tools.proxy, tools.scratch, tools.clock]
        if required.contains("") { usage("every tool path, --scratch and --clock are required") }

        let monitor = MainThreadMonitor()
        monitor.start()
        let context = Context(tools: tools, monitor: monitor)
        let scenarios: [(String, () async throws -> Void)] = [
            ("launch", context.launchAndNegotiation),
            ("contract", context.contractThroughTheBridge),
            ("bounded-consumer", context.boundedConsumer),
            ("parity", context.payloadParity),
            ("host", context.packagedHost),
            ("refusals", context.typedRefusals),
            ("cancellation", context.cancellationAndPreemption),
            ("crash", context.crashExitAndReconnect),
            ("window", context.windowCleanup),
            ("cycles", context.repeatedLifecycles),
            ("open-cancel", context.cancelledOpenings),
            ("command-faults", context.commandFaults),
            ("deadline", context.deadlineIsAbsolute),
            ("behind-failure", context.queuedBehindFailure),
            ("admission", context.boundedAdmission),
            ("generation", context.queuedCommandStaysBehind),
            ("validation", context.configurationAndSizes),
            ("consistency", context.negotiatedConsistency),
            ("responsive", context.mainThreadStaysResponsive),
            ("observer-startup", context.observerStartup),
            ("observer-completion", context.observerCompletionIsNotVerification),
            ("observer-latency", context.observerExternalCommitLatency),
            ("observer-race", context.observerStartupRace),
            ("observer-clock", context.observerClockOnlyGate),
            ("observer-stale", context.observerReportedOnlyAndStale),
            ("observer-rename", context.observerFollowsRenamedKeys),
            ("observer-refused", context.observerRefusedRefreshIsRetried),
            ("observer-duplicates", context.observerDuplicates),
            ("observer-reconnect", context.observerLostHelperReconnects),
            ("observer-source", context.observerSourceChange),
            ("observer-helper-fault", context.observerHelperFailure),
            ("observer-burst", context.observerBurst),
            ("observer-model", context.observerModelRules),
            ("observer-decisions", context.observerReachesBlockedAndUnrunWork),
            ("observer-coverage", context.observerBoundedCoverage),
            ("observer-gap-startup", context.observerRetentionGapAtStartup),
            ("observer-gap-reconnect", context.observerRetentionGapAfterReconnect),
            ("observer-pending-open", context.observerPendingOpenIsEnded),
            ("observer-state-writer", context.observerStateWriterIsBounded),
            ("observer-slots", context.observerSelectionOwnedViewsLeave),
            ("observer-page-caps", context.observerPageCapsAreSaid),
            ("observer-window-race", context.observerWindowTotalAfterRacingWrite),
            ("gantt-rows", context.ganttRowsEqualTheQuery),
            ("gantt-relations", context.ganttRelationsAndMilestonesAreText),
            ("gantt-dates", context.ganttDatesOnlyWhereSupplied),
            ("gantt-keys", context.ganttKeysFlow),
            ("gantt-selection", context.ganttSelectionPersists),
            ("gantt-slots", context.ganttScheduleLeavesTheDisplayedSet),
            ("gantt-stale", context.ganttKeepsItsViewWhenStaleOrLost),
            ("gantt-dense", context.ganttDenseAndLargePlans),
            ("gantt-filter-focus-model-path", context.ganttFilterFocusLossDoesNotRequestTheRows),
            ("gantt-long-titles", context.ganttLongTitlesStayWhole),
            ("gantt-preview", context.ganttPreviewIsReadOnly),
            ("gantt-measurement-control", context.ganttMeasurementNegativeControl),
            ("gantt-schedule-debt", context.ganttScheduleDebtSurvivesATransientFailure),
            ("gantt-view-op-entry", context.ganttViewOperationIsLoggedOnEntry),
            ("gantt-estimates", context.ganttEstimatesArePresented),
            ("gantt-scroll-identity", context.ganttScrollTrajectoryAndIdentity),
            ("gantt-content-facts", context.ganttContentFactsAreCompared),
            ("gantt-pan-model-path", context.ganttPanSurvivesAnOffscreenCursor),
            ("gantt-axis-labels", context.ganttAxisLabelsDoNotCollide),
            ("network-content", context.networkContentIsTheApplications),
            ("network-projection", context.networkListAndCanvasShareOneProjection),
            ("network-entry-walk", context.networkEntryWalkAndDetail),
            ("network-filter-focus-model-path", context.networkFilterFocusIsTheFields),
            ("network-stale", context.networkKeepsItsViewWhenLost),
            ("network-dense", context.networkDenseGraph),
            ("network-long-titles", context.networkLongTitlesStayWhole),
            ("network-fit-whole", context.networkFitIsWhole),
            ("network-hover-locate-labels", context.networkHoverLocateAndLabels),
            ("network-skip-edge-routes", context.networkSkipEdgeIsRouted),
            ("gantt-fit-whole", context.ganttFitIsWhole),
            ("gantt-locate-ancestors", context.ganttLocateRevealsOnlyAncestors),
            ("gantt-relation-labels", context.ganttRelationLabelsDoNotOverprint),
            ("network-walked-text", context.networkWalkedTextFollowsEachStep),
        ]
        // A control that must fail: it waits for something that never happens. It runs only when asked
        // for by name, and the packaged qualification requires it to exit nonzero.
        let controls: [(String, () async throws -> Void)] = [("observer-negative-control", context.observerNegativeControl), ("gantt-negative-control", context.ganttNegativeControl)]
        let runnable = only == nil ? scenarios : (scenarios + controls).filter { $0.0 == only }
        for (name, run) in runnable {
            do { try await run() } catch { context.checks.check(false, "\(name) stopped early: \(error)") }
        }
        let offMain = context.audit.mainThreadSteps.isEmpty
        context.checks.check(offMain, "no blocking step ran on the main thread (\(context.audit.count) steps seen, on main: \(context.audit.mainThreadSteps))")
        let snapshot = monitor.snapshot
        print("main queue: longest gap since the last window \(Int(snapshot.maxGapMilliseconds)) ms")
        print("\(context.checks.passed) checks passed, \(context.checks.failures.count) failed")
        for failure in context.checks.failures { print("FAILED: \(failure)") }
        exit(context.checks.failures.isEmpty ? 0 : 1)
    }

    static func usage(_ message: String) -> Never {
        FileHandle.standardError.write(Data("qualify: \(message)\n".utf8))
        exit(2)
    }
}
