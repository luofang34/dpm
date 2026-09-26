# DPM manages dpm — prepared example

This is the default `dpm demo` plan. It is a **prepared roadmap, not a started project**.
It uses the existing domain model, with 24 task contracts, 9 work packages, 8 zero-duration
milestones, 10 requirements, 4 open decisions, 5 risks and 9 context artifacts.

The implementation currently supports **TUI MVP plus agent task understanding and operations**.
The longer roadmap is planning data only. Do not claim, report progress, submit, verify, resolve
its authorization gates or implement its tasks without a separate user instruction to begin.

## Is self-hosting possible now?

Yes, as a principal example and an inspectable execution plan. The current code can validate/import
this graph, persist it in SQLite, answer status/next/show/explain, render its Gantt/terminal views,
and expose matching execution operations through CLI/MCP. Existing regression fixtures exercise
those operations. That does **not** mean a real self-host cycle has already happened.

The current source also supports milestone conditions, FS/SS/FF/SF+lag projections, progress reports,
Git HEAD evidence and independent-actor verification. These are available capabilities to review,
not a reason to mark the roadmap's acceptance tasks as already verified.

Full in-place plan proposals/edits, user-facing history/diff/revert, calendar/resource management,
network sync, authenticated accounts and native/Web products are not implemented. Keep them as
later contracts. A source export is not an operation-history backup; do not replace an existing
workspace to simulate an in-place plan edit.

## Preview only

From the repository root, `cargo run` selects the CLI by default and uses
`.dpm/dpm.sqlite`. On a fresh checkout initialize it once, then use read-only queries:

```sh
cargo run -- validate examples/self-host/dpm-alpha.json --json
cargo run -- demo
cargo run -- status --json
cargo run -- next --json
cargo run -- explain MVP-10 --json
cargo run -- tui
```

Run `demo` only once for that path; an initialized self-host workspace can be opened directly with
`cargo run tui`. `import examples/self-host/dpm-alpha.json` initializes the same graph. Neither
command overwrites existing workspaces; use `--database PATH` for a separate workspace. Seed edits
do not silently update operational databases.

Expected results: revision 0, 41 work items, 0 ready, 0 complete, 4 open decisions, no owners and
an empty operation log. `next` returns `[]` deliberately. `explain MVP-10` resolves the root
`DEC-EXECUTE` gate, acceptance conditions, source context, requirements and risks.

Press `5`/`g` for Gantt, arrows to select/pan, `+/-` to zoom, `f` to fit, Enter for details, and `q`
to exit. Hover or select a task to read its full title and predecessor/successor list below the chart.
Click selects; `Tab` focuses the inspector, `↑/↓` or `PgUp/PgDn` scrolls long titles and relationships.
`?` explains status colors, relation markers and keyboard/mouse controls. Colors always have textual
or symbolic counterparts; `NO_COLOR=1 cargo run tui` disables color. The chart is the prospective remaining roadmap, not a history of finished implementation.
Its 0% indicates these contracts have not been performed or independently accepted. It does not
mean the current code has no functionality. Dates, floats and projections are never seed state.

## Milestones and contracts

All tasks are `Planned`, unowned, at 0% reported progress. Here Planned means the contract is
written down; it is not execution authorization. Work packages and milestones also remain Planned.
No claimed/submitted/verified history or independent-review identity is invented.

