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

Authoritative inputs include work lifecycle, objective, acceptance criteria, dependency identity,
type, lag, policy, provisional start basis and waivers, submission attempts and recorded dependency
bases, non-gating work links, duration estimates, requirements, decision options and outcomes, work
conditions and join policies, risks, ownership, artifacts, actors, and commands.

Derived values include readiness, applicability, basis invalidation, earliest/latest start/finish,
float, critical activities, completion percentiles, criticality, `next` scores, and UI layout.
Derived values are recomputed and
must not become synchronized truth.

## Scheduling model

`dpm-schedule` projects a validated plan onto elapsed hours measured from a projection origin at 0.
Each activity `i` has a start `S_i` and a finite, non-negative duration `d_i`: the weighted PERT
expectation of a task's three-point estimate, or 0 for milestones and unestimated tasks. Work
packages cannot be dependency endpoints. v0.1 uses elapsed hours; working calendars are a future
input adapter and must not change dependency semantics. Remaining forecasts measure from the
adapter-supplied clock reading, give completed tasks zero duration and remove constraints into
completed tasks or reached milestones, using the same completion and decision-gate projection as
execution queries. A constraint from completed work keeps only the lag the execution gate still
reports as elapsing, and keeps its whole lag when the event time was never recorded, so the
forecast waits exactly as long as execution will. They also drop waived soft constraints;
the baseline projection keeps every constraint.

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

There are no upper-bound (maximum-lag) constraints. Every dependency has a stable `id`. An ordered pair
carries at most one relation of each kind, so SS and FF bounds between the same two tasks coexist,
each applies, and each is edited or waived on its own.
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

Temporal bounds describe when work could run in the projection; they never authorize a transition.
Execution gates are evaluated in `dpm-engine` from authoritative event facts and a clock reading the
adapter supplies; the model and engine never read a wall clock. One evaluator,
`dpm_model::Timeline`, decides every edge release, and the engine's gate report is the only
readiness check: `next`, `status`, `explain`, the TUI, progress, milestone completion, the remaining
forecast and every lifecycle command read the same result for the same plan and time.

#### Execution events

A claim reserves work; it is not a start. The lifecycle commands record their own operation
timestamps as event facts on the task (`events.started_at`, `submitted_at`, `verified_at`), and
`decide` records `Decision.resolved_at`, as does applying a reviewed replacement decision. No
command accepts a caller-chosen event time, plan changes cannot add or rewrite these facts, and validation requires `started <= submitted <= verified`, so a
command stamped before the event it follows is refused with no state change. Progress reports and
submission require an explicit start. Unblocking started work returns it to `InProgress`; a rejection
clears the rejected submission time. Work that started before start times were recorded has no
`started_at`; blocking it sets `events.start_unrecorded`, so gates, progress reports and `unblock`
all keep treating it as started (`WorkItem::start_event`), and its unknown start time still holds
positive lag closed. Queries read these facts from the snapshot, never by scanning
the operation log, and readiness is never stored.

| Relation | Gates the successor's | Predecessor event it waits for |
|----------|-----------------------|--------------------------------|
| FS       | claim and start       | finish: verification (`verified_at`) |
| SS       | claim and start       | start (`started_at`) |
| FF       | submission            | finish: verification (`verified_at`) |
| SF       | submission            | start (`started_at`) |

Verification re-checks every unwaived relation of all four kinds and every decision, so a restored
waiver applies again before acceptance and a finish is never accepted on an unverified FF
predecessor. Only verification counts as a predecessor's finish; a submitted predecessor has not
finished for gating, except that a provisional edge (below) may release a successor's start. Claim
and start share the start gates, so only claimable work is recommended by `next`.

#### Provisional submission bases

Each submission appends a `SubmissionAttempt` with a deterministic per-task ordinal; rejection and
verification close it in place, so attempts are never removed or renumbered. A finish-to-start edge
between tasks may declare `start_basis = Provisional`, an authoritative policy changed only by
reviewed plan change. `Timeline::start_edge` releases such an edge for the successor's claim and
start on the submission of the predecessor's current (pending or verified) attempt plus positive
lag, so verifying that attempt never delays an elapsing start; every other evaluation, including the
successor's submission and verification, milestone reach, progress and the remaining forecast, uses
`Timeline::edge`, so a submission never counts as a finish. A submission recorded before attempts
existed has no identity to rely on and does not release the edge, and neither does an attempt of
work a choice did not select.

The start command records the attempts the shared evaluator released it on as immutable
`DependencyBasis` entries on the successor. Basis state is derived from the predecessor's attempt
record, never cached: a basis on a rejected attempt adds `UnmetGate::BasisInvalidated` to the
successor's submission and verification gates until `RevalidateBasis` appends a basis on the
predecessor's current pending or verified attempt. Only a human or service that owns neither task may
revalidate, and a stale or unrejected request changes nothing. Later upstream submissions and
verifications never re-base a successor, and no command rewrites downstream lifecycles.

