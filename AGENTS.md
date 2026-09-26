# AGENTS.md

This repository is designed to be worked on by humans and coding agents. Read this file before
changing code. For substantial multi-file changes, also read `PLANS.md` and maintain an ExecPlan
under `docs/exec/`.

## Mission

The public identity is **DPM — DAG Project Manager**, targeting `luofang34/dpm`.
Never advertise an unpublished package as installable. Verification evidence belongs in commit
messages and hand-offs, not in tracked status documents.

DPM is a Rust execution-planning engine for humans and software agents. Its authoritative
model is a semantic execution graph: objectives, acceptance criteria, work, dependencies,
decisions, requirements, risks, artifacts, actors, and auditable state transitions.

A Gantt chart is a view. A Kanban board is a view. A Ratatui screen is a view. An agent tool call is
a view. Do not put authoritative project semantics into any UI adapter.

The v0.1 product question is:

> Given a project, can an agent reliably determine what it should do now, why that work matters,
> what counts as done, and what changes after the result is verified?

## Architectural invariants

1. **Domain before UI.** `dpm-model`, `dpm-schedule`, and `dpm-engine` must not depend on
   CLI, TUI, web, Apple, Android, database, or transport code.
2. **Commands mutate; queries observe.** UIs, agents, and future APIs must not directly update
   database rows or domain fields. Mutations flow through semantic commands in `dpm-engine`.
3. **Derived schedule data is never authoritative state.** Earliest/latest dates, float, critical
   path membership, percentiles, and criticality are recomputed from authoritative inputs.
4. **Readiness is derived.** A work item is executable only when lifecycle state, dependencies,
   decision gates, ownership, and blockers permit it. Do not persist a `ready = true` cache as
   domain truth.
5. **Acceptance is first-class.** Executable work requires an objective and concrete acceptance
   criteria. Agent completion means `Submitted`; verification is a separate transition.
6. **Uncertainty is first-class.** Prefer optimistic / most-likely / pessimistic duration estimates
   over false precision. Deterministic CPM and probabilistic simulation are complementary views.
7. **Human priority and schedule criticality are distinct.** Never conflate P0/P1 priority with
   zero float or Monte Carlo criticality.
8. **Decisions are gates, not comments.** If a decision blocks work, `next` must not recommend that
   work until the decision is resolved.
9. **Identity exists even locally.** Every operation has a Human, Agent, or Service actor. Future
   passkeys, Sign in with Apple, OIDC, and device keys attach credentials to principals; they do
   not redefine the domain actor model.
10. **Git is an artifact/version projection, not the live database.** Git commits and PRs may be
    linked deeply to work. Do not require ordinary collaborative edits to become Git merges.
11. **Offline/sync friendliness is a design constraint.** Operations carry stable IDs, actors,
    timestamps, and revisions. Do not add hidden process-local identity to persistent semantics.
12. **Explainability beats opaque ranking.** `next` may score candidates, but every factor must be
    inspectable and reproducible. Do not introduce an opaque ML ranker into core scheduling.

## Workspace boundaries

- `crates/dpm-model`: serializable domain types, identifiers, and pure validation.
- `crates/dpm-schedule`: pure deterministic/probabilistic schedule projections.
- `crates/dpm-engine`: validated commands, queries, readiness, `next`, and `explain`.
- `crates/dpm-store`: persistence adapters. SQLite is the v0.1 implementation.
- `src/` (root `dpm` package): human CLI and machine-readable JSON adapter.
- `crates/dpm-tui`: Ratatui operator console; it owns no business rules.
- `crates/dpm-sdk`: small public facade for stable library consumers.
- `examples/self-host/`: the sole user-facing example, a prepared unstarted roadmap.
- `tests/support/`: minimal synthetic regression input, never real project completion evidence.

Dependencies must point inward. In particular, model/schedule/engine must never import store/CLI/TUI.

## v0.1 scope

Implement and harden:

- nested projects and nested work items in the model;
- requirements, artifacts, decisions, risks, and Human/Agent/Service actors;
- FS/SS/FF/SF temporal dependencies with lead/lag;
- deterministic CPM and float;
- three-point estimates plus Monte Carlo P50/P80/P95 and criticality;
- semantic readiness and decision gates;
- `status`, `next`, `show`, `explain`, `claim`, `block`, `unblock`, `submit`, `verify`, `decide`;
- Git HEAD as an attachable artifact;
- SQLite snapshot + append-only semantic operation log;
- CLI JSON output and a small Ratatui operator console;
- realistic integration fixtures and tests.

