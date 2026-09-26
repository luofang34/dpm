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

`dpm-schedule` projects a validated plan onto elapsed hours measured from a projection origin at 0.
Each activity `i` has a start `S_i` and a finite, non-negative duration `d_i`: the weighted PERT
expectation of a task's three-point estimate, or 0 for milestones and unestimated tasks. Work
packages cannot be dependency endpoints. v0.1 uses elapsed hours; working calendars are a future
input adapter and must not change dependency semantics. Remaining forecasts give completed tasks
zero duration and remove constraints touching completed tasks or reached milestones, using the same
completion and decision-gate projection as execution queries.

### Relations

A dependency `a -> b` with signed lag `L` (positive delays, negative leads) is a lower bound between
one endpoint of `a` and one endpoint of `b`. Every relation reduces to a start-to-start weight
`w(a, b)` with `S_b >= S_a + w(a, b)`:

| Relation | Constraint                 | `w(a, b)`         |
|----------|----------------------------|-------------------|
| FS       | `S_b >= S_a + d_a + L`     | `d_a + L`         |
| SS       | `S_b >= S_a + L`           | `L`               |
| FF       | `S_b + d_b >= S_a + d_a + L` | `d_a - d_b + L` |
| SF       | `S_b + d_b >= S_a + L`     | `-d_b + L`        |

There are no upper-bound (maximum-lag) constraints. Parallel edges between the same pair each apply.
A zero-duration endpoint has coincident start and finish, so FS and SS (and FF and SF) coincide for
a zero-duration predecessor, while FS and FF (and SS and SF) coincide for a zero-duration successor.

The dependency graph must be acyclic. Every directed cycle, including a self-loop, is rejected
whatever its relations and lags, even when leads would make the inequalities satisfiable.
`deterministic` and `deterministic_with_durations` report `ScheduleError::DependencyCycle`;
`deterministic_remaining` and simulation validate the plan first and report
`ScheduleError::Validation`. Missing, negative or non-finite durations are rejected, lags must be
finite, and non-finite intermediate sums fail with `ScheduleError::ArithmeticOverflow`.

### Bounds and float

The forward pass visits activities in topological order; the backward pass in reverse:

- `ES_i = max(0, max over incoming a -> i of ES_a + w(a, i))`: the origin acts as an implicit
  predecessor, so a lead never places work before 0.
- `EF_i = ES_i + d_i`, and project finish `F = max(0, max_i EF_i)`.
- `LS_i = min(F - d_i, min over outgoing i -> b of LS_b - w(i, b))`, and `LF_i = LS_i + d_i`.
- Total float `TF_i = max(0, LS_i - ES_i)`: delaying `i` by `TF_i`, with all other work as early
  as possible, leaves `F` unchanged; any further delay moves `F`.
- Free float `FF_i = min(TF_i, max(0, min(F - EF_i, min over outgoing i -> b of
  ES_b - ES_i - w(i, b))))`: delaying `i` by `FF_i` leaves every other activity's earliest start
  and `F` unchanged; any further delay moves one of them. The `F - EF_i` bound applies to every
  activity, not only terminal ones, because an SS or SF successor can retain slack after its
  predecessor already finishes the project.
- `critical_i` holds exactly when `TF_i <= 1e-8` hours; `critical_activities` lists those
  activities in topological order. The activity that attains `F` is always critical.

`ES` is the longest weighted path from the origin and `F - LS_i - d_i` the longest path to project
finish, so `LS_i >= ES_i` and `0 <= FF_i <= TF_i` hold exactly in real arithmetic. The `max(0, _)`
and `min(TF_i, _)` clamps only absorb floating-point rounding, keeping those inequalities exact in
the output. The tolerance `1e-8` hours (36 microseconds) is absolute: at a million elapsed hours the
spacing of f64 values is about `1e-10`, so rounding from summed durations and lags stays at least
two orders of magnitude below it, while any intentional slack of a second or more stays far above
it. Derived hours are not rounded; consumers compare them with the same tolerance.

The projection is a pure function of the plan and durations: it never edits the plan, and repeated
runs and reordered dependency lists produce identical output.

### Worked examples

Task A (4 h) precedes task B (3 h) under one relation; task C (10 h) runs in parallel with no
dependencies. A always has `ES = 0`, `EF = 4`; C has `ES = 0`, `EF = 10`, `LS = TF = FF = F - 10`.

| Relation, lag | F  | A: LS / LF / TF / FF | B: ES / EF / LS / LF / TF / FF | Critical |
|---------------|----|----------------------|--------------------------------|----------|
| FS +2         | 10 | 1 / 5 / 1 / 0        | 6 / 9 / 7 / 10 / 1 / 1         | C        |
| FS -2         | 10 | 5 / 9 / 5 / 0        | 2 / 5 / 7 / 10 / 5 / 5         | C        |
| FS +4         | 11 | 0 / 4 / 0 / 0        | 8 / 11 / 8 / 11 / 0 / 0        | A, B     |
| SS +2         | 10 | 5 / 9 / 5 / 0        | 2 / 5 / 7 / 10 / 5 / 5         | C        |
| SS -1         | 10 | 6 / 10 / 6 / 1       | 0 / 3 / 7 / 10 / 7 / 7         | C        |
| FF +2         | 10 | 4 / 8 / 4 / 0        | 3 / 6 / 7 / 10 / 4 / 4         | C        |
| FF -3         | 10 | 6 / 10 / 6 / 2       | 0 / 3 / 7 / 10 / 7 / 7         | C        |
| SF +5         | 10 | 5 / 9 / 5 / 0        | 2 / 5 / 7 / 10 / 5 / 5         | C        |
| SF +1         | 10 | 6 / 10 / 6 / 2       | 0 / 3 / 7 / 10 / 7 / 7         | C        |
| SF +9         | 10 | 1 / 5 / 1 / 0        | 6 / 9 / 7 / 10 / 1 / 1         | C        |

In the SS -1, FF -3 and SF +1 rows the origin bound, not the relation, fixes B at 0, which gives A
free float. With a zero-duration milestone M in `A -FS-> M -FS+1-> B` beside C, M has
`ES = EF = 4`, `LS = LF = 6`, total float 2 and free float 0. A milestone joining parallel 4 h and
6 h branches starts at 6 and is critical with the longer branch. The `temporal_contract`
integration tests in `dpm-schedule` assert these values and check seeded random networks against
an independent relaxation oracle.

### Temporal bounds are not execution permission

Temporal bounds describe when work could run in the projection; they never authorize a claim.
Execution readiness is a separate, conservative policy in `dpm-engine`: a task is claimable only
when every predecessor is complete (a verified task or a reached milestone), for all four relation
kinds and any lag. A lead that lets B's projected start precede A's finish does not make B
claimable while A is unverified; `UnmetGate::Dependency` reports the relation and lag as context
only.

### Uncertain durations

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
The console displays an explicit revision snapshot. `r` reloads through the application boundary,
retaining selection and viewport. Validation or source-identity failures preserve the last valid view
and display an error. Detail exposes the same execution contract and review context as `explain`.

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
