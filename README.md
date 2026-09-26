# DPM — DAG Project Manager

**Agent-first execution planning for humans and machines.**

DPM is an experimental Rust engine for representing real project work as an executable semantic
graph. It is designed so a coding agent, project lead, procurement operator, or future native/web UI
can ask the same core questions:

- What can be done now?
- Why is this the right work now?
- What blocks it?
- What does “done” mean?
- Which requirement or decision caused this work to exist?
- What changes when the result is verified?
- How uncertain is the completion date?

DPM is **pre-alpha**. The current milestone is deliberately narrow and intended to validate the
domain model and agent execution loop before building broad collaboration/UI features.

The repository and command name is `dpm`; the formal title is **DPM — DAG Project Manager**.
This source checkout is pre-alpha. No registry or package-manager release is advertised as available.

## Gantt is a projection

DPM does not define a project as a Gantt chart. The authoritative model is objectives,
acceptance criteria, work, dependencies, decisions, requirements, risks, actors, and artifacts.
Timelines, network diagrams, Gantt charts, Kanban boards, native apps, and agent tools are projections
of the same model.

## Current MVP

- nested Projects and WorkItems in the domain model;
- WorkPackage / Task / Milestone work kinds;
- explicit objectives and acceptance criteria;
- owner-reported task percentages and derived package/workspace progress;
- Requirements, Decisions, Risks, and Artifacts;
- Human / Agent / Service actor identities;
- FS, SS, FF, SF dependencies with lead/lag;
- deterministic CPM, float, and critical activities;
- O/M/P duration estimates;
- seeded Monte Carlo P50/P80/P95 completion risk and activity criticality;
- derived readiness plus explicit decision gates;
- explainable `next` ranking;
- semantic `claim`, `start`, `block`, `unblock`, `submit`, `verify`, and `decide` commands;
- SQLite snapshot + append-only semantic operation log;
- Git HEAD artifact linkage;
- human CLI plus structured JSON and matching MCP tools;
- Ratatui `Now / Work / Network / Detail / Gantt` operator console.

## Workspace

```text
crates/
  dpm-sdk/       public facade
  dpm-model/     authoritative domain types
  dpm-schedule/  pure scheduling projections
  dpm-engine/    commands, queries, readiness, next/explain
  dpm-store/     SQLite persistence + operation log
  dpm-app/       shared query/command application service
  dpm-mcp/       principal-bound stdio agent tools
  dpm-tui/       Ratatui operator console
src/                `dpm` CLI adapter
.dpm/project.toml   committed read-only preview locator
examples/self-host/  prepared DPM roadmap
tests/support/      minimal synthetic input for isolated regression tests
```

See [`AGENTS.md`](AGENTS.md) for architectural invariants and [`docs/architecture.md`](docs/architecture.md)
for the data-flow model.

## Run a project or preview this repository

Install the pinned Rust **1.98.1** toolchain (`rust-toolchain.toml`). From this repository:

```sh
cargo run
cargo run -- status --json
cargo run -- next --json
cargo run -- explain MVP-10 --json
```

The committed `.dpm/project.toml` points to the **DPM Alpha** self-host JSON plan. Opening it reads
an in-memory preview, creates no database, and rejects modifications through both CLI and MCP.
The console labels it `PREVIEW read-only`. `cargo run tui` also works; root package `dpm` owns
the CLI, while `cargo run -p dpm-mcp -- --actor agent:reader` starts the MCP adapter.

The preview contains task contracts, milestones, requirements, open decision gates, risks and
source context. `next` deliberately returns no candidates: **DEC-EXECUTE** gates the entire roadmap. All
contracts remain unowned at zero progress, and no execution is authorized by this example.
See [the self-host guide](examples/self-host/README.md) for scope and acceptance details.

After building/installing the CLI, ordinary projects use:

```sh
dpm init "My project"
dpm status --json
dpm
```

