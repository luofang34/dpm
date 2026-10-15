// A task's classical PERT figures as the schedule projection reports them: the expected duration and standard
// deviation of its whole three-point estimate, the te and σ a PERT network labels an activity with. Read as given.

import DPMNative
import Foundation

public struct GanttPert: Equatable, Sendable {
    /// Expected duration te = (O + 4M + P) / 6, in hours.
    public let expected: Double
    /// Standard deviation σ = (P − O) / 6, in hours.
    public let sigma: Double

    init?(_ json: JSON) {
        guard let expected = json["expected_hours"].double, let sigma = json["sigma_hours"].double else { return nil }
        self.expected = expected
        self.sigma = sigma
    }
}