## Explicit non-goals for v0.1

Do not implement these unless an accepted ExecPlan explicitly advances the milestone:

- Gantt editor or drag/drop scheduling UI;
- web, SwiftUI, Android, CloudKit, or hosted server;
- account registration, passkeys, Sign in with Apple, OIDC, RBAC;
- CRDT/Yrs live collaboration or presence;
- resource leveling/optimization, timesheets, earned value, or cost accounting;
- procurement/manufacturing-specific UI;
- Git as the authoritative database;
- LLM-generated scheduling decisions inside the deterministic core.

Preserve extension points for these, but do not create speculative abstractions with no current use.

## Agent workflow

When operating on a real DPM workspace, prefer the machine interface:

1. `dpm status --json`
2. `dpm next --json --capability <capability>`
3. `dpm explain <KEY> --json`; read `work.instructions.steps`, `in_scope`, `out_of_scope`,
   `verification`, objective and acceptance together with the resolved context;
4. `dpm claim <KEY> --actor agent:<name>`
5. perform the work and run its acceptance checks;
6. `dpm attach-git-head <KEY> --actor agent:<name>` when a commit is relevant;
7. `dpm submit <KEY> --actor agent:<name> --note "..."`;
8. a separate human/service verifier runs `dpm verify <KEY> ...`.

If work cannot proceed, use `block` with a concrete reason instead of silently switching scope.
If the plan itself is wrong, propose a plan change; do not route around dependencies in code.

## Engineering rules

- Prefer small, explicit types over unstructured strings for domain concepts.
- Keep serialization backwards-compatible once a public format is released.
- Validate graph invariants at command/import boundaries.
- Workspace lints forbid unsafe code and deny missing docs, unwrap/expect/panic, discarded Results,
  discarded futures, and awaiting with a synchronous lock. Tests may allow expect/panic explicitly.
- Use typed `thiserror` errors in libraries; retain error sources and useful entity/path context.
- Use `tracing` for diagnostics, propagate failures, and do not call `process::exit()`.
- Wrap monotonic counters with `wrapping_add(1)`.
- Keep Rust files at most 500 lines, functions at most 80 lines, and types at most 30 members.
  Keep `lib.rs` under 100 lines, starting with crate docs and containing only module/export declarations.
- Use domain-named modules and `foo.rs` plus `foo/`; never `mod.rs` or generic utility modules.
- Synchronous I/O functions use `_blocking`; cached queries advertise possible staleness in the name.
- Comments explain invariants and reasons, not refactor history, PR numbers, or changing line counts.
- Unit tests live in `src/<domain>/tests.rs`; integration tests use only public APIs.
- Regression tests accompany semantic changes. Synchronize concurrent tests with events, not sleeps.
- No network access in model/schedule/engine.
- Keep scheduling deterministic for identical inputs and seed probabilistic tests explicitly.
- Add tests for every non-trivial invariant or state transition.

Use the toolchain declared in `rust-toolchain.toml`. Before committing or pushing, run the complete
format → structural guardrails → Clippy → tests → docs → release → smoke pipeline:

```sh
./ci.sh
```

If the environment cannot run one of these commands, say so explicitly in the commit/hand-off; do
not claim the check passed.

## Licensing

All repository code is **AGPL-3.0-only** unless a file explicitly states otherwise. Contributions are
accepted under the [CLA](CLA.md), which grants the copyright holder relicensing rights for
application-store distribution. Third-party dependencies linked into DPM binaries must be permissively
licensed; copyleft third-party code cannot be relicensed and would block store builds. Do not copy
incompatible source code into this repository.

## CLI and agent-tool parity

- Execution queries and mutations flow through `dpm-app`; CLI/MCP are adapters.
- MCP structuredContent.data must equal the matching CLI JSON data. Add parity coverage to
  `scripts/smoke_agent.py` whenever an execution command or query changes.
- `explain` supplies resolved requirements, gates, risks, dependencies and evidence. Read this
  context before claiming the top-ranked eligible work; never infer priority from the Gantt picture.
