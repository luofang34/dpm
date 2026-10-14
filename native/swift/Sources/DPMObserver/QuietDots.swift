// A quiet sign that a run reports working: three small dots rising and fading in turn. It is decoration only,
// hidden from assistive technology and always shown beside words that say the same ("Working"); under reduced
// motion, and while a scripted measurement runs, the three dots stand still.

import DPMObserverCore
import SwiftUI

struct QuietDots: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion

    static let size = 3.0
    static let spacing = 3.5
    static let period = 1.9
    static let delays = [0.0, 0.15, 0.3]

    var body: some View {
        Group {
            if reduceMotion || MeasureLog.shared != nil {
                dots { _ in (opacity: 1, offset: 0) }
            } else {
                TimelineView(.animation) { context in
                    let now = context.date.timeIntervalSinceReferenceDate
                    dots { index in Self.pose(at: now - Self.delays[index]) }
                }
            }
        }
        .frame(height: Self.size + 6)
        .accessibilityHidden(true)
    }

    private func dots(_ pose: @escaping (Int) -> (opacity: Double, offset: Double)) -> some View {
        HStack(spacing: Self.spacing) {
            ForEach(0..<3, id: \.self) { index in
                let now = pose(index)
                Circle()
                    .frame(width: Self.size, height: Self.size)
                    .opacity(now.opacity)
                    .offset(y: now.offset)
            }
        }
        .foregroundStyle(.secondary)
    }

    /// One dot's opacity and vertical offset `time` seconds into the loop. Keyframes, as fractions of the period: hidden
    /// 2 points low until 8%, risen 2.5 points and opaque at 22%, settled from 36% to 70%, faded out by 90%.
    static func pose(at time: Double) -> (opacity: Double, offset: Double) {
        let phase = time.truncatingRemainder(dividingBy: period) / period
        let t = phase < 0 ? phase + 1 : phase
        func between(_ start: Double, _ end: Double) -> Double {
            DPMMotion.Curve.easeOut.value(at: (t - start) / (end - start))
        }
        switch t {
        case ..<0.08: return (0, 2)
        case ..<0.22:
            let k = between(0.08, 0.22)
            return (k, 2 + (-2.5 - 2) * k)
        case ..<0.36: return (1, -2.5 + 2.5 * between(0.22, 0.36))
        case ..<0.70: return (1, 0)
        case ..<0.90: return (1 - between(0.70, 0.90), 0)
        default: return (0, 0)
        }
    }
}

/// "Working", the quiet dots and, when the run says when it started, how long it has run: for a run whose status is
/// working. The words carry the meaning; the dots only accompany them.
struct RunWorkingLine: View {
    let run: RunSummary

    var body: some View {
        HStack(spacing: 6) {
            QuietDots()
            Text("Working").font(.caption.weight(.medium))
            if let started = run.startedAt?.date {
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    Text(Self.elapsed(from: started, to: context.date))
                        .font(.caption)
                        .monospacedDigit()
                        .foregroundStyle(.secondary)
                }
            }
        }
        .accessibilityElement(children: .combine)
    }

    /// "for 4:07" or "for 1:02:30": the time since the run started, never negative.
    static func elapsed(from start: Date, to now: Date) -> String {
        let total = max(0, Int(now.timeIntervalSince(start)))
        let hours = total / 3600, minutes = total / 60 % 60, seconds = total % 60
        return hours > 0 ? String(format: "for %d:%02d:%02d", hours, minutes, seconds) : String(format: "for %d:%02d", minutes, seconds)
    }
}
