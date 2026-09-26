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
conditional joins, scoped queries, external references and interchange work. These contracts, with
their prerequisites `CORE-20` and `SCH-10`, are part of the MVP: `M0-MVP` requires them through the
`MVP-20`/`MVP-30` reviews. `DEC-EXPAND` approves that scope, and `DEC-POST-MVP` gates every later
phase. A regression test pins the exact MVP prerequisite set, so changing it is a deliberate plan
change. Implementation boundaries and edge-case scenarios live in the task contracts.

## Execution and evidence

The prepared tasks remain Planned, unowned and at zero reported progress. All authorization gates
remain open, so `next` is empty. These acceptance tasks have not been independently performed merely
because related implementation exists. The console labels the source `PREVIEW read-only`; CLI/MCP
execution commands return `read_only_project` and create no SQLite state.

An explicitly authorized live self-host cycle requires a separate operational workspace. `demo` or
`import` can initialize an absent store; neither overwrites an existing one or resolves its gates.
Local actor names express attribution and do not provide authentication. Submission and independent
verification remain separate operations. Exported JSON does not include operation history.

Source artifacts describe where to inspect relevant code or discussion. `repo:PATH` resolves in this
checkout; `git:COMMIT:PATH` resolves in Git history. These references are planning context, not proof
that acceptance was performed. Estimates and risks are provisional planning inputs.

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
