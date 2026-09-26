# Agent tools and CLI contract

Run `dpm-mcp --actor agent:coder` as a stdio subprocess inside a configured project.
Use `--project DIR` for an exact project, or `--database PATH` (`--db` alias) for a SQLite file.
The same [project discovery](projects.md) rules apply to CLI and MCP; the repository preview rejects
project mutations with `read_only_project` and never creates a project database.
The configured actor is the local principal for every project mutation. A separate verifier process uses
`--actor human:reviewer` or a service actor. This is a trusted local workspace, not remote authentication.

Both adapters use `dpm-app` for queries, revision checks, engine commands and atomic persistence.
The CLI's `--json` output equals the MCP result's `structuredContent.data`. Execution tools add `api_version:2`
and the observed `revision`, so an agent can send `base_revision` with its next mutation. CLI callers
can enforce the same precondition with `--base-revision N`; without it the CLI uses its loaded revision,
which the store still checks atomically. Presentation text is not the API contract.

| CLI | MCP tool | Shared behavior |
| --- | --- | --- |
| export | export_plan | Authoritative snapshot for proposals |
| plan diff FILE | propose_change | Validate a candidate and inspect entity/field differences |
| plan apply FILE --reason TEXT | apply_change | Human/service applies an observed-revision proposal atomically |
| history --after-sequence N --limit N | history | Chronological operation pages with actor, time, reason and command |
| status | project_status | Counts and optional Monte Carlo forecast |
| next | next_work | Eligible leaf tasks, deterministic ranking and reasons |
| show KEY | get_work | Objective, steps/results, scope, acceptance/checks and derived status |
| explain KEY | explain_work | Readiness, dependencies, resolved requirements/gates/risks/evidence |
| ratify KEY | ratify_contract | Human/service approves a complete Proposed contract |
| reject KEY REASON | reject_work | Independent reviewer returns Submitted work for rework |
| claim KEY | claim_work | Claim only ready tasks |
| block KEY REASON | report_blocker | Record blocker and preserve owner |
| unblock KEY | unblock_work | Resume without changing owner |
| progress KEY PERCENT --note TEXT | report_progress | Owner reports 0..100 execution; verification remains separate |
| submit KEY --note TEXT | submit_work | Request independent verification |
| verify KEY --note TEXT | verify_work | Reject the submitting actor's self-verification |
| decide KEY OUTCOME | decide_gate | Resolve an open decision gate |
| artifact KEY FILE.json | add_artifact | Attach the same Artifact JSON object |
| attach-git-head KEY --resource KEY | attach_git_head | Capture HEAD for an explicit task resource; locator binding is the default |
| workspace list | workspace_list | List device-local bindings without changing the plan |
| workspace register --database PATH | workspace_register | Register an existing store; explicit replace redirects a local binding |

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
`blocks`. The old outcome, rationale and sources stay intact, and `explain` returns both records.
Open gates are resolved only by `decide`; superseding one, rewriting a prior decision, dangling or
repeated `supersedes` links, and replacements that add gates are rejected with no state change.
`affected_work` in the preview lists every work item (key, kind, status) whose context contains either
decision, including started work, so reviewers can reassess it; it never changes readiness.

`history` returns entries in append order with a `next_after_sequence` cursor (default limit 100,
capped at 1000). Sequence is local to the store, distinct from wrapping revision IDs. Snapshot export
is not an operation backup; retain consistent SQLite backups for recovery.

## Prepared self-host example

The default `demo` is the [dpm roadmap](../examples/self-host/README.md), with open execution
and phase gates. Its `next_work` result is intentionally empty. Query its contracts freely, but do
not call mutation tools on it until the user separately authorizes beginning the self-host work.
The workflow below applies to an authorized execution workspace; tool availability is not approval.

## Agent workflow

1. Call project_status and next_work. Supply actual capabilities; an empty capability set is the
   unfiltered operator view, not a claim that the actor has every skill. Default limit is 5.
2. Call explain_work. Read `work.objective`, `work.instructions`, `work.acceptance`, predecessor
   evidence and unresolved gates. Steps describe the procedure, not permission to execute it.
3. Call claim_work using the observed revision. Refresh after a revision_conflict.
4. Perform the work and acceptance checks. Use report_progress for intermediate execution reports;
   use add_artifact or attach_git_head to record evidence.
5. Call submit_work with an evidence summary. Another configured actor verifies the result.
6. Call report_blocker when blocked; do not silently ignore dependencies or change lifecycle fields.

`probabilistic:false` matches CLI `status --no-simulation` or `next --deterministic-only`.
Capabilities and limit map to repeated `--capability` and `--limit`. Work identifiers in tool arguments
are human keys; returned records also include stable UUIDs. Domain errors use stable `code` plus
human-readable `message`; CLI wraps this in `error`, MCP uses `isError:true` and structuredContent.

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
`note`. The configured actor must own the claimed/in-progress/blocked task. Reports on planned,
submitted, verified, container or milestone work are rejected. Reports may correct the percentage
downward; blocked reports retain the blocker. Each successful report is a semantic operation.

`status`, `show` and `explain` include `progress: {percent_complete, verified}`. Task percentages
reflect `reported_progress_percent` until submission, when execution displays 100%. Only independent
verification satisfies successor execution prerequisites. Work-package/workspace percentages equally
weight descendant leaf tasks; milestone percentages are 0 or 100 according to their derived condition.
A package can have 100% execution with `verified:false` while reviews or gates remain unresolved.
An empty package/workspace reports 0%. A taskless package uses its binary aggregate condition.

Old snapshots that omit `reported_progress_percent` load as zero; lifecycle status still determines
submitted/verified display. Imported reports above 100 or nonzero reports on unowned/aggregate work
are rejected. The report is an additive serialized field; no stored schedule dates are introduced.

`explain.context.dependencies` retains full FS/SS/FF/SF types and signed hour lag; milestone kinds,
zero-duration schedules and derived states are exposed by the same queries. Temporal constraints
remain scheduling bounds. Execution still conservatively waits for verified predecessors; reports
do not implement SS/SF wall-clock timers or automatically shorten the remaining-duration forecast.

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

`python3 scripts/smoke_agent.py` verifies real-process CLI/MCP query equality and shared execution,
progress reporting, revision conflict, evidence, blocker, decision and independent-verification behavior.

`ratify_contract` approves a complete Proposed task as a human/service; `reject_work` requires
Submitted work, a different reviewer, and a nonempty `reason`. Rejection retains ownership and
the latest review in `last_rejection`. Both require `key` and `base_revision`.
`explain_work.gates` and `project_status.gates` expose the same structured claim conditions.

`workspace_list` and `workspace_register` manage device configuration through the shared registry.
Registration accepts `database` and optional `replace`; neither tool takes a project revision.
Their results contain `local_config: true` and `data`, without a project operation or revision.
`attach_git_head` accepts an explicit `resource` key when the selected locator does not bind one.
