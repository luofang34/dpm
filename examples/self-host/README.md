# DPM manages dpm — prepared example

This repository's `.dpm/project.toml` selects `dpm-alpha.json` as a read-only preview. It is the
sole user-facing example and the project's own task graph. No database initialization is needed:

```sh
cargo run
cargo run -- status --json
cargo run -- explain MVP-30 --json
cargo run -- explain SEM-20 --json
```

Each task carries ordered steps, expected results, scope boundaries, acceptance checks, capability
requirements, linked constraints and source context. Use TUI/`show`/`explain` to read these records;
this guide does not duplicate their task inventory or design choices.

The MVP acceptance covers graph/store integrity, Gantt inspection and agent operations. Current
commands include contract ratification, independent rejection/resubmission, resource-linked Git
HEAD evidence and device-local workspace bindings. See [CLI/MCP](../../docs/mcp.md) and
[project selection](../../docs/projects.md) for the supported interfaces.

`M8-SEMANTICS` groups dependency policies, event-based execution lag, provisional submission bases,
conditional joins, scoped queries, external references and interchange work. The declared MVP
boundary includes these contracts and their prerequisites `CORE-20` and `SCH-10`: `M0-MVP`
requires them through the `MVP-20`/`MVP-30` reviews. `DEC-EXPAND` records authorization for that
scope; its question is not approval. A regression test pins the exact MVP prerequisite set, so
changing it is a deliberate plan change. Implementation boundaries and edge-case scenarios live
in the task contracts.

Publication requires independent qualification of replay/recovery and format compatibility through
`CORE-40` and `CORE-50`. Design questions name remaining choices, and `explain` distinguishes their
context links from explicit execution gates. Inspect each decision's `blocks` rather than inferring
permission from its title or presence in a guide.

## Execution and evidence

The prepared acceptance tasks are Planned; proposed extensions are Proposed and require
ratification in an authorized live workspace. All are unowned with zero reported progress, and
the authorization gates keep `next` empty. Qualification contracts link immutable implementation
and regression sources and ask reviewers to reproduce behavior and repair demonstrated gaps.
Those sources establish what can be inspected, not independent acceptance. Re-scoped and proposed
work has no duration estimate until assessed; forecasts expose it as unestimated.
The console labels the source `PREVIEW read-only`; CLI/MCP execution commands return
`read_only_project` and create no SQLite state.

An explicitly authorized live self-host cycle requires a separate operational workspace. `demo` or
`import` can initialize an absent store; neither overwrites an existing one or resolves its gates.
Local actor names express attribution and do not provide authentication. Submission and independent
verification remain separate operations. Exported JSON does not include operation history.

Source artifacts carry `role = planning_source` and describe where to inspect relevant code or
discussion. `repo:PATH` resolves in this checkout; `git:COMMIT:PATH` resolves in Git history. A source
revision identifies the implementation being qualified; execution evidence must identify the exact
candidate and configuration actually checked. These references never prove that acceptance was
performed. Estimates and risks are provisional planning inputs.

`SELF-20` supplies the bounded maintenance scenario: inspect context, claim and explicitly start,
review any semantic plan proposal, perform the change, attach evidence and submit. `SELF-30` audits
the real trace independently. Native observation reuses that scenario through `UI-50`; synthetic
regression runs check the protocol but never count as a live self-host cycle.

The local native milestone `M9-LOCAL` follows the facade, run observation, Swift bridge and maintenance
pilot. It does not require terminal redesign, hosted sync, steering or configuration management.
Those have their own contracts and acceptance boundaries. New design decisions describe proposed
choices for review; they do not authorize implementing the proposed features.

## Updating the project

Edit the existing Plan's task contracts, requirements and relevant decisions, preserving stable IDs.
Before publication, move bundled inputs directly to the chosen domain format without legacy adapters.
Use `dpm validate examples/self-host/dpm-alpha.json --json`; update the expected projection only for
an intentional graph/query change. Operational databases retain their own revision/history and are
not synchronized by editing this preview file. Reviewed in-place plan changes are tracked by `CORE-20`.

CI validates every contract and reads it through real CLI/MCP processes, checks the exact MVP
prerequisite set and the gates on later phases, and asserts revision zero with no operations or
claimed work.
Execution regressions use disposable synthetic input under `tests/support/`, not self-host tasks.
