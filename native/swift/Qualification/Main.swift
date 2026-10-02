// The integration suite for the native bridge: the real helper, the real library and the real CLI,
// each as its own process, against disposable synthetic stores. It is built by scripts/build_native.py
// and run by scripts/smoke_native.py, which supplies the tools it needs.
//
// Usage: qualify --dpm PATH --helper PATH --host PATH --plan PATH --lagged-plan PATH --proxy PATH
//                --scratch DIR --clock RFC3339 [--only NAME]
// Exit status: 0 when every check held, 1 when one did not, 2 for usage.

import DPMNative
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
        ]
        for (name, run) in scenarios where only == nil || only == name {
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