- Gantt is a read-only projection of remaining elapsed hours. Navigation must not mutate the plan.
- Current scope is Gantt preview plus agent-understandable/operable work. Server/sync/auth and
  larger domain features require a separate accepted plan.

## Progress and Gantt navigation

- Use CLI `progress KEY PERCENT` or MCP `report_progress` for owned task reports. Reports use the
  same actor/revision/command boundary as other mutations; never update SQLite or lifecycle directly.
- A 100% execution report is not acceptance. Read `progress.verified`; submit and request independent
  verification. Percentages must not auto-unlock dependencies or scale scheduling estimates.
- Package/workspace progress uses equal-weight descendant leaf tasks; do not double-count nested
  packages or milestones. Milestone completion stays a binary prerequisite/gate projection.
- Horizontal Gantt navigation changes only the viewport; keys must preserve selection and the plan.
  Keep clipping and actual key-event coverage alongside any rendering change.

## Self-host example: prepared, not started

- The default `demo` is `examples/self-host/dpm-alpha.json`. Read its guide and use read-only
  status/next/show/explain/TUI until the user explicitly authorizes roadmap execution.
- DEC-EXECUTE is intentionally open and inherited by every task. Empty `next` is expected; never
  resolve a gate, claim a task or expand scope merely to make this example produce ready work.
- Current capability is TUI MVP plus agent task understanding/operations. Later core/history,
  scheduling, layout, server/sync/auth/native phases are contracts only, not implementation requests.
- Do not invent completion/verification records for existing code. Source artifacts describe context;
  the prepared acceptance tasks remain Planned, unowned and at 0% until actually performed.
- Preserve operational database history. Removing an unwanted demonstration database requires user
  authorization and checking that it has no operations; never silently replace it during startup.
- Self-host CI uses temporary storage and reads all 24 contracts through CLI/MCP. Lifecycle
  regressions import only `tests/support/execution-plan.json`; do not restore retired examples.
- Keep task steps, expected results, scope boundaries and verification checks concrete. These
  instructions describe work; they never override gates or authorize execution on their own.


## Gantt inspection and accessibility

- Full titles and all direct relationships must remain readable through wrapped, scrollable text.
  Truncated chart labels are acceptable only with this complete inspection path.
- Mouse hover is temporary inspection, never selection or a state mutation. Hit tests follow the
  rendered row offset and invalidate on resize. Keyboard focus and scrolling remain usable alone.
- Status colors and relation colors have separate roles (bars versus labels); pair every color with
  text/symbols, retain NO_COLOR support and test precedence on blocked/reviewed critical tasks.
- Restore mouse capture/raw mode on exit and errors. Do not route mouse events into domain commands.

## Project discovery

- CLI and MCP share `dpm-app` project resolution. Prefer `.dpm/project.toml`, never special-case
  a checkout name or silently initialize a sample when no project exists.
- The nearest locator wins; malformed/missing local configuration must not fall through to a
  parent project. Stop discovery at Git boundaries. Explicit database/project options override it.
- Normal projects keep SQLite and operations local; only locator/ignore rules belong in Git.
- This repository's locator selects an in-memory read-only preview. All adapters reject mutations
  on it; do not convert the preview to a live workspace merely to obtain executable tasks.

## Workspace and interchange boundaries

- Follow [the workspace design](docs/workspaces-and-collaboration.md) when implementing format,
  repository-binding or sync milestones. Its future contracts do not authorize their implementation.
- Workspace/project/task identity is independent of Git repositories and local directory paths.
  One workspace may span many repositories and non-code work; directory discovery is only selection.
- Local SQLite state and unsent operations are durable data, never disposable cache. Never infer
  that a tracked plan export contains all operation history or the newest state.
- Keep locator TOML separate from plan interchange TOML/JSON. Current plans use JSON; adding the
  intended TOML encoding requires a versioned DTO and semantic round-trip tests.
- Imports and sync must pass through the same command/validation boundary. No file watcher, raw
  SQLite replication, Git merge or CRDT may bypass it or silently overwrite running work.
- Keep machine-specific checkout bindings and credentials out of shared plans and tracked locators.
  Checkpoints/lineage, authentication, deduplication and durable pending work precede collaboration.
