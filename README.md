# DPM — DAG Project Manager

DPM keeps a project as a dependency graph of objectives, acceptance criteria, tasks, decisions,
requirements, risks and actors. People and software agents read and change it through the same
commands, and it answers:

- what can be done now, and why;
- what blocks it and what counts as done;
- when the project is likely to finish, and how uncertain that is.

> **⚠️ Pre-alpha.** DPM is under active development and not ready for work you depend on. The plan
> format, the SQLite store, the JSON API (`api_version`) and the command line can all change
> between releases, and a newer DPM refuses an older store rather than migrating it. Keep backups
> (`dpm backup --to FILE`) and expect to re-import your plans.

![The macOS app's Gantt chart of the roadmap `dpm demo` loads: critical bars in red, milestones, and the P80/P95 finish lines](https://raw.githubusercontent.com/luofang34/dpm/main/docs/images/gantt.png)

| Dependency network | Terminal console |
| --- | --- |
| ![The macOS app's dependency network of DPM's live roadmap, with the selected task's links explained](https://raw.githubusercontent.com/luofang34/dpm/main/docs/images/network.png) | ![The terminal console's Gantt view of the remaining work](https://raw.githubusercontent.com/luofang34/dpm/main/docs/images/tui.png) |

Gantt charts, the dependency network, the terminal console, the macOS app and the agent tools are
all views of one graph; none of them stores schedule data.

## Features

- Nested projects and work packages, tasks and milestones, each with an objective and acceptance
  criteria.
- FS, SS, FF and SF dependencies with lead or lag, and working calendars.
- Critical path method: earliest and latest times, total and free float, drag, and the critical
  activities and links.
- Optimistic, most likely and pessimistic estimates, with seeded Monte Carlo P50/P80/P95 finish
  times, per-task criticality and the classical PERT approximation beside it.
- Readiness derived from dependencies, decision gates and ownership; `next` ranks ready work and
  shows every factor.
- Lifecycle commands: `claim`, `start`, `progress`, `block`, `submit`, `verify`, `decide`.
  Verification is a separate step by a different actor.
- SQLite storage with an append-only operation log, backup, restore and history replay.
- JSON output for every command, and an MCP server (`dpm mcp`) returning the same data.
- Microsoft Project XML (MSPDI) import and export.
- A terminal console and a macOS app, both with Gantt and network views; the app can also edit the
  plan through reviewed changes.

Not in scope yet: web or mobile apps, sync servers, accounts, resource levelling, cost accounting
and earned value.

## Install

```sh
cargo install --locked dpm
```

This installs one `dpm` binary: the CLI, the terminal console (`dpm tui`) and the agent server
(`dpm mcp`). It needs Rust 1.98.1 or later. The macOS app is built from a checkout (see
[macOS app](#macos-app)).

## A first project

```sh
mkdir website && cd website && git init
dpm init "Website launch"
dpm plan template > plan.json      # a small plan to edit: tasks, a milestone, a decision gate
dpm plan apply plan.json --reason "First plan" --actor human:ada
dpm ratify TEMPLATE-DESIGN --actor human:ada
dpm ratify TEMPLATE-BUILD --actor human:ada
```

Every change carries an actor (`human:NAME`, `agent:NAME` or `service:NAME`); these are local
identities, not accounts. New tasks start as `Proposed` and become executable once ratified.

`next` ranks the work that can start now and says why:

```console
$ dpm next
#1 TEMPLATE-DESIGN  Write the design
  score 137.6, float 0.0h, downstream 2
  start gates are satisfied: every FS/SS predecessor event and positive lag has elapsed;
  criticality basis is simulated: critical in 100% of schedule simulations; unblocks 2 downstream
  work items; requested capabilities match; drag 4.3h is reported but not ranked
```

Work moves through claim, start and submit, and someone else verifies it:

```console
$ dpm claim TEMPLATE-DESIGN --actor agent:coder        # each change prints its recorded operation
$ dpm start TEMPLATE-DESIGN --actor agent:coder
$ dpm submit TEMPLATE-DESIGN --actor agent:coder --note "Design in docs/design.md"
$ dpm verify TEMPLATE-DESIGN --actor agent:coder
ERROR the submitting actor cannot verify its own work 78eed09c-…
$ dpm verify TEMPLATE-DESIGN --actor human:ada
```

The build is still held by an open decision, and `explain` says so:

```console
$ dpm explain TEMPLATE-BUILD --json | jq -c .data.gates
{"ready":false,"transition":"claim","unmet":[{"key":"TEMPLATE-DEC","type":"decision"}]}
$ dpm decide TEMPLATE-DEC simple --actor human:ada
$ dpm next
#1 TEMPLATE-BUILD  Build the result
  score 135.2, float 0.0h, downstream 1
  …
```

Other useful commands:

```sh
dpm status                  # counts, progress and P50/P80/P95 finish
dpm schedule --json         # critical path, float, critical links and finish percentiles
dpm status --horizon 40     # probability of finishing within 40 hours
dpm tui                     # the console; g opens the Gantt chart, ? lists keys
dpm history                 # the operation log
dpm plan import-mspdi project.xml --project-key WEB   # Microsoft Project XML as a reviewed candidate
```

`dpm` finds the nearest `.dpm/project.toml`, stopping at a Git boundary; `--project DIR` or
`--database FILE` selects one explicitly. See [project discovery](docs/projects.md).

## Agents

Add `--json` to any command. Every reply is an envelope `{api_version, revision, lineage_id, data}`,
and a mutation's `data` is the recorded operation. `dpm mcp` serves the same commands and queries as
MCP tools over stdio, bound to one actor:

```json
{
  "mcpServers": {
    "dpm": { "command": "dpm", "args": ["mcp", "--project", "/path/to/project", "--actor", "agent:claude"] }
  }
}
```

An agent reads `explain` before claiming work, submits when done, and leaves verification to a
human or service. [docs/mcp.md](docs/mcp.md) lists every command and tool and their rules.

## macOS app

On macOS 14 or later with Swift installed, from a checkout:

```sh
python3 scripts/build_native.py
target/native/DPMObserver.app/Contents/Helpers/dpm gui --project DIR   # or --database FILE
```

The app shows what to do now, live agent runs, review, details, a Gantt chart and a dependency
network. With Edit on (⌘E), dragging from a bar's end to another bar links them, dragging a whole
bar offers a lag on the link that drives it, Option-dragging a bar's finish changes its estimate,
and an inspector edits the selected item and runs its task operations. Plan edits gather in a draft
and apply only after the application's diff is reviewed, with a reason.

`dpm` alone opens the console in a terminal and the app otherwise; `dpm tui` and `dpm gui` choose
explicitly.

## DPM's own roadmap

In this repository, `.dpm/project.toml` opens DPM's own roadmap as a read-only preview:

```sh
cargo run -- status
cargo run -- explain MVP-10 --json
cargo run -- tui
```

The roadmap is gated by an open decision (`DEC-EXECUTE`), so `next` returns no work. See
[examples/self-host](examples/self-host/README.md).

## Development

```sh
./ci.sh
```

`ci.sh` runs formatting, structure and repository checks, dependency licence and advisory checks,
Clippy, iOS and wasm client builds, tests, API docs, a release build and end-to-end smoke tests. It
requires Python 3.11+ and `cargo-deny` 0.19.8. `scripts/smoke_native.py` qualifies the macOS app.

`scripts/package_release.py --output DIR` builds an unsigned archive with `dpm`, the licence, the
corresponding source, a build manifest and a SHA-256 file; `scripts/smoke_release.py DIR/*.tar.gz`
tests it outside the checkout.

Read [AGENTS.md](AGENTS.md) for the architectural rules and
[docs/architecture.md](docs/architecture.md) for how data flows.

## License

Copyright © 2026 Fang Luo. Licensed under the GNU Affero General Public License v3.0 only
(`AGPL-3.0-only`); see [LICENSE](LICENSE). Contributions are accepted under the
[CLA](CLA.md); see [CONTRIBUTING](CONTRIBUTING.md).