`dpm` searches the current directory and its parents for `.dpm/project.toml`, stopping at a Git
boundary. Subdirectories open the closest project. No project means an initialization hint, not
an automatic demo. `--project DIR` selects an exact project; `--database PATH` opens a specific
SQLite file. The two overrides are mutually exclusive. `dpm --json` without a subcommand returns
status rather than opening a terminal.

Normal projects store their plan and operations in ignored `.dpm/state.sqlite`. Commit the locator,
not the database. Preview and database sources share the same query/command boundary; preview
sources additionally reject every project mutation. [Project discovery and initialization](docs/projects.md)
documents nested projects, cloned configurations and existing databases.

A workspace may span several Git repositories and non-code projects. Use explicit project/database
selection for a central local plan; automatic discovery remains conservative at Git boundaries.
SQLite holds durable runtime state. Plan interchange uses JSON; TOML configures project locators.
`dpm next --project-key KEY --resource-key KEY` narrows globally ranked work to a project subtree or
resources without hiding eligible work elsewhere; see the [agent contract](docs/mcp.md#scoped-next).
Inspect `CORE-20` and `SYNC-10` with `explain` for semantic plan editing and sync requirements.
Current resource identities and local bindings are described in the project-selection guide.

Self-host is the only bundled example. `demo` explicitly initializes a copy in another project or
an explicit database; it never overwrites existing state. Each task includes ordered actions and
expected results, scope boundaries, acceptance criteria and verification checks. `show/get_work`
exposes the contract; `explain/explain_work` also resolves requirements, decisions, risks,
dependencies and evidence. Internal mutation tests use a minimal synthetic graph.

Add `--json` to queries and mutations for agent consumption. Mutation responses contain the
persisted operation and revision. Operational errors return a JSON error object and a nonzero exit
status. The CLI text format is not an API; agents
should consume structured JSON or the [MCP adapter](docs/mcp.md) over the same application service.

## Local plans and execution rules

`init`, `demo`, and `import` explicitly initialize local project state. They refuse to replace an
existing database or preview. Queries never create a missing database. Use `--database PATH`
for a separate disposable copy, or initialize a different project directory.

```sh
cargo run -p dpm -- validate examples/self-host/dpm-alpha.json --json
cargo run -p dpm -- --database ./example.sqlite import examples/self-host/dpm-alpha.json
cargo run -p dpm -- --database ./example.sqlite export > example.json
```

Imported plans are checked for unique keys, valid references, containment/dependency cycles, finite
ordered estimates, acceptance contracts, and consistent lifecycle/ownership. Import/export operate
on authoritative snapshots; export does not include the operation history. To edit a live workspace,
export a candidate, inspect `dpm plan diff candidate.json`, then apply it with
`dpm plan apply candidate.json --reason "Clarify scope" --actor human:planner`.
New tasks enter as Proposed; `ratify` approves complete contracts. Plan changes preserve execution,
evidence and existing decisions. Read the append-only audit with `dpm history --json`. The console is a read-only snapshot: use `1`–`5` to
switch views (`5` or `g` opens Gantt), arrows or `j`/`k` to select work, Enter for details, and `q` to
quit. Press `r` to reload agent changes while retaining the selected work and Gantt viewport.
Detail shows the full steps, scope, acceptance, review and linked context; scroll to read long contracts.
A failed reload keeps the last valid view and displays its revision with the error.

Only planned tasks can be claimed. A claim reserves work; `start` begins it and records the start
event, and progress reports and submission require it. Submitted work requires a different actor to
verify it. Blocking and resuming claimed work retains its owner, and started work resumes started. Nonempty capability filters are eligibility constraints.
Decision gates on a work package also gate its descendants. Milestones complete when all their
prerequisites complete; work packages complete when all children complete. These aggregate statuses
are derived in `status`, `show`, `explain`, and the console, and never written into stored work state.
An empty package or an unconstrained milestone is not implicitly complete.

FS/SS/FF/SF relationships and positive/negative lag are supported by CPM and simulation. Execution
gates follow the relation: FS and SS govern claiming and starting the successor, FF and SF its
submission, and verification re-checks all four. They wait for the predecessor's recorded start or
verification plus any positive lag in elapsed time; a lead never releases work before the event. See
[architecture](docs/architecture.md#temporal-bounds-are-not-execution-permission). Remaining projections
zero completed task durations and omit their historical dependency constraints, including lag.
Unfinished dependency constraints retain their lag. Use task or milestone endpoints for temporal
constraints; work packages group work and do not accept temporal dependency edges.

SQLite commits each semantic operation and resulting snapshot in one transaction, checks revisions,
and rejects stale writers or snapshots that do not match the operation. Actor names are local
identities, not authenticated accounts. Risk records provide context; Monte Carlo samples the task
duration estimates and does not apply separate risk-event distributions or resource calendars.

## What v0.1 intentionally does not do

No Gantt editor, web app, SwiftUI/Android app, CloudKit, hosted server, passkeys/OIDC, live CRDT
collaboration, resource leveling, earned value, timesheets, or cost accounting yet. Those features
must build on the core command/query and operation semantics rather than bypass them.

## Development

The declared minimum and CI toolchain are **Rust 1.98.1**, using Rust edition 2024.

```sh
./ci.sh
```

The quality gate checks formatting, source limits, repository hygiene, Clippy, all targets, API documentation,
release compilation, and a disposable-database CLI workflow and project-discovery/preview parity.

The self-host integration checks are read-only and require revision 0 with an empty operation log.
The internal synthetic graph exercises the agent/human mutation loop, gates and downstream work.
Read-only checks compare every self-host task contract through actual CLI and MCP processes.
Keep future changes in task scope/acceptance and related requirements/decisions; inspect them with
`cargo run -- explain CORE-10 --json`.

## Name

The formal name is **DPM — DAG Project Manager**; the intended GitHub repository is
`luofang34/dpm`, and the intended CLI is `dpm`. The execution graph remains the core; Gantt and
agent tools are projections. Naming and package-manager distribution are separate steps: a short
GitHub name does not reserve an upstream package name. Release qualification is tracked as work in
the [self-host roadmap](examples/self-host/README.md), not as a status document.

## License

Copyright © 2026 Fang Luo.

GNU Affero General Public License v3.0 **only** (`AGPL-3.0-only`). See [`LICENSE`](LICENSE).
Contributions require the [Contributor License Agreement](CLA.md), which also permits the copyright
holder to distribute application-store builds; see [CONTRIBUTING](CONTRIBUTING.md).

## Gantt preview and agent interface

Start the console with `cargo run -p dpm -- tui`; press `5` or `g` for the Gantt preview.
`1`–`4` retain Now / Work / Network / Detail. Use **↑/↓** or `j`/`k` to select work, Enter for
its contract, and `q` to quit. With the chart focused, **←/→** pan the time window by one quarter,
`+/-` zoom, `Home/End` jump to the start/end, and `f` fits the whole schedule. Task labels stay fixed
while the timeline moves. The initial window shows up to 48 hours.

The inspector below the chart wraps the **full task title** and lists **all direct predecessors and
successors**, including their full titles, dependency direction, FS/SS/FF/SF type, signed lag,
`[soft]`/`[soft, waived]`/`[provisional start]` tags and each link's release state at the snapshot
clock, as the shared gate evaluator derives it: which transition it gates (start for FS/SS, submit and verify
for FF/SF), and whether it awaits an event, has lag elapsing, is released (provisionally on a
pending attempt), or is waived. Decision gates on the task are listed with their state. `Tab` switches focus between chart and inspector; inspector **↑/↓**, `j/k`,
`Home/End` scroll its text. `PgUp/PgDn` scroll inspector pages from either focus. A scrollbar and
line range indicate hidden content. Detail/Network/Now also support `PgUp/PgDn` and `Home/End`.

**Hover** previews a row's title and relationships without moving keyboard selection. **Click**
selects it. The mouse wheel selects rows over the chart and scrolls text over the inspector. Keyboard
input returns to the selected task; click the hovered row before using Enter for its Detail page.
Mouse motion requires terminal support; keyboard inspection remains fully usable. Mouse capture is
released on exit; terminal text selection commonly uses Shift while capture is active.

Press `?` for scrollable help and the full color legend; `Esc` closes help or returns inspector focus
to the chart. Otherwise `Esc`/`q` quits. `NO_COLOR=1 cargo run tui` keeps text/symbols without colors.

| Indication | Meaning |
| --- | --- |
| Red `#` | Critical task |
| Yellow `!` | Explicitly blocked task |
| Magenta `?` | Submitted, awaiting independent review |
| Green `.` | Verified task |
| Cyan `=` / gray `-` | Other task / work package |
| Yellow `◇` / green `◆` | Pending / reached milestone on the timeline |
| Name prefix `◇[M]` / `◆[M]` | Pending / reached milestone, visible in Gantt and Work even when its time point is off-screen |
| Cyan label `<P` / magenta label `>S` | Direct predecessor / successor of the inspected task |
| Reversed/bold `@` / underlined `~` | Keyboard selection / temporary hover preview |

Status colors take precedence over criticality: verified, blocked, then review, then critical. The
inspector separately displays `critical=true/false`. Relationship colors apply to labels; bar colors
continue to describe task status. Off-screen related tasks remain in the complete inspector list.

The timeline shows **remaining elapsed hours from the snapshot**. `#` marks critical tasks,
`=` other tasks, `-` containers, and `◇/◆` pending/reached zero-duration milestones. `<` and `>`
indicate bars continuing outside the window. The inspector shows FS/SS/FF/SF dependency types
and signed hour lag; Detail and Network show full relationship names. Positive lag is delay,
negative lag is lead. Reopen the console to load a newer database revision.

The percentage column displays reported task progress; submitted work displays 100% execution.
**100% does not mean verified** and never unlocks dependencies by itself. Packages and the workspace
average descendant leaf tasks equally, without double-counting nested packages or milestones;
milestone percentages are binary and derive from their verified prerequisites and decision gates.
Progress does not reduce duration estimates or infer calendar/time spent.

Regression tests construct partial progress and pending/reached milestone cases in disposable
snapshots. The self-host example remains at 0% until its contracts are actually performed.

The chart never changes the execution graph or persists derived dates. A future calendar/Gantt
editor can reuse these projections without replacing the authoritative model.

Agents use the same engine and application service as CLI. For the prepared self-host workspace,
query `project_status`, `next_work`, `get_work` and `explain_work` through MCP; do not initiate its
execution merely because mutation tools are available.

```sh
cargo run -p dpm-mcp -- --actor agent:reader
```

The MCP command is a stdio server launched by an MCP client. [Agent/CLI contract](docs/mcp.md)
documents supported tool names, revision checks and independent verification. `explain` resolves
requirements, decision gates, risks and predecessor evidence so agents can inspect each contract.

This increment deliberately defers server/sync, auth enrollment, undo, resource leveling,
calendar expansion and rich UI. AGPL-3.0-only and the core/store/adapter boundaries are unchanged.

Use `dpm ratify KEY --actor human:reviewer` to approve a Proposed contract.
Use `dpm reject KEY "unmet acceptance" --actor human:reviewer` to return Submitted work
to its owner. `show` and `explain` retain the latest rejection across resubmission.

The local quality gate requires Python 3.11+ and `cargo-deny`: install the latter with
`cargo install cargo-deny --locked --version 0.19.8`, then run `./ci.sh`.
License checks use resolved Cargo metadata, including workspace inheritance; security checks
refresh the advisory database and fail if the check cannot complete.
