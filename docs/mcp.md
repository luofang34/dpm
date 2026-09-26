# Agent tools and CLI contract

Run `dpm-mcp --actor agent:coder` as a stdio subprocess inside a configured project.
Use `--project DIR` for an exact project, or `--database PATH` (`--db` alias) for a SQLite file.
The same [project discovery](projects.md) rules apply to CLI and MCP; the repository preview rejects
project mutations with `read_only_project` and never creates a project database.
The configured actor is the local principal for every project mutation. A separate verifier process uses
`--actor human:reviewer` or a service actor. This is a trusted local workspace, not remote authentication.

Both adapters use `dpm-app` for queries, revision checks, engine commands and atomic persistence.
The CLI's `--json` output equals the MCP result's `structuredContent.data`. Execution tools add `api_version:6`
and the observed `revision`, so an agent can send `base_revision` with its next mutation. CLI callers
can enforce the same precondition with `--base-revision N`; without it the CLI uses its loaded revision,
which the store still checks atomically. Presentation text is not the API contract.

| CLI | MCP tool | Shared behavior |
| --- | --- | --- |
| export | export_plan | Authoritative snapshot for proposals |
| plan diff FILE | propose_change | Validate a candidate and inspect entity/field differences |
| plan apply FILE --reason TEXT | apply_change | Human/service applies an observed-revision proposal atomically |
| plan import-mspdi FILE --project-key KEY | import_mspdi | [MSPDI](#microsoft-project-xml-interchange) candidate, preview and per-item report; no state change |
| plan export-mspdi --project-key KEY | export_mspdi | One project's work as MSPDI with a report of omitted data |
| history --after-sequence N --limit N | history | Chronological operation pages with actor, time, reason and command |
| status | project_status | Counts and optional Monte Carlo forecast |
| next --project-key KEY --resource-key KEY | next_work | Full-graph ranking, then [scope](#scoped-next) and limit; outside-scope work stays visible |
| show KEY | get_work | Objective, steps/results, scope, acceptance/checks and derived status |
| explain KEY | explain_work | Readiness, dependencies, resolved requirements/gates/risks/evidence |
| ratify KEY | ratify_contract | Human/service approves a complete Proposed contract |
| reject KEY REASON | reject_work | Independent reviewer returns Submitted work for rework |
| claim KEY | claim_work | Reserve only ready tasks; a claim is not a start |
| start KEY | start_work | Owner starts claimed work; records the start event SS/SF successors wait for |
| block KEY REASON | report_blocker | Record blocker and preserve owner |
| unblock KEY | unblock_work | Resume without changing owner |
| progress KEY PERCENT --note TEXT | report_progress | Owner reports 0..100 execution; verification remains separate |
| submit KEY --note TEXT | submit_work | Request independent verification of started work once FF/SF gates are released |
| verify KEY --note TEXT | verify_work | Reject self-verification; re-check every relation and decision; record the finish event |
| decide KEY OUTCOME | decide_gate | Resolve an open decision; with `options`, OUTCOME is exactly one option key |
| artifact KEY FILE.json | add_artifact | Attach the same Artifact JSON object |
| attach-git-head KEY --resource KEY | attach_git_head | Capture HEAD for an explicit task resource; locator binding is the default |
| link-external KEY --provider P --instance HOST --namespace NS --kind K --id ID | link_external | Link work to a provider-scoped external object; context only |
| unlink-external KEY --provider P --instance HOST --namespace NS --kind K --id ID | unlink_external | Remove one link; the work graph is unchanged |
| workspace list | workspace_list | List device-local bindings without changing the plan |
| workspace register --database PATH | workspace_register | Register an existing store; explicit replace redirects a local binding |
| waive-dependency ID --reason TEXT | waive_dependency | Human/service stops enforcing a Soft edge; Hard edges need plan review |
| restore-dependency ID --reason TEXT | restore_dependency | Human/service enforces a waived Soft edge again |
| revalidate-basis KEY --dependency ID --attempt N --reason TEXT | revalidate_basis | Independent human/service re-bases started work after the attempt it relied on was rejected |

Project initialization/import and opening the TUI are local CLI administration. `export_plan` supplies
the full candidate shape for `propose_change` and `apply_change` (argument `plan`). Preserve its
workspace identity and revision; plan apply defaults to the file's revision, never a silently refreshed
one. Diff entries contain collection, stable ID, changed fields and full before/after values; null
means addition/deletion. Preview has no side effects. Applying requires a nonempty reason and a
human/service actor. Agents draft scope; they do not approve their own expansion.

New tasks must be Proposed, without execution/evidence. Existing work keys/kinds, lifecycle, owners,
progress, reviews and artifacts cannot be changed through this route. Started work, its
prerequisites and containing packages, and the projects, requirements and resources it names are
protected, and no new gate may block them; add follow-up work instead. Unstarted contracts,
dependencies, projects, requirements, resources and risks can be maintained after review. New
decisions are Open questions; decision replacement below may list started work for reassessment.
Git remotes in a shared plan cannot embed `user:password@` credentials. Undo is not part of this route.

A Decided choice is replaced, never edited: the proposal changes only its `status` to `Superseded`
and adds one new Decided decision whose `supersedes` names it, with a nonempty `rationale` and no
`blocks`. The old outcome, rationale and sources stay intact. `explain` returns both records for
every work item linked to either one, following `supersedes` forward, so work linked only to the old
choice still sees its replacement.
Open gates are resolved only by `decide`; superseding one, rewriting a prior decision, dangling or
repeated `supersedes` links, and replacements that add gates are rejected with no state change.
`affected_work` in the preview lists every work item (key, kind, status) whose context contains either
decision, including started work, so reviewers can reassess it; it never changes readiness.
A replacement for a decision with `options` keeps the same option keys, and its outcome may select a
different option: that is the only way to change a choice once made. `applicability_changes` in the
preview lists each work item whose [applicability](#conditional-work-and-branch-joins) the proposal
changes, with `in_flight`, `before` and `after`.

## External tracking references

An external reference records an issue, pull request or other tracker object without making its
state authoritative. Its identity is the tuple provider family (`GitHub`, `GitLab`, `Forgejo`,
`Gitea`, `Jira`, `Linear` or `{"Other":"name"}`), `instance` (lowercase `host[:port]` of the hosted
or self-hosted server), `namespace` (owner/repository, group path, tenant or project; required for
repository forges and Linear; rejected for Jira, whose keys such as `PROJ-1` are unique per
instance), `kind` (`Issue`, `PullRequest` or `{"Other":"name"}`) and
`external_id`. Equal IDs on another instance or namespace are different objects. Kind separates
objects only where the provider numbers them separately (GitLab issues and merge requests); GitHub,
Forgejo and Gitea number issues and pull requests together, so `#5` is one object whichever kind is
named, and the first recorded kind is kept. The label
and URL are display data. Adapters canonicalize equivalent spellings (host and forge namespace
case, `https://` prefixes, default ports `:443`/`:80`, `#`/`!` ID prefixes, a `.git` repository suffix, leading zeros in forge
numbers, Jira and Linear key and Linear workspace case) before lookup, and validation rejects any other form.

`link_external` takes `key`, `identity`, `base_revision` and optional `label`, `url`, `role`
(`Tracks` by default, or `Relates`) and `observed` (`Open`, `Closed` or `Merged`). The CLI uses
lowercase flag values. Each identity is recorded once, under a stable reference ID that survives
relabeling and namespace moves; links name work by stable ID, so key changes keep them. One work
item may track an identity (`tracking_conflict` otherwise); any number may relate to it. A work item
links an identity at most once. `unlink_external` removes one link, and removing the last one
removes the record. Unknown work, identities or links are `not_found`; stale revisions are
`revision_conflict`. Every failure leaves the snapshot and revision unchanged.

Links are never evidence, dependency satisfaction, gates or verification. `observed` is an
attributed report: a closed issue does not complete work, and a merged pull request does not
submit or verify it. Attach a pull request as an Artifact through `add_artifact` when it is
evidence; a separate verifier still decides. `explain.context.external_references` lists references
linked to the work or its containing packages, while `next`, `status` and schedules ignore them.
URLs must be http(s) on the identity's instance. Userinfo (`TOKEN@host` or `user:password@host`),
credential parameters such as `access_token`, and credentials in labels or identity fields are
rejected. No connector, token store or assignee write is involved.

References are part of the exported plan, so reviewed plan changes can add, relabel, move or
remove them under the same validation. `plan diff` reports them under `external_references`.
Like evidence and reviews, observations are attributed records: a plan change cannot add or
rewrite one; record it with `link_external`.

`history` returns entries in append order with a `next_after_sequence` cursor (default limit 100,
capped at 1000). Sequence is local to the store, distinct from wrapping revision IDs. Snapshot export
is not an operation backup; retain consistent SQLite backups for recovery.

## Microsoft Project XML interchange

The supported external format is the documented Microsoft Project XML schema (MSPDI), which
Microsoft Project, ProjectLibre, OmniPlan and MPXJ read and write. Binary `.mpp` and Primavera
files are not supported; convert them to MSPDI with another tool first. Tests read documents
written by MPXJ and check that MPXJ reads DPM's export back unchanged; acceptance by Microsoft
Project itself is not verified. `dpm-interchange` parses
and writes the subset without network access and never touches the store.

`plan import-mspdi FILE --project-key KEY [--key-prefix P] [--candidate OUT.json]` and
`import_mspdi` (`xml`, `project_key`, optional `key_prefix`) return `{report, preview, candidate}`.
`candidate` is a full plan at the observed revision, `preview` is its `propose_change` result, and
`--candidate` also writes it to a file. Importing is a query: nothing is persisted until a human or
service applies the candidate with `plan apply` / `apply_change`, which re-validates it against
the current revision. A malformed document, an unknown project or a stale candidate leaves the
revision and operation history unchanged.

| MSPDI | DPM candidate |
| --- | --- |
| Task `GUID` | Work identity. Without one, an identity derived from the project `GUID` and task `UID`; neither skips the task |
| `OutlineLevel` order | `parent`; a summary task (or any task with children) becomes a WorkPackage |
| `Milestone=1` | Milestone; a nonzero source duration is dropped and reported |
| other tasks | Task, `Proposed`, empty acceptance: never executable until ratified. A zero-duration task without the flag stays an unestimated Task, so exported unestimated tasks keep their kind |
| `Name`, `Notes` | `title`, `objective` (absent notes keep the local objective) |
| `Priority` 0..1000 | P0 ≥800, P1 ≥600, P2 ≥400, P3 ≥200, else P4; export writes 900/700/500/300/100 |
| `Duration` `PTnHnMnS` | Single-point estimate O=M=P in hours; zero means unestimated |
| `PredecessorLink` `Type` 0/1/2/3 | FF/FS/SF/SS dependency |
| `LinkLag` | `lag_hours = LinkLag / 600` (tenths of a minute) |

DPM schedules elapsed hours. Elapsed duration/lag formats (`em`, `eh`, `ed`, `ew`, `emo`) convert
exactly. Working-time formats (`m`, `h`, `d`, `w`, `mo`) keep their hour value but lose the
calendar: 1 working day (`LinkLag` 4800 on an 8-hour calendar) becomes 8 elapsed hours, not a
calendar day. The report marks those values as approximations. Percentage lags, unknown formats,
and nonzero lags without `LagFormat` are rejected with the link rather than guessed. Durations
with day, week or month designators are rejected. Cross-project links are rejected.

Work packages cannot be dependency endpoints. A summary finishes with its last child and starts
with its first, so a summary predecessor of an FS or FF link and a summary successor of an FS or
SS link expand exactly into one edge per task/milestone descendant. The other combinations, links
between a summary and its own descendant, and summaries without imported descendants are rejected.
Relations that expand onto the same pair and kind merge, keeping the larger lag.

`report.items` has one entry per source task with `outcome` (`Created`, `Updated`, `Unchanged`,
`Skipped`), the mapped `work`, and `preserved`, `approximated` and `rejected` findings;
`report.links` has one entry per `PredecessorLink` with `outcome` (`Preserved`, `Approximated`,
`Rejected`), notes and the resulting dependency IDs. Rejected data includes constraints,
deadlines, calendars, resources and assignments, baselines, custom fields and outline codes,
manual scheduling, recurrence, cost, timephased data, and percent complete or actuals. Source
progress never submits, verifies or completes local work. Inactive, blank, external and subproject
tasks are skipped with a reason; `report.rejected` covers document-level data and
`report.retained` lists local work in the project that the document does not name.

Re-importing updates work with the same identity and never duplicates it. Keys, lifecycle,
ownership, progress, evidence and acceptance stay local. The document owns only the dependencies
between work it imports; other edges are kept. An unchanged source duration or lag (compared in the
exported encoding) keeps the richer local three-point estimate, exact lag, policy, rationale and
waiver. Local work the document omits is retained, not deleted. Changes that touch protected
(started) work are refused by `propose_change` as with any reviewed change.

`plan export-mspdi --project-key KEY [--output FILE]` and `export_mspdi` (`project_key`) return
`{xml, report}`; without `--json` the CLI prints the document itself. Output is deterministic:
siblings follow key order, `UID`s number that order (they are local to the file), and `GUID`s are
the stable project and work identities. Tasks carry the PERT expectation in elapsed hours, and
links carry elapsed-hour lags. The report lists per-item omissions (acceptance, instructions,
lifecycle, owner, requirements, evidence, resources) and project-level data outside the subset.
MSPDI has no conditional work: every task is written unconditionally, never dropped, and the item
report names its `condition`, an active-branch `join`, and any current non-applicable state
(`applicability`); a link from not-selected work carries a note that it is written as enforced.
Exporting a project and importing the document into the same workspace yields no changes.

## Dependency policies, waivers and links

Every dependency in `export_plan` and `explain_work.context.dependencies` carries a stable `id`,
`policy` (`Hard` or `Soft`), optional `rationale` and, while set aside, a `waiver` with actor, time
and reason. An ordered task pair holds at most one edge per relation kind, so SS and FF coexist;
`propose_change` reports each edge as its own `dependencies` entry keyed by `id`. Commands name
edges by `id` (argument `dependency`); malformed or absent IDs return `not_found`. Proposals may
omit `id` on new edges, which then receive a deterministic identity from predecessor, successor and
kind. Duplicate identities or relations, missing endpoints, cycles and stale revisions reject the
whole proposal.

Policy, kind, lag and rationale change only through reviewed `apply_change`; proposals cannot add,
alter or remove waivers, nor edit or remove a waived edge, which must be restored first. `waive_dependency` and `restore_dependency` take `dependency`, a nonempty
`reason` and `base_revision`. Only human/service actors may use them, only Soft edges can be waived,
and restoring requires a current waiver. The waiver's actor, time and reason stay on the edge until
restoration; `history` records both operations with actor, time and reason. A waived edge is absent
from `gates.unmet`, readiness, verification prerequisites, milestone completion and the remaining
forecast. An unwaived Soft edge gates exactly like a Hard one, and `gates.unmet` reports its
`dependency` and `policy`.

`links` in the plan are typed non-gating relationships (`RelatesTo`, `Duplicates`, `DerivedFrom`,
`Supersedes`) with `source`, `target` and optional `note`, edited through reviewed plan changes.
Endpoints must exist and differ; one pair holds at most one link of each kind in either direction.
`explain_work.context.links` lists those touching the work. Links never change readiness,
scheduling, ranking or progress.

## Conditional work and branch joins

A decision may list structured `options` (`[{key, label}]`, at least two, unique trimmed keys).
Once decided, its `outcome` is exactly one option key; `decide` refuses any other outcome with no
state change. A work item's optional `condition: {decision, option}` makes it, and everything a
work package contains, apply only when that option is selected; conditions on nested packages all
apply. A task or milestone may set `join: {"mode": "active_branches", "allow_empty": false}`; the
default (`all_predecessors`, omitted) keeps ordinary dependency semantics. All three fields are
additive and edited through `propose_change` / `apply_change`.

Applicability is derived at query time and never stored. `explain_work.applicability` and
`project_status.not_applicable` report it with `state`:

| State | Meaning | Transitions | Forecast | Progress |
|---|---|---|---|---|
| `applicable` | Selected, and every prerequisite can proceed | gated as usual | included | counted |
| `undecided` | A condition awaits an open decision | refused | excluded; see scenarios | not counted; container/workspace incomplete |
| `not_selected` | A decision selected another option | refused | excluded | not counted; never completion |
| `awaiting_choice` | A prerequisite (or, for a work package, a child) is undecided | refused | excluded; see scenarios | counted |
| `stranded` | An ordinary edge from not-selected or stranded work never releases | refused | excluded | counted, outstanding |
| `empty_join` | Every branch into an active-branch join was not selected and `allow_empty` is false | refused | excluded | never reached |
| `all_children_excluded` | Every child of a work package was excluded; `decisions` names the excluding decisions | — | excluded | not counted; never completion; not required for workspace completion |
| `children_stranded` | No child of a work package is applicable and `child` is stranded or an empty join | — | excluded | never complete |

A refused transition reports `{type: "applicability", applicability}` in `unmet`. A dependency from
not-selected work reports `release.state = "not_selected"` into an ordinary successor and
`"skipped_branch"` (released at the choice time) into an active-branch join. A join is reached when
at least one branch is verified (or `allow_empty` is set and every branch was skipped), every active
branch is released, and its gates are resolved; its time includes the `resolved_at` of the choices
that selected it or skipped its branches. A work package completes when every child that a choice
did not exclude is complete, with at least one; its applicability follows the same children: with
no applicable child it is `all_children_excluded` (every child excluded, handled like
`not_selected`, including as a skipped branch of its parent package), `awaiting_choice` (a child
still waits for a choice; `predecessor` names it) or `children_stranded`. These package states are
additive values of `state`; `api_version` is unchanged. `progress.scope` is `not_selected` (also for
`all_children_excluded`) or `undecided` for work outside the counted scope.

`next`, scoped `next`, `status`, `explain`, progress, milestone completion, the remaining CPM and
Monte Carlo and the TUI read one evaluation. While open decisions condition work,
`project_status.open_choices` lists them with `scenario_count` and one `scenarios` entry per option
combination (up to 16): `choices`, `expected_finish_hours`, optional percentiles and the work that
would be `stranded`. The headline `expected_finish_hours` then covers committed work only and the
headline percentiles are null: branches without a probability model are never blended. Float and
criticality in `next` and `explain` are likewise computed over committed work. Lifecycle counts
(`in_flight`, `blocked`, `awaiting_verification`) still tally excluded work whose lifecycle a
reviewed choice change kept; `complete` and progress do not count it. A replacement decision carries
no `blocks`, as for any replacement.

`decide` refuses a choice that would exclude claimed or started work. Changing a made choice is a
reviewed decision replacement; it keeps in-flight work's lifecycle, owner and evidence, lists it in
`applicability_changes`, and that work then refuses further transitions until the plan changes.

## Prepared self-host example

The default `demo` is the [dpm roadmap](../examples/self-host/README.md), with open execution
and phase gates. Its `next_work` result intentionally has no candidates. Query its contracts freely, but do
not call mutation tools on it until the user separately authorizes beginning the self-host work.
The workflow below applies to an authorized execution workspace; tool availability is not approval.

## Agent workflow

1. Call project_status and next_work. Supply actual capabilities; an empty capability set is the
   unfiltered operator view, not a claim that the actor has every skill. Default limit is 5.
2. Call explain_work. Read `work.objective`, `work.instructions`, `work.acceptance`, predecessor
   evidence and unresolved gates. Steps describe the procedure, not permission to execute it.
3. Call claim_work using the observed revision. Refresh after a revision_conflict. A claim only
   reserves the task.
4. Call start_work when execution begins. Its operation time is the start event that SS/SF
   successors wait for; report_progress and submit_work are refused until the task has started.
5. Perform the work and acceptance checks. Use report_progress for intermediate execution reports;
   use add_artifact or attach_git_head to record evidence.
6. Call submit_work with an evidence summary once `explain_work.transitions.submit.ready` holds.
   Another configured actor verifies the result.
7. Call report_blocker when blocked; do not silently ignore dependencies or change lifecycle fields.

`probabilistic:false` matches CLI `status --no-simulation` or `next --deterministic-only`.
Capabilities and limit map to repeated `--capability` and `--limit`; `project_keys` and `resource_keys`
map to repeated `--project-key` and `--resource-key`. Work identifiers in tool arguments
are human keys; returned records also include stable UUIDs. Domain errors use stable `code` plus
human-readable `message`; CLI wraps this in `error`, MCP uses `isError:true` and structuredContent.
A command refused by its readiness gates (dependencies, decisions, applicability, provisional basis
or lifecycle eligibility checked by the gate evaluator) also carries `details: {transition, unmet}`,
the same structured conditions `explain_work` reports for that transition. Refusals decided before
the gates run, such as a lifecycle transition from `Blocked`, a missing start or another actor's
ownership, return only `code` and `message`; read `explain_work` `transitions` for the full list.

## Execution events and elapsed lag

Lifecycle commands record their own operation time: `start_work` sets `work.events.started_at`,
`submit_work` sets `submitted_at` (cleared by `reject_work`), `verify_work` sets `verified_at`, and
`decide_gate` sets the decision's `resolved_at`. No tool accepts an event time; plan changes cannot
add or rewrite these fields (omit `resolved_at` from a replacement decision), and a command whose
time precedes the event it follows is refused unchanged. Every query evaluates gates at one clock
reading taken by the adapter for that response.

| Relation | Gates the successor's | Waits for the predecessor's |
| --- | --- | --- |
| FS | claim and start | verification (`verified_at`) |
| SS | claim and start | start (`started_at`) |
| FF | submission | verification (`verified_at`) |
| SF | submission | start (`started_at`) |

Verification re-checks all four relation kinds and decisions. Only verification is a predecessor's
finish. Positive lag must elapse in calendar time after the event (`release.state: elapsing` with
`event_at` and `opens_at`); negative lag affects the schedule only and never releases work before the
event, which `why_now` states. Tasks verified or started before event times were recorded, and
decisions resolved before then, count as having occurred at an unrecorded time: zero or negative lag
releases, positive lag reports `release.state: unrecorded_event_time` with an actionable reason.

`explain_work.transitions` reports `claim`, `start`, `submit` and `verify` with the same shape as
`gates` (the claim report). A milestone's `progress.completed_at` is the latest release among its
incoming edges and gating decisions, so a decision resolved after every prerequisite sets it;
`{"recorded": TIME}` or `"unrecorded"`.

## Provisional submission bases

Each `submit_work` appends a submission attempt to `work.attempts`: a 1-based `number`, its
`submitted_at` and an `outcome` whose `state` is `pending`, `rejected` (with reviewer, time and
reason) or `verified` (with verifier and time). Reviews close attempts but never remove or renumber
them, so rejection history stays in the snapshot. Submissions recorded before attempts existed have
no attempt record and remain valid.

A finish-to-start edge between two tasks may carry `start_basis: "Provisional"`, set or cleared
only through reviewed `apply_change` like `policy` (and, like every edge into started work, frozen
once its successor is claimed). For the successor's `claim` and `start` gates it releases on the
submission of the predecessor's current (pending or verified) attempt plus positive lag, so
verifying that attempt never delays an elapsing start; `gates.provisional[]` names each edge
released on a still-pending attempt with the `attempt` relied on, and `why_now` and `next_work`
reasons say so. A legacy submission without
an attempt record, or a predecessor a choice did not select, does not release it. `submit`, `verify`, milestone completion, progress and the
remaining forecast still wait for the predecessor's verified finish: in `transitions.verify` the
edge appears with `start_basis: "Provisional"` and no `accepts_submission`.

`start_work` records the attempt each provisional edge was released on as an immutable entry in
`work.basis` (`dependency`, `predecessor`, `attempt`, `recorded_at`, `source.kind: "start"`). Claims
record nothing because they only reserve work. Whether a basis is invalidated is derived, never
stored: when the relied-on attempt is rejected, `transitions.submit` and `transitions.verify` report
`type: "basis_invalidated"` with the edge, `attempt`, the rejection in `state` and the
predecessor's `current_attempt`, and `project_status.basis_invalidated` counts such work. A later
submission or verification of the predecessor never clears it. `explain_work.basis.relies_on` lists
the work's own effective bases and `basis.relied_on_by` the downstream work relying on its attempts,
including the rejected ones, read from the snapshot rather than from history. A waived edge's basis
is reported with `enforced: false` and gates nothing.

`revalidate_basis` (`key`, `dependency`, `attempt`, nonempty `reason`, `base_revision`) appends a
new basis with `source.kind: "revalidation"`, actor and reason. It is refused with no change unless
the actor is a human or service that owns neither task (accepting work built on a rejected result is
an accountable judgement, like a waiver, and neither owner may certify its own work), the effective
basis on that edge is invalidated, and `attempt` is the predecessor's current pending or verified
attempt. Revalidating onto a pending attempt relies on it again: if it is also rejected, the work is
flagged again. Nothing rewrites lifecycles in cascade; the successor keeps its state throughout.

## Scoped next

`next`/`next_work` returns one object with `result_version: 1`. Evaluation order is fixed: gates,
readiness and scores use the full workspace graph; capability eligibility then selects the eligible
set; scope partitions it; `limit` truncates only the in-scope list. Scope is query-only: it never
changes claims, graph membership, readiness or scores, and it grants no filesystem authorization.

| Field | Meaning |
| --- | --- |
| result_version | Shape version of this object, independent of `api_version` |
| scope.projects[] / scope.resources[] | Resolved `{id, key}` entries actually applied; both empty means unscoped |
| capabilities | Capabilities used for eligibility; not a scope axis |
| limit | Maximum in-scope candidates returned |
| eligible_count | Ready, capability-eligible work in the whole workspace |
| in_scope_count | Eligible work inside the scope, before `limit` |
| candidates[] | In-scope work in global rank order; each is a ranked candidate plus `global_rank` (1-based among all eligible work) |
| outside_scope.count | Eligible work excluded by scope |
| outside_scope.higher_ranked_count | Outside work ranked above the best in-scope eligible work (before `limit`), or all outside work when none is in scope |
| outside_scope.keys[] | Keys of eligible outside work in global rank order; the first `higher_ranked_count` rank higher |

Membership, applied to eligible work; each empty axis does not filter and non-empty axes intersect:

- **Project** (`--project-key`, `project_keys`): the work's project is a listed project or a descendant.
  `--project DIR` still selects a project directory; the key filter is a separate option.
- **Resource** (`--resource-key`, `resource_keys`): the work names at least one listed resource, and every
  resource it writes is listed. Reads of unlisted resources are allowed. Work naming no resources
  (for example non-code work) never matches a resource scope; if eligible it is in `outside_scope`.
- **Capability** (`--capability`, `capabilities`): eligibility, not scope. Work the caller cannot
  perform is neither a candidate nor counted in `outside_scope`.

Unknown project or resource keys fail with `not_found` instead of matching nothing. A dependency
outside the scope is evaluated like any other: in-scope work waiting on it stays unready, and the
blocking work appears in `outside_scope` when it is itself eligible.

## Task instructions

`show/get_work` returns a WorkItem with derived `progress`. `explain/explain_work` returns that
contract under `work`, alongside readiness/ranking and resolved `context`. Both adapters serialize
the same types and never synthesize or truncate task instructions.

| Field under WorkItem | Meaning |
| --- | --- |
| objective | Purpose and observable goal |
| instructions.steps[] | Ordered actions, each with `action` and `expected_result` |
| instructions.in_scope[] | Permitted changes/deliverables |
| instructions.out_of_scope[] | Explicit exclusions |
| instructions.verification[] | Checks and evidence to collect |
| acceptance[].text | Conditions a separate reviewer must assess |
| capabilities / requirement_ids / artifact_ids | Skill requirements and stable context references |

Instructions are optional for compatibility with plans that omit them. When present, they are valid
only on Tasks and must include nonempty steps/actions/results, both scope lists and verification
checks. Every self-host task supplies all fields. Instructions are author-written data, not a script
runner, execution authorization or automatically verified evidence. If instructions are missing or
contradict gates/acceptance, report the gap instead of guessing permission or completion.

`python3 scripts/smoke_self_host.py` imports the prepared plan into temporary storage, compares every
task contract through real CLI/MCP processes and confirms no operations or revision changes.

`context.decisions` includes both gates and related design choices. `blocks` controls readiness;
`related_work` only supplies context, including through parent work packages. Optional `rationale`
and `artifact_ids` provide the reason and sources, resolved under `context.artifacts`. These sources
are planning context, not task completion evidence. A decided choice does not authorize execution
of the task or resolve any other gate. Missing optional fields retain their empty/default meaning.

## Progress and schedule indications

`report_progress` takes `key`, integer `percent` (0..100), required `base_revision`, and optional
`note`. The configured actor must own the started (in-progress, or blocked after starting) task.
Reports on planned, claimed-but-unstarted, submitted, verified, container or milestone work are rejected. Reports may correct the percentage
downward; blocked reports retain the blocker. Each successful report is a semantic operation.

`status`, `show` and `explain` include `progress: {percent_complete, verified, completed_at}`. Task percentages
reflect `reported_progress_percent` until submission, when execution displays 100%. Only independent
verification satisfies successor execution prerequisites. Work-package/workspace percentages equally
weight descendant leaf tasks; milestone percentages are 0 or 100 according to their derived condition.
A package can have 100% execution with `verified:false` while reviews or gates remain unresolved.
An empty package/workspace reports 0%. A taskless package uses its binary aggregate condition.

Old snapshots that omit `reported_progress_percent` load as zero; lifecycle status still determines
submitted/verified display. Imported reports above 100 or nonzero reports on unowned/aggregate work
are rejected. The report is an additive serialized field; no stored schedule dates are introduced.

`explain.context.dependencies` retains full FS/SS/FF/SF types and signed hour lag; milestone kinds,
zero-duration schedules and derived states are exposed by the same queries. Execution gates follow
[execution events and elapsed lag](#execution-events-and-elapsed-lag); progress reports never
release a gate or shorten the remaining-duration forecast.

## Protocol

The runnable adapter implements MCP `2025-11-25` initialize/initialized, ping, tools/list and tools/call.
Messages are newline-delimited UTF-8 JSON-RPC, limited to 8 MiB. Stdout is protocol-only. Initialize
with protocolVersion, capabilities and clientInfo, then send notifications/initialized. Responses
include text content and structuredContent. Domain failures are tool errors; malformed protocol
requests use JSON-RPC errors. EOF cleanly stops the process. The adapter advertises only tools.

Specification:
- https://modelcontextprotocol.io/specification/2025-11-25/basic/transports
- https://modelcontextprotocol.io/specification/2025-11-25/basic/lifecycle
- https://modelcontextprotocol.io/specification/2025-11-25/server/tools

`python3 scripts/smoke_tracking.py` covers external links through both adapters.
`python3 scripts/smoke_interchange.py` covers MSPDI import/export parity, refused applies and round trips.
`python3 scripts/smoke_agent.py` verifies real-process CLI/MCP query equality and shared execution,
progress reporting, revision conflict, evidence, blocker, decision and independent-verification behavior.
It includes `scripts/smoke_conditional.py`, which covers conditional-work parity and choice changes.

`ratify_contract` approves a complete Proposed task as a human/service; `reject_work` requires
Submitted work, a different reviewer, and a nonempty `reason`. Rejection retains ownership and
the latest review in `last_rejection`. Both require `key` and `base_revision`.
`explain_work.gates` and `project_status.gates` expose the same structured claim conditions;
`explain_work.transitions` adds start, submit and verify.

`workspace_list` and `workspace_register` manage device configuration through the shared registry.
Registration accepts `database` and optional `replace`; neither tool takes a project revision.
Their results contain `local_config: true` and `data`, without a project operation or revision.
`attach_git_head` accepts an explicit `resource` key when the selected locator does not bind one.
