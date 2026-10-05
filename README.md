# DPM — DAG Project Manager

DPM is a Rust engine that stores project work as a dependency graph of objectives, acceptance
criteria, tasks, decisions, requirements, risks and actors. People and software agents query and
change it through the same commands, and it answers:

- what can be done now, and why;
- what blocks it and what counts as done;
- when the project is likely to finish, and how uncertain that is.

Gantt charts, PERT networks, the terminal console, the macOS observer and agent tools are all views
of that graph; none of them stores schedule data.

**Status:** pre-alpha. Build from source; no packaged release is published.

## Features

- Nested projects and work packages, tasks and milestones, each with an objective and acceptance
  criteria.
- FS, SS, FF and SF dependencies with lead or lag, and working calendars.
- Critical path method: earliest and latest times, total and free float, and the critical
  activities and links.
- Optimistic, most likely and pessimistic estimates, with seeded Monte Carlo P50/P80/P95 finish
  times and per-task criticality.
- Readiness derived from dependencies, decision gates and ownership; `next` ranks ready work and
  shows every factor.
- Lifecycle commands: `claim`, `start`, `progress`, `block`, `submit`, `verify`, `decide`.
  Verification is a separate step by a different actor.
- SQLite storage with an append-only operation log, backup, restore and history replay.
- JSON CLI output, with an MCP server returning the same data.
- Microsoft Project XML (MSPDI) import and export.
- A terminal console and a read-only macOS observer, both with Gantt and network views.

Not in scope yet: editing in a Gantt chart, web or mobile apps, sync servers, accounts, resource
levelling, cost accounting and earned value.

## Quick start

Install the Rust toolchain pinned in `rust-toolchain.toml`. In this repository, `.dpm/project.toml`
opens DPM's own roadmap as a read-only preview:

```sh
cargo run -- status
cargo run -- explain MVP-10 --json
cargo run -- tui            # g opens the Gantt chart; ? there lists keys and the colour legend
```

The roadmap is deliberately gated by an open decision (`DEC-EXECUTE`), so `next` returns no work.
See [examples/self-host](examples/self-host/README.md).

Build the binaries with `cargo build --release` (`target/release/dpm` and `target/release/dpm-mcp`),
or install them with `cargo install --path .` and `cargo install --path crates/dpm-mcp`. For your
own project:

```sh
dpm validate plan.json      # check a JSON plan
dpm import plan.json        # creates .dpm/project.toml and an ignored local database from the plan
dpm next --json
dpm schedule --json         # critical path, float, critical links and finish percentiles
```

`dpm init "My project"` creates an empty workspace instead, and `dpm plan template` then prints a
minimal plan to fill in. Actor names such as `human:ada` or `agent:coder` are local identities, not accounts.

`dpm` finds the nearest `.dpm/project.toml`, stopping at a Git boundary; `--project DIR` or
`--database FILE` selects one explicitly. See [project discovery](docs/projects.md).

## Agents

Add `--json` to any command. Every reply is an envelope `{api_version, revision, lineage_id, data}`,
and a mutation's `data` is the recorded operation. The MCP server exposes the same execution
commands and queries as tools:

```sh
cargo run -p dpm-mcp -- --actor agent:NAME
```

An agent reads `explain` before claiming work, submits when done, and leaves verification to a
human or service. [docs/mcp.md](docs/mcp.md) lists every command and tool and their rules.

## macOS observer

On macOS 14 or later with Swift installed:

```sh
python3 scripts/build_native.py
open target/native/DPMObserver.app --args --project DIR   # or --database FILE
```

The observer is read-only. It shows what to do now, live agent runs, review, details, a Gantt chart
and a dependency network.

## Development

```sh
./ci.sh
```

`ci.sh` runs formatting, structure and repository checks, dependency licence and advisory checks,
Clippy, iOS and wasm client builds, tests, API docs, a release build and end-to-end smoke tests. It
requires Python 3.11+ and `cargo-deny` 0.19.8. `scripts/smoke_native.py` qualifies the macOS
observer.

`scripts/package_release.py --output DIR` builds an unsigned archive with `dpm`, `dpm-mcp`, the
licence, the corresponding source, a build manifest and a SHA-256 file;
`scripts/smoke_release.py DIR/*.tar.gz` tests it outside the checkout.

Read [AGENTS.md](AGENTS.md) for the architectural rules and
[docs/architecture.md](docs/architecture.md) for how data flows.

## License

Copyright © 2026 Fang Luo. Licensed under the GNU Affero General Public License v3.0 only
(`AGPL-3.0-only`); see [LICENSE](LICENSE). Contributions are accepted under the
[CLA](CLA.md); see [CONTRIBUTING](CONTRIBUTING.md).