A milestone has zero duration: its start and finish are one reach event, and all four relation kinds
into it gate that event. It is reached when every unwaived incoming edge is released and every
decision naming it or a containing package is resolved; its completion time is the latest of those
release and resolution times, so a decision resolved after every prerequisite verification sets the
time. A work package completes with its children, at the latest child or decision time.

#### Lag

An edge is released at the clock reading `now` when its predecessor event has occurred at time `t`
and `t + max(L, 0) <= now`. Positive lag is elapsed calendar time and cannot be bypassed: a gate
24 hours after an event is closed at +23 h and open at +24 h. Negative lag (a lead) shapes the
schedule projection only; execution still waits for the event itself, and `explain` says so.
Working-time calendars do not apply to execution lag.

Work completed before event times were recorded keeps its lifecycle but has no time. Such an event
has occurred at an unknown time: it releases zero or negative lag, while a positive lag that needs it
stays closed with `release.state = unrecorded_event_time` and an actionable reason (waive a Soft
edge, or change the lag through a reviewed plan change while neither the successor nor any
work depending on it has been claimed, since a claim already protects its prerequisite basis). A
milestone or package whose time depends on an unrecorded event has an unrecorded completion time.
`UnmetGate::Dependency` reports the edge identity, policy, relation, lag, the required predecessor
event (`requires`) and its `release` state (`awaiting_event`, `elapsing` with `event_at` and
`opens_at`, `unrecorded_event_time` or `lag_out_of_range`).

### Dependency identity, policy and waivers

A dependency's `policy` is `Hard` (the default) or `Soft`, with an optional `rationale`. Policy,
kind, lag and rationale change only through a reviewed plan change; edges into started work stay
protected like the rest of its prerequisite basis. A human or service may waive an unwaived `Soft`
edge, or restore a waived one, with a nonempty reason; `Hard` edges cannot be waived and agents may
do neither. Like verification and basis revalidation, the actor must own neither endpoint task, so
no owner relaxes a gate on its own work or on the result it hands on. The waiver's actor, time and reason stay on the edge until restoration, and each waiver
or restoration is a semantic operation in the history. Plan changes cannot add, alter or remove a
waiver, and a waived edge must be restored before a reviewed change edits or removes it.

A waived edge no longer gates any transition or milestone completion, and no longer bounds
the remaining forecast or simulation, or the downstream count `next` and `explain` rank by. It still
counts for cycle validation, the baseline schedule, review protection and execution context.
A milestone whose every incoming edge is waived has no enforced prerequisite and therefore stays
unreached. Restoring an edge does not revoke an existing
claim, start or submission, but it gates the successor's next governed transition and always its
verification.

Edges written without an `id` receive one derived deterministically from predecessor, successor and
relation kind, so snapshots and operation logs that predate edge identity load unchanged and every
reload agrees. Such edges are `Hard`. Serialization always writes the identity and policy.

`links` hold typed non-gating relationships (`RelatesTo`, `Duplicates`, `DerivedFrom`,
`Supersedes`) from a `source` to a `target` work item with an optional note. Both endpoints must
exist and differ, and one pair of work items carries at most one link of each kind in either
direction. Links never affect readiness, scheduling, ranking or progress; `explain` returns them as
context.

### Conditional work and joins

A decision may offer structured `options`; a decided outcome is exactly one option key. Work may
carry a `condition` naming a decision and option; a condition on a work package applies to all its
descendants, and every condition in a work item's containment chain must hold. Conditions follow a
decision's `supersedes` chain, so a reviewed replacement (with the same option keys) is how a made
choice changes. A task or milestone may declare an explicit `join` policy for its incoming edges.

`dpm_model::Timeline` derives each work item's applicability once, from decisions, conditions,
join policies and unwaived edges in dependency order, then derives each work package from its
children (innermost first), and every projection reads that result:

| Situation | Rule |
|-----------|------|
| Condition's decision is open | `undecided`: no transition; not counted in progress; not complete |
| Decision selected another option | `not_selected`: no transition; excluded from progress, CPM and Monte Carlo; verified work stops counting as completion |
| Ordinary edge (`all_predecessors`) from not-selected work | never released (`release = not_selected`); the successor is `stranded` |
| Edge from stranded work, or from an empty join | never released; the successor is `stranded` |
| Edge from undecided or awaiting work | the successor is `awaiting_choice`: not committed until the choice is made |
| `active_branches` join, edge from not-selected work | a skipped branch, released at the choice's `resolved_at` |
| `active_branches` join, every branch skipped | `empty_join` unless `allow_empty`; with it, reached at the latest choice time |
| Work package with an applicable child | `applicable`; complete when every child a choice did not exclude is complete, and at least one is |
| Work package whose every child is excluded (`not_selected` or itself `all_children_excluded`) | `all_children_excluded`: treated exactly as `not_selected` (listed, not counted, never completion, a skipped branch of its parent at the latest excluding choice) |
| Work package with no applicable child and one still awaiting a choice | `awaiting_choice` naming that child: not committed, not complete, the workspace stays open |
| Work package whose remaining children are all stranded or empty joins | `children_stranded` naming the first: it can never complete without a reviewed plan change |
| Selected task already verified | stays `applicable`; a later choice cannot strand finished work |

