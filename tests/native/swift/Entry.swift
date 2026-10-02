// The entry point: the main thread only ticks and waits in its run loop, as a UI host's does, and
// everything else runs off it.

import Foundation

@main
struct Proof {
    static func main() {
        // Progress lines reach a pipe as they happen, so a hung proof shows where it stopped.
        setvbuf(stdout, nil, _IOLBF, 0)
        let arguments = Arguments(CommandLine.arguments)
        let monitor = MainThreadMonitor()
        monitor.start()
        Task {
            let failed = await Scenarios(arguments: arguments, monitor: monitor).run()
            exit(failed ? 1 : 0)
        }
        // A run loop with a source, which is the model an app's main thread follows.
        RunLoop.main.add(Timer(timeInterval: 3600, repeats: true) { _ in }, forMode: .common)
        RunLoop.main.run()
    }
}
