# Synthetic contract input

`execution-plan.json` is a minimal internal test graph, not a demo or a real project.
Its two gated branches join before a zero-duration milestone. Tests use disposable storage and
synthetic actors to exercise readiness, blockers, verification, revisions, progress and scheduling.
`conditional-plan.json` is a second synthetic graph for conditional work. `DEC-SUPPLIER` offers
options `A` and `B`; `SUP-PKG-A` (with a nested package) and `SUP-PKG-B` apply only to their
option, and both qualification branches meet at `SUP-MERGE`, an active-branch join followed by
`SUP-BUILD`. `SUP-A-AUDIT` is unconditional but has an ordinary dependency on supplier A work, so
choosing B strands it rather than releasing it.
The only user-facing example is `examples/self-host/dpm-alpha.json`; its tasks stay unstarted.