A package's applicability and its completion rule read the same children, so no view can call a
package applicable that the timeline can never complete. Work packages cannot be dependency
endpoints, so no edge leaves an excluded package; its container and the workspace are its only
consumers. A package with no children at all is outside these rules and stays `applicable`.

Only `applicable` work passes any lifecycle gate (`UnmetGate::Applicability` otherwise), is
recommended by `next`, or enters the remaining projections; excluded activities are absent from
`deterministic_remaining` and `simulate_remaining` output. Stranded and awaiting work stays
outstanding in progress, so a plan that can no longer finish never reports completion. A milestone
or package time includes the resolution times of the choices that selected it or skipped its
branches; a replacement created by a reviewed plan change is resolved at the time of the
`ApplyChange` operation that records it, so completions and lag that depend on it have a recorded
time.

While open decisions condition work, `status` reports one forecast per option combination instead
of a single percentile: with no probability model, blending mutually exclusive branches would be a
false claim. The headline forecast then covers committed work only. `decide` refuses a choice that
would exclude claimed or started work; a reviewed replacement may do so, keeps that work's
lifecycle and evidence, reports it in the preview, and the work then takes no transition until the
plan changes. Automatic cancellation is not part of this model. `status` lifecycle counts
(`in_flight`, `blocked`, `awaiting_verification`) use the scope progress counts, so such kept work
appears only in `excluded_in_flight`.

### Uncertain durations

For uncertain work, DPM stores optimistic / most-likely / pessimistic durations. Simulation
samples triangular distributions, recomputes the network, and reports completion percentiles and
the fraction of runs in which each activity is critical.

## Persistence

SQLite v0.1 commits a JSON domain snapshot and a semantic operation record in one transaction.
Operations already include actor, timestamp, command, base revision, and resulting revision. Full
operation replay, semantic merge, CRDT text collaboration, and remote synchronization are future
milestones.

The store records its layout version in SQLite's `user_version` header and checks it on every
open before any write: newer versions are refused untouched, the unversioned baseline layout is
stamped by its next write transaction, and layout migrations run in one transaction or are refused.
Recovery uses `dpm backup` (SQLite online backup of one consistent snapshot, full operation history
and schema version into a new file), `dpm restore` (into a new path only) and read-only
`dpm verify-store` (page integrity, layout, snapshot validation, contiguous history ending at the
snapshot revision). JSON export is not a backup, and copying a live WAL database file is unsafe;
see [backup and restore](projects.md#backup-restore-and-verification).

SQLite is durable operational state, not a disposable cache of an exported plan. A workspace can
span multiple repositories and non-code projects; Git roots affect discovery, not domain scope.
Resources have stable IDs and explicit task requirements. Device-local workspace bindings select
one store from multiple entry points; locators check its identity before use. Semantic plan editing,
TOML plan interchange and sync remain task contracts available through `explain`.

## Project-file interchange

`dpm-interchange` maps the documented Microsoft Project XML (MSPDI) subset to and from the plan.
It sits beside the store, outside model/schedule/engine, and holds no state: an import builds a
candidate plan and a per-item report, and the candidate enters through the same reviewed
`propose_change` / `ApplyChange` path as any edited export. Imported tasks are Proposed; source
progress never completes local work. See [the agent contract](mcp.md#microsoft-project-xml-interchange)
for the mapping, unit conversions and loss reporting.

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
roll up descendant ranges for display; a row that the `Timeline` marks not applicable shows its
applicability state instead of a bar, read from the timeline rather than inferred from the schedule; milestones remain zero-duration points, and selecting a row
opens the same work context used by other views. Nothing in navigation or rendering updates state.
The console displays an explicit revision snapshot. `r` reloads through the application boundary,
retaining selection and viewport. Validation or source-identity failures preserve the last valid view
and display an error. Detail exposes the same execution contract and review context as `explain`.

## Execution progress

Task `reported_progress_percent` is an authoritative owner report defaulting to zero. `ReportProgress` validates ownership, range and lifecycle and persists a semantic operation.
It requires started work (an explicit `Start`) and never verifies work. Submission displays 100% execution;
verification remains a separate condition. Reports on blocked work preserve the blocker.

`dpm-engine::progress` derives `{percent_complete, verified, completed_at}` for tasks, milestones, containers
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
