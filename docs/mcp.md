# Agent tools and CLI contract

Run `dpm-mcp --actor agent:coder` as a stdio subprocess inside a configured project.
Use `--project DIR` for an exact project, or `--database PATH` (`--db` alias) for a SQLite file.
The same [project discovery](projects.md) rules apply to CLI and MCP; the repository preview rejects
mutations with `read_only_project` and never creates a database.
The configured actor is the local principal for every mutation. A separate verifier process uses
`--actor human:reviewer` or a service actor. This is a trusted local workspace, not remote authentication.

Both adapters use `dpm-app` for queries, revision checks, engine commands and atomic persistence.
The CLI's `--json` output equals the MCP result's `structuredContent.data`. MCP adds `api_version:1`
and the observed `revision`, so an agent can send `base_revision` with its next mutation. CLI callers
can enforce the same precondition with `--base-revision N`; without it the CLI uses its loaded revision,
which the store still checks atomically. Presentation text is not the API contract.

| CLI | MCP tool | Shared behavior |
| --- | --- | --- |
| status | project_status | Counts and optional Monte Carlo forecast |
| next | next_work | Eligible leaf tasks, deterministic ranking and reasons |
| show KEY | get_work | Objective, steps/results, scope, acceptance/checks and derived status |
| explain KEY | explain_work | Readiness, dependencies, resolved requirements/gates/risks/evidence |
| claim KEY | claim_work | Claim only ready tasks |
| block KEY REASON | report_blocker | Record blocker and preserve owner |
| unblock KEY | unblock_work | Resume without changing owner |
| progress KEY PERCENT --note TEXT | report_progress | Owner reports 0..100 execution; verification remains separate |
| submit KEY --note TEXT | submit_work | Request independent verification |
| verify KEY --note TEXT | verify_work | Reject the submitting actor's self-verification |
| decide KEY OUTCOME | decide_gate | Resolve an open decision gate |
| artifact KEY FILE.json | add_artifact | Attach the same Artifact JSON object |
| attach-git-head KEY | attach_git_head | Capture the current repository's immutable HEAD |

Workspace setup/import/export and opening the TUI are local CLI administration, not agent execution
operations. Plan proposals and edits beyond these commands remain future work; no unsupported tool
is advertised. The same SQLite schema, engine and stable IDs remain available for future adapters.

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