| Milestone | Task contracts | Acceptance meaning | Scope |
| --- | --- | --- | --- |
| M0-MVP | MVP-10, MVP-20, MVP-30 | Independently assess graph/store, TUI preview and agent operations already present | Current capability review, not yet performed |
| M1-SELF | SELF-10, SELF-20, SELF-30 | Pin a real baseline; perform one separately approved small task through dpm; audit its evidence | First live self-host cycle, not started |
| M2-CORE | CORE-10, CORE-20, CORE-30 | Versioned schemas, reviewed plan changes, history/diff/append-only revert | Deferred expansion |
| M3-SCHEDULE | SCH-10, SCH-20, SCH-30 | Temporal/float contract, calendars and milestone uncertainty | Deferred expansion |
| M4-TUI | TUI-10, TUI-20, TUI-30 | Approve, implement and review a focused terminal layout | Deferred layout work |
| M5-ALPHA | QA-10, QA-20, QA-30 | Cross-domain fixtures, scoped quality evidence and public release preparation | Deferred qualification |
| M6-COLLAB | SYNC-10, AUTH-10, SERVER-10, SYNC-20 | Validated operation exchange, identity boundary and optional server | Optional, separately scoped |
| M7-DESIGN | UI-10, COLLAB-10 | Native/Web contracts and rich-text/CloudKit boundaries | Design contracts, not platform products |

MVP-10 precedes the parallel MVP-20/30 reviews; both feed M0. M0 precedes the first self-host cycle
and the separately gated layout track. M1 precedes core/scheduler expansion. M2/M3 feed alpha
qualification, while M2 separately precedes optional collaboration. These are real causal FS edges
with zero lag; the self-host plan does not add artificial SS/FF/SF edges merely to showcase symbols.
Focused scheduling and terminal regression tests cover all relationship kinds on synthetic inputs.

Every task has an objective, three ordered steps with expected results, explicit `in_scope` and
`out_of_scope` boundaries, verification checks, three observable acceptance criteria, capabilities,
requirement links, source artifacts and provisional optimistic/likely/pessimistic hours. These are rough planning
inputs to re-estimate before execution, not calendar commitments, measured work or resource-leveled
dates. Risk probabilities are provisional planning judgments, not observed frequencies.

## Gates and source context

| Decision | Blocks | Meaning |
| --- | --- | --- |
| DEC-EXECUTE | WP-DPM and every descendant | A separate instruction must authorize starting the self-host contracts |
| DEC-BASELINE | SELF-10, QA-30 | Establish the actual committed source, database and independent reviewer |
| DEC-LAYOUT | WP-4-LAYOUT | Accept a later layout scope |
| DEC-EXPAND | Core, schedule, quality, collaboration and human-projection packages | Explicitly approve expansion beyond the current MVP |

Gates are separate from technical capability. Do not resolve one merely because `next` is empty.
The MVP trusts local actor identities; these gates express planning intent and are not an external
authentication system. Future credentials must not be confused with the seed's artifact author.

`repo:PATH` artifacts refer to files relative to this repository. They are source context, not
completion evidence. Their `created_by` service only records who prepared the references, not a
reviewer. Source references are not claimed public releases, review approvals or execution evidence.

After an explicit later start instruction, use the existing operation names from
[the CLI/MCP contract](../../docs/mcp.md). No `start`, `propose-change`, `history` or `revert` command
is implied to exist today. `progress` supports an owned task's execution report; `attach-git-head`
links evidence; `submit` and `verify` remain distinct actors/steps. This preparation does not run them.

## Maintaining the seed

- `dpm-alpha.json` is an existing `Plan` document, not a parallel planning format.
- Keep UUIDs stable when editing contracts; do not regenerate IDs for cosmetic changes.
- Validate edits and update `dpm-alpha.expected.json` only when the intended query contract changes.
- Existing operational databases remain authoritative for their own revisions. Seed edits do not
  synchronize into them; safe semantic evolution is the future CORE-20/30 work.
- Self-host is the only bundled example. Mutation tests use `tests/support/execution-plan.json`,
  a minimal synthetic graph; do not execute roadmap tasks to exercise application commands.

CI imports this example only into temporary storage and runs read-only CLI/MCP queries. It asserts
that the seed/export match, revision stays 0 and the operation log stays empty. Rust tests validate
contracts, milestone reachability, inherited gates and TUI rendering. No self-host task is executed
by these checks. Every task is read through real CLI/MCP show/explain calls, with exact equality of
steps, boundaries, acceptance and resolved context; the lifecycle tests use only synthetic data.
