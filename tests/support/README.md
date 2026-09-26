# Synthetic contract input

`execution-plan.json` is a minimal internal test graph, not a demo or a real project.
Its two gated branches join before a zero-duration milestone. Tests use disposable storage and
synthetic actors to exercise readiness, blockers, verification, revisions, progress and scheduling.
The only user-facing example is `examples/self-host/dpm-alpha.json`; its tasks stay unstarted.
