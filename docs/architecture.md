# Architecture

DPM treats project management as an executable semantic graph rather than a collection of UI
screens.

```text
                         projections / adapters
       SwiftUI*   Web*   Android*   CLI   Ratatui   MCP   HTTP*
                             |        |       |
                  dpm-app command / query boundary
                                  |
                    +-------------+-------------+
                    |       dpm-engine      |
                    | readiness / next / explain|
                    +--------+-----------+------+
                             |           |
                    dpm-model   dpm-schedule
                             |           |
                         authoritative   derived only
                           semantics     CPM / risk
                             |
                       dpm-store
                       SQLite now;
                     sync backends later
```

`*` means a planned adapter, not a v0.1 deliverable.

## Authoritative vs derived

Authoritative inputs include work lifecycle, objective, acceptance criteria, dependency type and
lag, duration estimates, requirements, decision outcomes, risks, ownership, artifacts, actors, and
commands.

Derived values include readiness, earliest/latest start/finish, float, critical activities,
completion percentiles, criticality, `next` scores, and UI layout. Derived values are recomputed and
must not become synchronized truth.

## Scheduling model

Each activity has a start variable `S` and duration `d`. Dependencies become lower bounds on the
successor start:

- FS: `S_b >= S_a + d_a + lag`
- SS: `S_b >= S_a + lag`
- FF: `S_b >= S_a + d_a - d_b + lag`
- SF: `S_b >= S_a - d_b + lag`

The dependency graph must be acyclic. v0.1 uses elapsed hours; working calendars are a future input
adapter and must not change dependency semantics.

For uncertain work, DPM stores optimistic / most-likely / pessimistic durations. Simulation
samples triangular distributions, recomputes the network, and reports completion percentiles and
the fraction of runs in which each activity is critical.

## Persistence

SQLite v0.1 commits a JSON domain snapshot and a semantic operation record in one transaction.
Operations already include actor, timestamp, command, base revision, and resulting revision. Full
operation replay, semantic merge, CRDT text collaboration, and remote synchronization are future
milestones.

SQLite is durable operational state, not a disposable cache of an exported plan. A workspace can
span multiple repositories and non-code projects; Git roots affect discovery, not domain scope.
Resources have stable IDs and explicit task requirements. Device-local workspace bindings select
one store from multiple entry points; locators check its identity before use. Semantic plan editing,
TOML plan interchange and sync remain task contracts available through `explain`.

## Git

Git commits and pull requests are artifacts linked to work. Git history is valuable evidence and a
useful export/version projection, but Git is not the live collaborative database.

## Terminal preview and agent adapters

`dpm-app` owns application orchestration over the pure engine and SQLite store. CLI and stdio
MCP use the same query results, command validation and atomic persistence. Structured execution
context resolves references into requirements, decisions, risks, evidence and related work.
MCP execution tools add revision metadata around the same CLI JSON data. Device-registry tools
return local configuration without a project revision.

The Gantt page renders `deterministic_remaining` as a read-only hour-axis chart. Work packages
roll up descendant ranges for display, milestones remain zero-duration points, and selecting a row
opens the same work context used by other views. Nothing in navigation or rendering updates state.
The console is explicitly a revision snapshot; reopening refreshes it.

## Execution progress

Task `reported_progress_percent` is an authoritative owner report defaulting to zero. `ReportProgress` validates ownership, range and lifecycle and persists a semantic operation.
It may move Claimed to InProgress but never verifies work. Submission displays 100% execution;
verification remains a separate condition. Reports on blocked work preserve the blocker.

`dpm-engine::progress` derives `{percent_complete, verified}` for tasks, milestones, containers
and the workspace. Container/workspace percentages equally weight descendant tasks; milestones
contribute no task weight and reflect binary prerequisite/gate completion. TUI and application queries
consume this shared projection. Gantt pan/zoom is viewport state only, and no percentage scales the
CPM/Monte Carlo duration inputs.


## Inspectable execution instructions

`WorkItem.instructions` optionally holds ordered action/result pairs, scope inclusions/exclusions
and verification checks. Acceptance criteria remain the conditions for independent review. The
model validates present instructions at import/command boundaries; plans may omit instructions. Both CLI and MCP serialize these same domain values through `dpm-app`.
Instructions contain no execution code, grant no authorization and never bypass decision gates.
The sole example supplies complete contracts while its tasks remain unstarted.

`Decision.related_work` adds a choice/question to a task's context without gating it. `blocks`
alone controls gating; both associations inherit through work-package ancestors. Optional rationale
and source artifact references are returned by `explain`, and source artifacts remain distinct from
completion evidence attached to work.
