# DPM naming, project discovery and clean source history

## Purpose / observable outcome

Build and run the `dpm` CLI and `dpm-mcp` adapter. A bare `dpm` finds the nearest explicit project
from the current directory and opens its console. This repository opens its prepared self-host
plan through a committed locator, without creating a database or starting any task.

## Non-goals

No package-manager integration, remote publication, new scheduling/domain features or self-host
execution. No global project search, directory-name heuristics, implicit demo installation or
rewriting live operation logs. The physical checkout directory need not be renamed.

## Domain impact / interfaces

Rename package identifiers/imports to DPM while preserving serialized type shapes and stable IDs.
The root `dpm` package owns the CLI and embedded example; `dpm-sdk` is the library facade.
All internal dependencies specify their workspace-compatible version.

`dpm-app` owns project discovery and database/preview opening for both CLI and MCP. A versioned
`.dpm/project.toml` chooses exactly one source: `database` or `preview`, relative to that file.
CLI/MCP explicit database/project arguments override discovery. Nearest invalid configuration is
an error; discovery stops at a Git root. `init`/`import`/`demo` explicitly create a project in the
chosen directory or an explicitly supplied database. Preview mutations are rejected by the shared
application service, including agent calls.

## Milestones / acceptance

1. Back up Git/source/local state outside the repository; rename the existing MVP and package
   boundary, remove stale historical records and unreachable evidence references, verify all gates.
2. Create a fresh local root commit containing the clean DPM MVP; retain the external recovery
   archive. The user explicitly authorizes raw Git for this task and replacement of local history.
3. Implement project discovery, safe initialization and read-only preview access through the common
   application boundary. Commit the repository locator and ignore only runtime state.
4. Test ancestor/nested discovery, Git boundaries, invalid/unsupported locators, overrides, ordinary
   initialization, preview rejection, CLI/MCP parity and no database creation on reads. Run full CI,
   an actual terminal smoke, source package inventory and dependency audit. Commit the result.

## Progress

- [x] Inspect current state and create verified external recovery backups.
- [x] Rename the MVP and verify its clean root baseline (full CI passed: 67 Rust tests).
- [ ] Implement and test discovery/initialization/preview behavior.
- [ ] Update usage, verify final state and complete local history rewrite.

## Decision log

- User authorization overrides the GitButler skill for this task; no GitButler setup is required.
- Share project resolution between adapters. CLI/MCP do not duplicate store or execution rules.
- Prefer explicit project locators over special-casing this repository's name or current Git remote.
- Keep preview data read-only in memory. Normal projects use authoritative SQLite and semantic ops.
- Do not carry the retired demo into reachable rewritten history; keep recovery data outside Git.

## Discoveries

The initial source history has only one commit while the implemented MVP is uncommitted. Existing
local databases are revision 0 with zero operations; their data is backed up independently.
The facade/CLI package collision and external embedded seed must be fixed together during renaming.

## Outcome / remaining work

Implementation in progress. No remote repository or package publication is part of this work.
