# Workspace and portable-plan design

## Purpose / observable outcome

Document a practical path from one local preview to multi-repository, non-code and collaborative
projects without coupling the execution graph to Git or treating local operations as cache data.
Verify ignored build/runtime/OS files while preserving tracked configuration and portable plans.

## Non-goals

No new sync service, plan-format parser, locator version, resource registry, schema migration,
self-host execution or remote publication. The existing JSON preview stays unchanged.

## Domain impact / interfaces

No Rust API or serialized state changes. Document future workspace/resource/local-binding roles,
versioned TOML/JSON interchange, command-based imports and semantic synchronization in
`docs/workspaces-and-collaboration.md`. Keep current selection rules in `docs/projects.md` explicit.
Extend the existing publication guard to exercise Git's ignore matching and reject OS metadata.

## Milestones / acceptance

1. Inspect the moved checkout and verify actual tracked/ignored paths.
2. Record authority, cross-repository location, portable format and collaboration decisions, with
   an honest current/future boundary and acceptance scenarios.
3. Cover nested build output and OS metadata; check runtime files stay ignored while locator,
   lockfile and plan documents remain trackable.
4. Run the full local pipeline before committing, leaving the prepared roadmap untouched.

## Progress

- [x] Inspect `~/dpm`: clean starting state; no tracked ignored files.
- [x] Specify workspace/store/format/sync boundaries and update usage guidance.
- [x] Extend ignore rules and add behavioral publication probes.
- [x] Validate the guard against broken rules and run full local CI.
- [x] Review and commit the design/hygiene change.

## Decision log

- Git discovery boundaries prevent accidental selection; explicit bindings bridge repositories.
- One workspace is one consistency namespace. Multiple repo checkouts are entry points to it.
- SQLite keeps durable local state; text exports do not automatically become runtime authority.
- TOML is the intended human exchange encoding; JSON remains the corresponding machine encoding.
- Begin collaboration with a per-workspace sequencer and explicit conflicts, not raw-file merging.
- Continue the user's authorized raw-Git workflow for this repository preparation; do not rewrite
  the established source commits again or publish them.

## Discoveries

The model already separates projects/work/artifacts from repositories. The current Git artifact
adapter uses the selected locator root, or the working directory for explicit database selection;
it has no typed multi-repository resource registry. TOML currently parses locators only. The SQLite
log is local; no outbox, checkpoint exchange, schema-versioned replay or remote authorization exists.

Root build output, OS metadata and local data were ignored. Nested `target` directories needed a
broader rule; tracked files also require publication checks because ignore rules do not untrack them.

## Outcome / remaining work

The design records workspace/resource identity, portable plan encoding, durable local state,
import semantics and a conservative first collaboration protocol, separately from current behavior.
Root/nested build output and OS metadata are excluded while shared locators and plan documents stay
trackable. The publication guard also detects already tracked OS metadata.

The full local pipeline passed with 76 Rust tests, strict Clippy/docs, release build and CLI/MCP
smoke checks. In an isolated temporary repository, deliberately narrowing the target rule, removing
the DS_Store rule, hiding the locator and force-tracking OS metadata each caused the guard to fail.
Source/data APIs and the prepared self-host plan are unchanged. Subsequent implementation milestones
remain outside this change; no remote publication or collaboration service was performed.
