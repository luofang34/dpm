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
expectation of a task's three-point estimate, or 0 for milestones and unestimated tasks. The 0 h of
an unestimated task is not a claim about its duration, so `dpm-engine` names every outstanding
applicable unestimated task beside each forecast (`unestimated`) instead of leaving it optimistic
in silence; an explicit 0/0/0 estimate is a stated duration and is not listed. Work packages cannot
be dependency endpoints. Without a calendars block every hour counts; with one, durations are
working hours placed on calendars as described under [Calendars](#calendars), while the dependency
semantics stay the same. Remaining forecasts measure from the
adapter-supplied clock reading, give completed tasks zero duration and remove constraints into
completed tasks or reached milestones, using the same completion and decision-gate projection as
execution queries. A task that is not complete but has a recorded start event (in progress, blocked
or submitted) contributes only its remaining duration: its estimate conditioned on the task still
running after the hours elapsed since that start (see [Uncertain durations](#uncertain-durations)),
counted on the task's calendar when the plan has calendars. Blocked intervals count, because
estimates are durations rather than effort. A start
whose time was never recorded keeps the whole duration instead of assuming a head start. A task
that has outlasted its pessimistic bound projects to finish at the clock reading, which is no
earlier than execution can release its successors, since finish-based edges still wait for its
verification. A constraint from completed work, or a start-based (SS, SF) constraint from
work that has started, keeps only the lag the execution gate still reports as elapsing from that
event, and keeps its whole lag when the event time was never recorded. The forecast therefore never
releases a constraint before execution does, and waits exactly as long as execution for every
released or elapsing lag it keeps; it errs late, never early, in two cases. A finish-to-start edge
with a provisional start basis keeps its whole edge until the predecessor is verified, although the
gate may already have let the successor start on the pending submission. A started predecessor
projects at the clock reading unless its own outstanding constraints push it later, runs for its
remaining duration from there, and a start-based lag from it is then measured from that later
projection. They also drop waived soft constraints;
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

A claim reserves work; it is not a start. The lifecycle commands record event facts on the task
(`events.started_at`, `submitted_at`, `verified_at`), and `decide` records `Decision.resolved_at`,
as does applying a reviewed replacement decision. An event occurs at the operation's own timestamp
unless `start`, `submit` or `verify` names an earlier occurrence time (`occurred_at` in the
command, CLI `--at`, tool argument `at`) for work recorded after the fact. The operation keeps its
commit timestamp, so history holds both; the task's event, the gates evaluated for the transition,
its submission attempt or review and any dependency basis it captures all use the occurrence
time, so forecasts and calibration read honest times. An occurrence time after the commit
(`OccurrenceInFuture`) or before the latest time already recorded for the task — its events,
attempts and reviews, handoffs, releases or basis, and its claim (`OccurrenceBeforePrevious`) —
is refused with both times named. An unstarted claim records `events.claimed_at`, the claim's or
the reserving handoff's time; the start clears it, as the start time then orders the history, and a
release returns the work unowned without one. Because work is claimed before it starts, this floor
also covers tasks that apply_change added or that were ratified later. Only the claim of work that has not started carries
the field, so a store whose snapshot holds such a claim without it fails `verify-store` (its replay
now derives `claimed_at`) until that work starts; the data is intact. Backfilling covers the time
since the claim: work nobody claimed in advance is claimed when recorded, and `claim` itself takes
no occurrence time. A claim recorded before
claim times existed has none; the choice gate below still holds such a start after its selection.
Conditional work cannot record a transition before the choice that selected it: the gate evaluator
reports `Choice {key, chosen_at}` while the selecting decision of the work or a containing package
was made after the evaluated time, and a skipped branch into an active-branch join releases only
once its excluding choice was made. A start dated before the predecessor attempt it relies on was
reviewed records that attempt as its provisional basis, because the start then relied on an
unreviewed result; a predecessor whose latest attempt was since rejected releases no backfilled
start at all.

Two limits apply to backfilled times. Gates evaluate the current plan structure, dependencies,
waivers, conditions and decision chain at the occurrence time; a dependency added, waived or
restored since then is judged as it stands now, not as it stood then. And block, unblock, progress
reports and evidence attachment record no time, so a backfilled submission may land before a block
or a 100% progress report the log shows earlier; the operation order in `history` remains the
authority for those. No other command accepts a
caller-chosen event time, plan changes cannot add or rewrite these facts, and validation requires
`started <= submitted <= verified`, so a command stamped before the event it follows is refused
with no state change. Progress reports and
submission require an explicit start. Unblocking started work returns it to `InProgress`; a rejection
clears the rejected submission time. Work that started before start times were recorded has no
`started_at`; blocking it sets `events.start_unrecorded`, so gates, progress reports and `unblock`
all keep treating it as started (`WorkItem::start_event`), and its unknown start time still holds
positive lag closed. One limitation is not repaired: such work that was already `Blocked` when a
snapshot written before `start_unrecorded` existed was saved carries no start fact at all, so it
counts as unstarted (its SS/SF successors wait again) and `unblock` returns it to `Claimed`; its
owner starts it again with `start`, which records a new start time. Queries read these facts from the snapshot, never by scanning
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
predecessor's current pending or verified attempt. Only a human or service that holds or has held
neither task (see [release and handoff](#release-and-handoff)) may revalidate, and a stale or unrejected request changes nothing. Later upstream submissions and
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
A dependency with `lag_basis: Working` counts its positive lag in working hours of the successor's
calendar instead; the gate and the forecast use the same calendar arithmetic, so they open at the
same instant. Without calendars a working lag is elapsed.

Work completed before event times were recorded keeps its lifecycle but has no time. Such an event
has occurred at an unknown time: it releases zero or negative lag, while a positive lag that needs it
stays closed with `release.state = unrecorded_event_time` and an actionable reason (waive a Soft
edge, or change the lag through a reviewed plan change while neither the successor nor any
work depending on it has been claimed, since a claim already protects its prerequisite basis). A
milestone or package whose time depends on an unrecorded event has an unrecorded completion time.
`UnmetGate::Dependency` reports the edge identity, policy, relation, lag, the required predecessor
event (`requires`) and its `release` state (`awaiting_event`, `elapsing` with `event_at` and
`opens_at`, `unrecorded_event_time` or `lag_out_of_range`).

### Calendars

A plan's optional `calendars` block names an IANA `time_zone`, named weekly calendars with dated
exceptions, and the calendar of each actor kind. Two calendars are built in: `always`, where every
hour is working time, and `standard`, Microsoft Project's Standard calendar (Monday to Friday,
08:00-12:00 and 13:00-17:00), which a plan may redefine. The defaults follow Microsoft Project
once the block exists: people work on `standard`, agents and services on `always`, work with neither
an owner nor a planned `executor` is done by `default_executor` (Human), and verification waits for
the `verifier` kind's calendar (Human). A plan without the block schedules exactly as every hour
counting, so adding calendars is an explicit reviewed change.

A task's calendar is, from most to least specific: its own `schedule.calendar`; its owner's entry
in `calendars.actors` (availability only, never capacity or allocation); the calendar of its owner's
kind, else of its planned `schedule.executor`, else of `default_executor`. Its estimate counts
working hours on that calendar.

Remaining forecasts compile each calendar into working spans in hours after the clock reading, in
UTC, so daylight-saving changes and exceptions are exact. A task starts at the first working moment
after its start constraints, and its work ends when its duration of working hours has passed. A
finish constraint (FF, SF) moves the start back on the calendar so that the work ends no earlier
than the constraint; the finish is then held at the constraint if calendar gaps would end it
sooner. A task that still awaits verification finishes at the next working moment of the verifier's
calendar, which `explain` reports as `schedule.calendar.review_wait_hours`, together with the
calendar, executor kind and the rule that chose them. Latest times run the same arithmetic
backwards, with inverses that never undershoot the forward operations, so latest times never
precede earliest ones; a finish held to a constraint keeps that bound as its latest finish. Floats
stay elapsed hours. Work that waits for the verifier's calendar may have float without delaying the
project, so a plan whose finish is set by a review window can have no critical task. Calendar
arithmetic is exact to the millisecond, so results do not depend on the clock reading's sub-hour
part or on the window compiled; a window never exceeds a century, and work that would need more,
such as a calendar closed "until further notice", reports the projection as out of range. The baseline projection (`deterministic`, `simulate`) has
no clock reading and stays calendar-free. Derived dates are never stored.

### Dependency identity, policy and waivers

A dependency's `policy` is `Hard` (the default) or `Soft`, with an optional `rationale`. Policy,
kind, lag and rationale change only through a reviewed plan change; edges into started work stay
protected like the rest of its prerequisite basis. A human or service may waive an unwaived `Soft`
edge, or restore a waived one, with a nonempty reason; `Hard` edges cannot be waived and agents may
do neither. The actor must not hold, or have held before a handoff or release, either endpoint task, so no
owner relaxes a gate on its own work or on the result it hands on. The waiver's actor, time and reason stay on the edge until restoration, and each waiver
or restoration is a semantic operation in the history. Plan changes cannot add, alter or remove a
waiver, and a waived edge must be restored before a reviewed change edits or removes it.

A reviewed plan change is held to the same rule. `ApplyChange` refuses, with
`OwnGateRelaxed` naming the actor's work and a typed `RelaxedConstraint`, an actor that owns either
endpoint of an edge the change removes (also by deleting the other endpoint) or weakens: Hard to
Soft, a lower lag, a relation that no longer implies the old one (FS implies SS and FF, each of which
implies SF), a verified start basis made provisional, or any other difference. An edge between the
same two tasks that is at least as strict, under any identity, keeps the constraint, and tightening
is always allowed. The shared gate evaluator is also run before and after the change at the apply
time: any gate unmet on the actor's own work that the change would release (for example a
replacement re-selecting its excluded work), or any gate on a direct successor that waits on the
actor's work (for example a join changed to release work stranded behind it), refuses the whole
change. Another human or service may apply the same proposal.

Independence differs by command because the acts differ:

| Command | Refused actors |
|---------|----------------|
| `Verify`, `Reject` | holders of the task, and authors of evidence attached to it |
| `WaiveDependency`, `RestoreDependency` | agents, and holders of either endpoint |
| `RevalidateBasis` | agents, and holders of the successor or of its predecessor |
| `ApplyChange` | agents; holders of work whose constraint the change relaxes, as above |
| `AttachArtifact` on a task | everyone but its current owner |

A holder is the current owner or any actor that owned the task before a handoff or gave back a
claim on it (`WorkItem::held_by`). Evidence is attached to a task only by its current owner, because
an evidence author is not an independent reviewer: whoever wrote the proof would be judging it.
Planning sources (artifacts with `metadata.role = planning_source`, supplied in a workspace's
initial plan as context) are not evidence and do not disqualify their author; `AttachArtifact` refuses that
role so evidence cannot be relabelled as context.
Verification and rejection refuse any author of evidence on the task, which also covers evidence
imported with a snapshot. Only artifacts on the task itself count; evidence attached to a
containing work package or milestone (any actor may attach there, since aggregates are never
reviewed) and a decision's source artifacts do not. A single-person workspace therefore reviews
through a second actor, such as the person's human principal reviewing an agent's result.

A recorded rejection or basis revalidation is judged against the holders at its own time
(`WorkItem::holding_at`): the owner then, every handoff and release recorded at or before it (a
handoff at the same instant counts as earlier, since equal times cannot be ordered). A reviewer who
rejects a result may therefore take over the rework by a later handoff, while a record whose actor
held the task when it was made is invalid and the error names the handoff, release or ownership.

Verification checks a submitted result against its acceptance criteria and evidence and re-checks
every gate; it skips nothing, so the owner of a successor that consumes the result may verify it,
as a downstream reviewer would, and a two-person team needs no third principal to accept work that
the other person builds on. A waiver or a relaxing plan change removes a check instead of performing
it, and a revalidation certifies the successor's own execution, so each of those needs someone with
no stake in either task.

A waived edge no longer gates any transition or milestone completion, and no longer bounds
the remaining forecast or simulation, or the downstream count `next` and `explain` rank by. It still
counts for cycle validation, the baseline schedule, review protection and execution context.
A milestone whose every incoming edge is waived has no enforced prerequisite and therefore stays
unreached. Restoring an edge does not revoke an existing
claim, start or submission, but it gates the successor's next governed transition and always its
verification.

Every serialized edge requires an explicit `id`. Constructors may derive an initial ID from the
endpoints and relation, but edits retain the recorded ID; loading a plan never invents identity.
An omitted policy means `Hard`; serialization writes both identity and policy.

`links` hold typed non-gating relationships (`RelatesTo`, `Duplicates`, `DerivedFrom`,
`Supersedes`) from a `source` to a `target` work item with an optional note. Both endpoints must
exist and differ, and one pair of work items carries at most one link of each kind in either
direction. Links never affect readiness, scheduling, ranking or progress; `explain` returns them as
context.

### Conditional work and joins

Only a human or service may `decide`, whether the decision gates work or is context: a gate is how
people authorize execution, and an agent that resolved one would approve its own scope. A decision
may offer structured `options`; a decided outcome is exactly one option key. Work may
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
| `active_branches` join, edge from not-selected work | a skipped branch, released at the choice's effective time (below) |
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
time. That time counts only when the replacement changes the outcome: a choice's effective time is
the earliest resolution in the unbroken run of equal outcomes along its `supersedes` chain, so a
replacement that reaffirms the standing outcome moves no milestone, package or skipped-branch time
and never re-closes a gate that has already released (a reaffirmed choice first made before
resolution times were recorded stays unrecorded). A changed outcome starts a new run at its own
resolution time.

While open decisions condition work, `status` reports one forecast per option combination instead
of a single percentile: with no probability model, blending mutually exclusive branches would be a
false claim. The headline forecast then covers committed work only. `decide` refuses a choice that
would exclude claimed or started work; a reviewed replacement may do so, keeps that work's
lifecycle and evidence, reports it in the preview, and the work then takes no transition until the
plan changes. Automatic cancellation is not part of this model. `status` lifecycle counts
(`in_flight`, `blocked`, `awaiting_verification`) use the scope progress counts, so such kept work
appears only in `excluded_in_flight`. `dpm_engine::in_status_scope` and
`dpm_engine::excluded_in_flight` expose that scope, and the TUI Now page uses them: its blocked and
needs-review lists match their counts, and excluded in-flight work is listed apart with its
lifecycle and the reason it is excluded.

### Release and handoff

A claim can be wrong, an executor can stop, and work can need a different executor. Two commands
recover without a generic undo; neither is a gated transition, so neither consults the gate
evaluator, and both are semantic operations in the history with actor, time and reason.

| Command | Who | From | Effect |
|---------|-----|------|--------|
| `Release { work, reason }` | the owner, any actor kind | `Claimed` (no start event) | `Planned`, owner cleared |
| `Handoff { work, from, to, reason }` | a human or service | `Claimed`, `InProgress`, owned `Blocked` | owner becomes `to`; a `Handoff` record is appended |

Decisions and reasons:

- **Release is limited to unstarted claims.** A claim records only its reservation time, which the
  release ends, and no event, attempt or basis; only a start releases SS/SF successors, so undoing a
  reservation loses no fact and changes no successor's
  gates; readiness is derived, so the task is simply claimable again. Started work (including
  blocked work that had started, and submitted work) is refused with `AlreadyStarted`: releasing it
  would orphan its start event, progress and basis. Owned blocked work that never started is
  unblocked first, which returns it to `Claimed`. Agents may release their own claims, because
  giving up a reservation grants nothing.
- **Only a human or service authorizes a handoff.** An agent that could reassign work could pass
  its result to a collaborator and review it, or take work from another executor. An agent that
  cannot finish releases an unstarted claim, or blocks with a reason and asks for a handoff.
- **The authorizer may be the current or the new owner.** A person taking over an interrupted
  agent's work is the common recovery, and a single-person workspace has no one else to ask.
  Independence does not depend on who authorized the move but on the record: `WorkItem.execution.handoffs` is
  append-only (from, to, actor, at, reason), and `WorkItem::held_by` treats every earlier `from` as
  a holder. Verification, rejection, basis revalidation and waivers all refuse a holder, so an
  actor that executed any part of the work never reviews it, whoever holds it now.
- **Releases are recorded too.** `WorkItem.execution.releases` is append-only (actor, at, reason) and every
  releaser is a holder: a claimant may attach evidence before it releases, so giving back a claim
  never turns the claimant into the reviewer of the result it contributed to.
- **Everything recorded stays.** Status, events (including `start_unrecorded`), attempts, the last
  rejection, basis entries, progress, blocker and evidence are untouched; an invalidated basis is
  still invalidated for the new owner and still needs an independent revalidation.
- **Submitted work is not handed off.** Its pending attempt is the submitter's claim of completion;
  a reviewer rejects it first, which returns it to `InProgress`, and the rework can then move.
- **`from` is part of the command.** Adapters fill it from the observed owner, the operation log
  then names both sides, and a request whose `from` is no longer the owner is refused with
  `OwnerMismatch`. Handoff times are validated to be in order, like event times.
- **Choices.** Neither command asks the applicability gate: releasing excluded in-flight work stops
  it counting in `excluded_in_flight`, and handing it off moves ownership only; the work still takes
  no lifecycle transition until the plan changes.

Plan changes still cannot change owners, handoff or release records, and new work cannot carry
any of them.
A released task is no longer claimed, so its unstarted contract becomes reviewable again.

### Uncertain durations

For uncertain work, DPM stores optimistic / most-likely / pessimistic durations. Simulation
samples each task's beta-PERT distribution, recomputes the network, and reports completion
percentiles and the fraction of runs in which each activity is critical. The deterministic and
simulated forecasts share one duration model, so the CPM duration of every activity is the mean of
its simulated duration:

- **Whole duration.** Beta-PERT with the standard weight 4 on the most-likely value: shapes
  `α = 1 + 4(M − O)/(P − O)` and `β = 1 + 4(P − M)/(P − O)` on `[O, P]`, whose mean is the PERT
  expectation `(O + 4M + P) / 6` that CPM uses. Samples are `O + (P − O)·X/(X + Y)` with gamma
  draws `X ~ Γ(α)`, `Y ~ Γ(β)` (Marsaglia–Tsang over Box–Muller normals, from the seeded
  generator). An estimate with `O = P` is exact.
- **Remaining duration of started work.** With `e` hours since the recorded start (working hours of
  the task's calendar when the plan has calendars), the
  remaining duration is `D − e` conditioned on `D > e`. While `e ≤ O` nothing is ruled out and it
  is the whole distribution shifted by `e` (CPM: `PERT − e`). For `O < e < P` the conditional density
  is tabulated on a fine grid once per projection; simulation inverts that table with one uniform
  draw and CPM uses the same table's mean. CPM therefore never uses `max(PERT − e, 0)`, which would
  finish a task while its estimate still gives it a substantial chance of running on and so release
  its successors earlier than the simulation expects. For `e ≥ P` the remainder is 0, as described
  under [Scheduling model](#scheduling-model). Elapsed time runs from the first recorded start, so
  rework after a rejection counts toward `e` and a rejected task past `P` forecasts no remaining
  time; that errs early for the task itself but never releases successors, which still wait for
  verification.

The shared model makes each activity's means agree, not the project's: where paths merge, the
simulated P50 is above CPM's expected finish because the latest of several random finishes has a
higher mean than the latest of their means. Percentiles and criticality come from the same seed and
sampling order, and transcendental functions come from the pure-Rust `libm` (clippy disallows the
platform ones), so a query is bit-for-bit reproducible for the same plan and clock reading on every
device. Task durations are sampled independently; correlated overruns are not modeled.

## Calibration and flow

`dpm-engine` measures how recorded work compared with its estimates from the snapshot and the
operation log the application layer passes in, at one clock reading. Each verified task whose
verified submission and start are recorded and in the log gives one sample: the hours from its start
to that submission (working hours of its calendar when the plan has calendars) over its PERT
expectation, grouped by the kind of the submitting actor and by required capability. The review
waits of rejected attempts are subtracted, since they are measured as reviews. Work claimed, started
and submitted within minutes without occurrence times, work started before the log began, attempts
held by more than one actor kind, intervals with under a minute of working time and unestimated work
are excluded and counted by reason, overall and per executor kind. Review waits run from each
submission attempt to its review, by reviewer kind; decision waits from the reviewed change that
added a decision to its resolution, by deciding kind. Flow metrics (cycle and lead time, throughput,
aging work in progress, claim episodes) read the same records and skip the same bulk-recorded work.

These are derived views, never state: no estimate is rewritten. A calibrated `status` forecasts a
copy of the plan whose unfinished estimates are scaled by their executor kind's median ratio and
whose unverified tasks wait the verifier kind's median review time, each only with enough samples
and a factor only within a plausible band, so a record of zero time never collapses the forecast;
`dpm-schedule` places that review delay before the verifier's calendar, and a plan without
calendars is placed on `always` calendars so nothing else moves. The default forecast is untouched.

## Persistence

SQLite v0.1 commits a JSON domain snapshot and a semantic operation record in one transaction.
Operations include a caller-supplied identity, actor, timestamp, command, base revision and
resulting revision; the engine mints no identities, so re-executing a recorded operation reproduces
it exactly. The store keeps the genesis plan it was initialized or imported with, and the snapshot
is always the result of replaying every operation from it. Semantic merge, CRDT text collaboration
and remote synchronization are future milestones.

The operation identity is a version 7 UUID that clients may choose, and it is the idempotency key:
the store answers a recorded identity inside the write transaction before any revision or lineage
check, so `dpm-app` returns the recorded operation to a matching resend and refuses other content as
a duplicate (see [the agent contract](mcp.md#operation-identity-retries-and-lineage)). Each
recorded operation also carries the workspace it changed and the lineage of the store that
committed it. A lineage is one writable history, minted when a store file is created for writing:
by initialization or import, and by `restore`, whose copy gets a new lineage while the source keeps
its own. A backup keeps its source's lineage but is an archive that refuses writes, so two writable
stores never share a lineage and a restored copy is never mistaken for the history it was copied
from. Revision preconditions can name the lineage they were observed in; a store continuing another
lineage refuses the operation, since revisions of different lineages are not comparable and
histories are never merged silently. Authenticated principal identities are a later milestone.

A reviewed plan change is logged as a delta, not as the proposal. Adapters still submit a full
proposed plan; `plan_change` validates it exactly as `propose_change` does and records the canonical
entity-level difference: one entry per changed workspace, project, workspace asset, work item,
requirement, decision, risk, external reference or dependency edge, plus the link set, each with its
full `before` and `after` value and the changed field names. `patch` applies such a delta only if
every `before` still equals the current entity, otherwise it fails with a stale-change error, and
applying the command then runs the same validation, protection, agent refusal, independence check
and decision `resolved_at` stamping on the patched plan that applying the full proposal would.
Artifacts are not part of the delta: evidence changes only through the evidence command, so a
proposal that alters artifacts is refused before a delta exists.

The difference identifies edges by ID and compares links as a set, so a delta does not record the
order of `dependencies` or `links`. After a plan change both keep their current order, with removed
entries dropped and added ones appended in the delta's order; a proposal that only reorders them is
"no semantic changes". That order never changes schedule numbers (dates, float, critical-path
membership, percentiles and criticality are the same for any order), but it is the display order of
lists built from those collections: `critical_activities`, the dependency listings in `explain`,
the TUI's dependency lines and the order of the exported JSON. Such a list can therefore differ from
the order in the applied proposal file.

The store records its layout version in SQLite's `user_version` header and checks it, together with
the exact set of schema objects that version names, on every open and again inside every write
transaction: newer versions, retired versions whose logs cannot be replayed, and altered layouts
(including planted triggers, views or indexes) are refused untouched. Recovery uses `dpm backup`
(SQLite online backup of one consistent snapshot, genesis plan, full operation history and schema
version into a new file), `dpm restore` (into a new path only) and `dpm verify-store`, which writes
nothing and creates no files (page integrity, exact layout, record decoding, snapshot validation,
history contiguous from the genesis revision to the snapshot revision, and a replay from the genesis
plan that must reproduce the snapshot). A divergence is reported, never repaired. JSON export is not
a backup, and copying a live WAL database file is unsafe;
see [backup and restore](projects.md#backup-restore-and-verification).

Agent runs are observations, not project facts, so they are not in the operation log, the snapshot or
the replay. A run records the contract, plan revision and lineage it observed and exact source
references; its durable lifecycle (working, waiting, failed, interrupted, completed) is kept apart from
bounded activity telemetry and from derived freshness, which is computed from DPM's own receipt times
and the query clock and is never stored, so silence reads as stale, never as completion. They live in
a sidecar run store with its own layout version, lineage binding and writer lock (it is backed up, restored and verified as a consistent pair with the project store, and runs from before a restore take no new facts), so activity cannot
contend with project commands and adding runs does not change the project layout. Domain types are in
`dpm-model`, the state machine, link rules and projection in `dpm-engine`, persistence in `dpm-store`,
and the shared commands and queries in `dpm-app`; no provider types enter the core.
See [runs](mcp.md#runs-and-activity).

SQLite is durable operational state, not a disposable cache of an exported plan. A workspace can
span multiple repositories and non-code projects; Git roots affect discovery, not domain scope.
Workspace assets have stable IDs and explicit task requirements. Device-local workspace bindings
select one store from multiple entry points; locators check its identity before use. Reviewed plan
changes operate on this graph. A portfolio spanning independent workspaces needs explicit external
references and an exchange protocol; matching repository names or revision numbers never merges
workspaces. CLI TOML interchange and sync remain separate task contracts available through `explain`.

## Project-file interchange

`dpm-interchange` maps the documented Microsoft Project XML (MSPDI) subset to and from the plan.
It sits beside the store, outside model/schedule/engine, and holds no state: an import builds a
candidate plan and a per-item report, and the candidate enters through the same reviewed
`propose_change` / `ApplyChange` path as any edited export. Imported tasks are Proposed; source
progress never completes local work. MSPDI has no IANA time zone, so source calendars become
workspace [calendars](#calendars) only when the import names one; the candidate then places each
task on the calendar the source tool uses (`always` for elapsed durations) and counts working-time
lags in working hours. Without a time zone, calendars are reported as not imported and every hour
counts. Export writes the calendars exported work uses and working or elapsed time formats to
match. See [the agent contract](mcp.md#microsoft-project-xml-interchange)
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

## Native and web clients

`dpm-sdk` re-exports `dpm-app` as `dpm_sdk::app`, so a native or web client calls the same
application layer as the CLI and MCP instead of reimplementing rules. Besides the JSON
`query_blocking`, `Application` returns the engine's result types directly: `status_blocking`,
`next_blocking`, `show_blocking`, `explain_blocking`, `export_blocking`,
`propose_change_blocking` and `history_blocking`, each with the revision and lineage it observed.
The JSON adapters serialize exactly these values.

`revision_blocking` (CLI `revision`, tool `workspace_revision`) reads the committed revision and
lineage without loading the plan, so a client polls it and reloads only when it changes. Within one
process, `watch_commits` returns a channel receiving the revision and lineage of every operation
that `Application` commits, after the commit; resends answered from the log and refused mutations
send nothing. Commits by other processes are seen by polling.

`Application` is `Send` but not `Sync`: it owns one SQLite connection. A client keeps it on one
owner thread or actor that serializes requests, or shares it as `Arc<Mutex<Application>>`. There is
no global instance; several applications, in one process or many, coordinate through the store's
revision and lineage checks.

Queries read "now" from a query clock, the system clock by default. A client that must reproduce a
view pins it (`set_query_clock`, CLI `--clock`, `dpm-mcp --clock`); mutations never read it, so an
operation's timestamp is always when it was committed.

The `sqlite`, `registry` and `git` features of `dpm-app` and `dpm-sdk` are on by default, and CI
checks the application layer without them for `aarch64-apple-ios` and `wasm32-unknown-unknown`.
Without `sqlite` an application opens previews only: queries answer and mutations are refused as
read-only. A request needing a left-out feature fails with the code `unsupported`, which the CLI
and MCP, built with every feature, never return. On `wasm32-unknown-unknown` identities are drawn
from the Web Crypto source (`uuid`'s `js` feature), so a browser build needs a JavaScript host.

The Gantt page renders `deterministic_remaining` as a read-only hour-axis chart. Work packages
roll up descendant ranges for display; a row that the `Timeline` marks not applicable shows its
applicability state instead of a bar, read from the timeline rather than inferred from the schedule; milestones remain zero-duration points, and selecting a row
opens the same work context used by other views. Nothing in navigation or rendering updates state.
The console displays an explicit revision snapshot and follows its source through the application
boundary. Every second, between input events, it reads `refreshed_revision_blocking`, which resolves
the project locator again and reads the store's revision and lineage without decoding the plan, and
reloads (`refreshed_snapshot_blocking`) only when the revision advanced on the displayed lineage. An
older revision or another lineage is reported in text (`STALE` in the header, both lineages in the
notice) while the last snapshot stays; `r` explicitly loads whatever the source holds. A reload keeps
the page, selection, Gantt viewport and Detail scroll. Validation, source-identity and probe failures
preserve the last valid view and display an error. With an explicit `--database` the open
connection is the source, so a file replaced underneath it is not seen; repoint a locator instead.
Detail exposes the same execution contract and review context as `explain`.

### The local native boundary

A local native host (a macOS app) reaches the application through one versioned request/response
contract, `Application::native_json_blocking`: one JSON line in, one out, transport-neutral and
stateless per client. It adds no rule. Every answer is a shared query or the shared
`CommandRequest`, in the same envelope the CLI `--json` output and MCP `structuredContent` carry,
and every refusal keeps its shared code.

Calls are `hello`, `attach`, `query`, `changes` and `command`. Every request names a `protocol`; an
unsupported one is refused as `unsupported_protocol` with the versions supported, before the call is
decoded and before any command runs, whether the caller is typed or JSON. Only `hello` is exempt,
because it negotiates. A malformed line is `invalid_request`. The codes the boundary adds are
`unsupported_protocol`, `invalid_request`, `workspace_mismatch`, `lineage_mismatch`,
`source_changed` and `workspace_changing`; a client keeps a code it does not know as unknown.
`hello` also states the capabilities: the five views and the queries and feeds each reads, the
controls that do not exist (run steering and stopping, answering run input requests, provider
session control, push events, remote access), what a `reported_only` run is, and the bounds.

| View | Queries | Feeds |
| --- | --- | --- |
| Now | `next`, `status`, `schedule`, `runs`, `explain`, `export` | project, lifecycle, activity, links |
| Live | `runs`, `run`, `run_lifecycle`, `run_activity`, `history`, `export` | lifecycle, activity, links, project |
| Review | `status`, `explain`, `show`, `history`, `export`, `runs` | project, lifecycle, activity, links |
| Detail | `explain`, `show`, `runs`, `run`, `run_lifecycle`, `run_activity`, `history`, `export` | project, lifecycle, activity, links |
| Gantt | `schedule`, `explain`, `export` | project |

The Gantt page of the macOS observer (`--page gantt`, Cmd-5) is read-only. Rows follow the plan
hierarchy (`parent`) in the key order the `schedule` projection lists them; a bar is the row's
`span` in elapsed hours from the evaluation time, a milestone is a diamond with its own text, and
float, criticality (a fraction of seeded simulations, never the human priority) and the project's
p50/p80/p95 finish hours are the projection's values. Relations (FS, SS, FF, SF with lead or lag and
the Hard or Soft policy) come from the `export` snapshot the observer already reads and from
`explain` in Detail. The client lays these out and derives nothing: no scheduling, no readiness, and
no calendar date, which appears only where a query supplies one (the projection supplies none, so
hours are labelled elapsed). Collapse and expand, zoom, pan and filters change only what is drawn,
never the selection (`ObserverModel.selection`, shared with Detail) and never the project. The
page's `schedule` read is displayed, and judged for staleness, only while the page is shown. A
schedule read that is owed because the status changed stays owed, and the view stays marked stale,
until a schedule is installed or the page is hidden: a status read that succeeds followed by a
schedule read that fails once is retried, not forgotten. A row's text, its accessibility words and
Detail also give the item's own optimistic, likely and pessimistic hours exactly as the plan
supplies them (`schedule.estimate` of the `export` and `explain` payloads), or say that none is
recorded, apart from priority, float and criticality and from the project's p50/p80/p95; no
estimate is computed.

The measurement of the Gantt (`scripts/measure_gantt.py`) judges every run, a measurement, a control
or a replayed log, with one analyzer before any statistic is eligible: every planned generation, a
timed-out or late draw, completeness, content facts and the integrity of the log. A view operation's
call is logged on entry to its handler and the draw must carry its row count and its applied view
state (zoom, pan, filter, collapsed count); an external commit's draw must carry the task that was
selected before it. The content is finite, so a scroll request carries a logical distance (40 points
per request in its window) and the content wraps over its scroll range; the driver works out the
expected effective offset apart from the draw, and the draw stamps the offset its geometry actually
placed the content at. A frame counts only for a unique (window, axis, number) request, only when the
offset moved and the expected and drawn offsets are equal; an axis whose content fits its viewport has
no scroll FPS and yields no number. This is a FEAT-20 measurement clarification; the FEAT-05 record
is unchanged, and a frame is a completed draw pass, not a presented one. Resident memory counts a
sample only with a successful size for both the app and its helper. An INVALID run keeps its raw
observations and contributes no number, a failed control withholds every aggregate, and each launch
keeps its exact command, raw log, state, output, outcome and hashes next to the report.

Project operations are the semantic log. Run lifecycle, bounded activity and operation links are
observations beside it and never change the plan. A completed run is the executor's report: work
stays as it was until its owner submits it and an independent verifier accepts it. A `reported_only`
run's silence proves nothing, so an unfinished run goes stale and is never taken for idle or done.

`attach` names the workspace, the lineage and whether the source is a preview, a live store or an
archive, and every later call is checked against it. A project locator may be repointed while a
connection stays open, so each read and write re-resolves it: another file, another kind of source,
another workspace in a preview file, another lineage, or a declared workspace or asset that a fresh
open would refuse, is refused (`source_changed`, or the shared error a fresh open gives) and nothing
is written to the source the connection holds. Two previews both have no lineage, so the file and
the workspace tell them apart. The guard runs before the write, not inside its transaction.

**Positions.** The project store and the run store are separate files read in separate
transactions, so no answer claims one atomic snapshot of both. Revisions wrap and are never
compared for order, only for equality; the positions that are followed are local append counters.
A `query` carries a `basis`: where each store the query reads stood just before it was read. The
basis is a lower bound, so a subscription from it misses nothing, and the answer may already hold
some of what follows, so a poll may repeat it. A project-only query carries no run basis and a run
query carries both, so a later project-only answer can never move a run cursor past a change a held
run snapshot lacks. Reads never wait for writes to stop, so continuous run telemetry cannot starve
them; `workspace_changing` means only that the project itself changed identity or revision under a
read three times. `changes` takes independent cursors and returns one bounded page per feed:
project operations (scoped to a lineage), run lifecycle (never pruned) and run activity (bounded;
retention that outran a cursor is reported as a `gap`, and the feed goes on), the latter two scoped
to the run store's epoch. A cursor that cannot be followed is a `reset` (`lineage_changed`,
`epoch_changed` or `cursor_ahead`, the last for a source rolled back), never a silent continuation;
a restore forks the lineage and the epoch. A client that attached before any run existed holds a
cursor valid in any epoch. Operation links change a run's view with no transition and no activity,
so they have a count of their own: `link_count`, the number of links in the epoch. It only grows, a
replay does not move it, and it offers no order and no resumable position, so `changes` reports
only whether it changed and the client reads its displayed runs again.

**Time.** A result carries `evaluated_at`, the one clock reading every time-dependent value in it
was evaluated at. `refresh_at` is the earliest instant the answer changes with no new revision: a
run turning stale, or the shared gate evaluator's earliest elapsing dependency lag
(`Timeline::next_release`, the rule the terminal also uses, which holds the exact boundary: one
reading before it the gate waits, a reading at it the gate is open). A client re-queries at that
instant and never derives readiness. Forecasts drift continuously and a fact dated in the future is
not reported as elapsing, so a client also re-evaluates within `reevaluate_within_seconds`.

**Consumer rules.** Nothing advances until it has been applied: a cursor moves after its page was
applied, a page already applied is discarded by feed identity (lineage or epoch, with the
workspace for the project) and sequence, and a changed identity is an explicit reset that clears
what was held. The client remembers the furthest position it has seen of each store. A refreshed set
of views makes a store current only if every view in the set that carries it was anchored at or
beyond that position, and the installed set is what is displayed, so a response delayed past a
change it lacks leaves the client stale, and an old response installed after a fresh one makes it
stale again. A client seeded from several views is anchored at the earliest of them; views taken
under different identities cannot be composed. The link mark is adopted only from a view that was
installed. A failed refresh or a dropped connection therefore leaves the staleness visible to the
next poll. Because the producer keeps no per-client state, a slow or disconnected consumer costs it
nothing and recovers from its own cursors; one request is in flight per connection.

**The local bridge.** A macOS host reaches the contract through one persistent helper process,
`dpm-native` (`crates/dpm-native`), over the same line-delimited JSON on standard input and output;
there is no second, in-process path. The helper owns the application and its store for its whole
life, serves one request at a time from a single loop, and writes only protocol frames to standard
output (tracing goes to standard error). Its workspace is selected as the CLI selects it: an
explicit `--project` or `--database`, or discovery from the working directory; it never initializes
one. `--clock` pins the query clock for the session and is the only way to pin it; no request can.
A workspace that cannot be opened is a structured refusal frame with an empty identifier, then exit
status 2.

Frames are bounded in both directions (`--max-request-bytes`, `--max-response-bytes`; at least 1024,
4 MiB and 32 MiB by default). A request longer than its bound is discarded as it streams, never
buffered, and is refused `frame_too_large`; invalid UTF-8, a truncated last line and a malformed
line are each refused with a stable code, and the next request is still served. A correlation
identifier is at most 128 characters of `[A-Za-z0-9._:-]` and is rejected before anything runs, so
every refusal can echo it within the minimum bound. A response that would exceed its bound is
replaced by `response_too_large`, which names the committed operation when the request was a
command that committed: an answer that could not be delivered is never taken for a rollback.

The Swift client (`native/swift/Sources/DPMNative`) keeps the rules a host would otherwise
rediscover. One exchange is in flight per connection; up to four more wait, and one beyond that is
refused as busy and never sent. A request is pinned to the helper it was admitted to and is refused,
never moved, if that helper is gone. Cancellation takes effect at once on the caller: a request that
had not left is removed and never sent; one that had is not stopped, its answer is read and
discarded, and the error says it may have been processed. Closing, killing and reconnecting do not
wait for an exchange blocked on the helper: they abort it, tell its caller, and end the process
within bounds (input closed, then SIGTERM, then SIGKILL), reporting which step ended it. A helper
that cannot be ended is an error that keeps it owned, never a success. The connection is published
open only after negotiation, attach and the source check pass, and a frame fault (wrong correlation,
invalid or truncated frame, oversized answer, a protocol or api version other than the negotiated
one, a timeout) closes it before the caller hears of it, so nothing is ever sent on a stream that is
out of step. A reconnect reaches the same source or is refused unless the caller adopts the new
identity; the client's own cursors continue through the new helper.

Commands are sent once. Each carries a client-minted version 7 `operation_id`, which the
application treats as an idempotency key, so a command whose outcome is unknown
(`commandOutcomeUnknown`) may have committed and is reconciled only by an explicit resend of the same
request, which returns the recorded operation. The library never resends, never rolls back, and
closing a window releases no ownership and makes no verification: a claim stays with its actor.

The helper is packaged in `DPMHost.app` beside a minimal host (`dpm-host`), found from the host
executable's own location and never from the working directory. `scripts/build_native.py` builds the
bundle (ad-hoc signed; distribution signing is not part of it). `scripts/smoke_native.py` copies it
away from the build tree and, from another directory, compares what the host prints with the real
CLI and agent-tool output at one pinned clock and runs the Swift integration suite in
`native/swift/Qualification` against the real helper, the real CLI and a fault-injecting relay. It
needs macOS and a Swift compiler and says so when it has none.

### The macOS observer

`DPMObserver.app` (`native/swift/Sources/DPMObserver`) is built by `scripts/build_native.py` beside
`DPMHost.app`, with its own copy of the helper, ad-hoc signed and not an installer. Both bundles are
built for, and declare, macOS 14 or later. It is a local, read-only SwiftUI window onto one workspace:
**Now** (ranked work with the application's own reasons, the runs in progress and, when `next` is
empty, what holds work back, each thing openable), **Live** (managed and reported-only runs, public
activity, input requests, lifecycle and freshness), **Review** (submitted work with its acceptance
criteria and evidence, verification kept apart from anything a run reported) and **Detail** (one
task, decision or run in full as wrapped, scrollable text, beside a search over every task and
decision). It reads the queries and feeds of the table above. `status` counts the work awaiting
verification and names tasks only by identity, so Review, the search and the decisions come from the
shared snapshot (`export`); the runs of one task come from the per-task `runs` query. It has no
command call, so it cannot steer a run, answer its requests, verify or release work; what it does not
do is said in plain words.

Its engine (`DPMObserverCore`, no UI) polls the feeds from the consumer's cursors. Run activity is
appended to the selected run's bounded window and reads nothing else; a project change reads the
project views once; the runs are read at most once per interval; and the time-dependent views are read
again, at an unchanged revision, only when the application's `refresh_at` or re-evaluation bound
passes. What a change obliges stays owed until its read was installed, and staleness is published
before a read is awaited, so a newer answer is never shown beside older ones marked current. Every
connection has a generation taken before the open or close suspends, a helper still in its handshake
is cancelled and ended before the next open starts, and a late answer is dropped rather than attached
to a new source. A selection is a task's, decision's or run's persistent identity. A lost helper is
replaced and shown as lost; a repointed locator is reported and never followed until the operator
reads again; closing or quitting ends the helper and nothing else. Every blocking step runs on the
bridge's own queues.

Lists say how much they read. The newest runs are read, up to a bound, and a list that reached it says
older ones were not read instead of claiming none exist; a task's own runs are read for it. A run is
opened by its identity, so one that leaves the newest list stays inspectable, and its detail shows the
contract it observed when it started (workspace, lineage, revision, objective, acceptance), its source
references and provider, apart from the task as it is now. A retention gap the feed reports, and a run
whose lifecycle or activity exceeds what one reading takes in, are named. "Observation basis" in the
sidebar lists the basis and evaluation time of every displayed view and the feed cursors.

Start it with a workspace the operator names, `open target/native/DPMObserver.app --args --project DIR`
or `--database FILE`, or with no arguments and choose one with ⌘O. This repository's directory opens
its read-only preview. A missing or invalid project is an error on screen and nothing is created.
⌘1 to ⌘4 switch views, ⌘F opens the search in Detail with the cursor in it, ⌘R reads again, ⇧⌘W
closes the workspace. `--state-file PATH` writes what the window shows (including whether the search
field has focus), and `--page`, `--select-key`, `--select-run`, `--select-decision` and `--clock` set
the starting view, selection and query clock; `scripts/smoke_observer.py` uses them to read the real
app.

A repeatable operator scenario, with the keyboard or VoiceOver:
`python3 scripts/smoke_observer.py --prepare DIR` writes disposable stores and prints the launch
command. In `review.sqlite`, TEST-A awaits review after a managed run that asked a question, beside a
reported-only run, and an open decision holds the rest back. From Now, with nothing ready, choose the
decision under "What holds work back": its question and the work it gates are listed, and each can be
opened. Press ⌘F, type `TEST-B`, and open it: its readiness says what is in the way. In Review, TEST-A
shows its acceptance criteria, its evidence and that only an independent verifier accepts it, whatever
the run reported. In Live, each run shows its public activity, its input request and, in Detail, the
contract it observed. Then, in another terminal,
`dpm --database DIR/review.sqlite verify TEST-A --actor human:reviewer` leaves the Review queue empty
as the project feed delivers it, and the selection stays; TEST-B is then held back by the decision
alone. `unrun.sqlite` holds started work that no run has touched, found with ⌘F. The update budget
the contract proposes is two seconds under a stated workload; measured timings belong to the
qualification output. Whether this is usable is for a person to judge.

### The Claude Code adapter

`dpm-claude` (`crates/dpm-claude`) connects the installed Claude Code runtime, as one provider, to
the run records above. `dpm-claude run --work KEY --executor agent:NAME --directory DIR --prompt
TEXT` starts one provider process confined to `DIR` (`--safe-mode --restricted`, a permission mode
that never approves, `--permission-prompt-tool stdio`) and records a **managed** run of the task
through the shared application, as a service actor. Provider types and process I/O live only in that
crate; the model, schedule and engine know nothing of them. It is Claude-specific by design: there is
no provider framework, and a second provider would be a second adapter.

*Protocol and capabilities.* It speaks the runtime's documented stream-json control protocol:
`initialize`, one user message, and an answer to every `can_use_tool` request. Nothing is recorded
and nothing is sent until the provider answers `initialize` with a matching success response: a
flood of other frames, a session announced first, a refusal, the output ending, or the time bound
leaves no run and no prompt, and the command exits as unrecorded. Every such answer is a
refusal: the adapter approves nothing, answers no question and steers nothing (run steering is a
later node), and refuses any other control request as unsupported. Which public events it
normalizes is an explicit allowlist: session announcements, assistant text, tool starts and results,
permission requests, retries and the terminal result. Reasoning, thinking blocks and signatures,
account, plugin and command listings, usage and cost, rate-limit and token events, tool arguments
beyond one identifying value (a path, pattern, question or command, bounded and redacted), and the
output of a successful tool are never copied out; they are counted, not kept. A tool failure keeps a
bounded excerpt. Authentication belongs to the runtime: the adapter never reads, passes on or stores
a credential, and the provider's standard error is counted and discarded.

*Identity.* A record is identified by what the provider itself calls it: the event `uuid` for text
(never the shared message id, which two distinct tool uses carry), the `tool_use_id` for tool starts,
results and permission requests, the event `uuid` of a retry. An event without such an identity is
refused and counted, never given an invented one. The provider session is generated by the adapter
and must match what the provider announces; events from before the session initialized, from another
session or from a subagent's turn are counted and not recorded as activity, and a subagent's result
never ends the run (a subagent's permission requests are still refused, since the provider waits for
the answer). The provider
reports no turn identity in this mode, so `turn` is the adapter's own ordinal within the session
(`1`, then `2` for a recovery). A run's `session.provenance` attests, immutably, the model asked for,
the model and runtime version the provider announced (absent when it had not announced them: this
runtime announces itself only after a prompt, which is never sent before the run is recorded), and a
digest of the configuration it was started with. Exact sources are recorded only when explicitly
supplied with `--source-commit` or `--source-artifact` and validated by the application; the adapter
never borrows one from the plan's history.

*Order and durability.* Records get strictly increasing source sequences in the order accepted and
are written in bounded batches before the terminal transition, which is written last, once, under an
identity derived from the run. A write that fails is retried under the same sequences, which the
store treats as idempotent, within a time budget; if it cannot be written the provider is stopped and
the failure is reported, never hidden, and the run stays `working` so it shows as stale. Identities
already recorded are remembered in a bounded replay window: a repeat with the same content adds
nothing, and a repeat with different content is reported as a conflict and not kept. The window is
not a provider cursor. The provider does not replay its stream, so nothing resumes from a cursor;
past the window an old identity would be recorded as new, and the run says so.

*Evidence of an ending.* Only the provider's own explicit, well-formed terminal result completes a
run: a `success` subtype, an explicit boolean error flag of false, and the provider's own reason
`completed`, all agreeing; even then the task is neither submitted nor verified and stays owned. A
reason that says the turn was interrupted or cancelled interrupts it, whatever else the result
says; a result flagged as an error fails it; a success whose reason is absent or unrecognized ends
nothing at all. A provider whose output ends or that
exits without a result, with a clean or a nonzero status, is a failed turn, not a completed one; a
signal or a stop by the adapter (time bound, cancellation, lost recording) is an interruption. Each
is a distinct recorded evidence. Malformed, oversized and non-UTF-8 lines are noted, bounded, and
the stream goes on.

*Bounds.* Output reaches the intake through a bounded queue that stops the provider reading when it
is full, so pending work never exceeds the queue and nothing is dropped to make room. All three
pipes are nonblocking and every wait is a poll against a deadline and a stop check; a provider that
stops reading its input, one that ignores its input closing, and a descendant that holds an
inherited pipe cannot hold the adapter past its bounds. Only the child the adapter started is
signalled. Prompt, line, queue, time and input sizes are all capped. Run writes go to the run store
and never take the project writer lock, so project commands never wait on the adapter or on an
observer; the existing change feeds expose the activity and lifecycle to a host, from its own
cursor, with its own bounded pages.

*Recovery.* A run that has not ended is not proof that its provider stopped. An adapter holds an
exclusive lock on a host record, under the store's canonical identity, for as long as it hosts a
provider session, and the record says whether a provider is running and which process. `dpm-claude
resume --run RUN_ID` refuses unless the record exists and is well formed, no live adapter holds it,
and the record proves no provider can be running; a record that is missing, corrupt, says a provider
was starting, or names a process that exists or cannot be checked refuses and changes nothing. The
lock proves only that the adapter process is gone, not that its provider, or anything it started,
has stopped. A resume is never a replay of the missing events or a continuation of the old turn: it
records a gap and closes the prior run as `interrupted` with its fate unknown, then starts a new
run, naming the prior as its parent, with the next turn ordinal in the same provider session and a
new prompt, after re-checking that its executor still owns the claimed or started task. Window
close, adapter exit and loss of telemetry never complete a run or release ownership.

*Limits.* One turn per run; no steering or approvals; the host record is a same-machine,
local-filesystem guarantee; a provider's own children are not tracked beyond the process it was
started as; and a run recorded as reported-only is a different thing: a CLI or tool client that
reports itself, whose silence proves nothing and turns stale, never idle or finished.

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

`WorkItem.contract.instructions` optionally holds ordered action/result pairs, scope inclusions/exclusions
and verification checks. Acceptance criteria remain the conditions for independent review. The
model validates present instructions at import/command boundaries; plans may omit instructions. Both CLI and MCP serialize these same domain values through `dpm-app`.
Instructions contain no execution code, grant no authorization and never bypass decision gates.
The sole example supplies complete contracts while its tasks remain unstarted.

`Decision.related_work` adds a choice/question to a task's context without gating it. `blocks`
alone controls gating; both associations inherit through work-package ancestors. Optional rationale
and source artifact references are returned by `explain`, and source artifacts remain distinct from
completion evidence attached to work.
