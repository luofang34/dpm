# Project discovery and local state

`dpm` opens the nearest configured project's TUI; `dpm --json` returns status. CLI and MCP use
one application service for discovery, commands, queries and device bindings.

## Selection

- `--database PATH` (`--db`) opens an existing SQLite store directly.
- `--project DIR` selects exactly `DIR/.dpm/project.toml`; this option takes a directory, not a project key.
- Otherwise search upward for the nearest locator, stopping at a Git root or worktree boundary.

Explicit database and project options are mutually exclusive. A malformed nearer locator never
falls back to a parent. Queries never initialize missing stores or silently open a sample.

## Local projects

Run `dpm init "My project"` in a code or non-code directory. It creates a local SQLite store,
ignore rules and a locator containing the generated workspace UUID:

```toml
version = 2
workspace = "00000000-0000-4000-8000-000000000001"
database = "state.sqlite"
```

The UUID must match the actual store. Paths are relative to `.dpm/`; absolute paths and unknown
fields are rejected. Only the locator and ignore rules belong in Git. Plan JSON uses format 2;
unsupported formats fail explicitly. A portable plan contains no device bindings or derived schedule.

`import PLAN.json` initializes an absent store from a validated plan; it never replaces live state.
An existing locator requires the imported workspace identity to match. `export` produces a snapshot,
not a backup of operation history. Keep consistent database backups for recovery.

## One workspace, several entry points

Register an existing store on each device:

```sh
dpm workspace register --database /path/to/planning/.dpm/state.sqlite
dpm workspace list --json
```

The returned workspace UUID can be used in several repository or document-directory locators:

```toml
version = 2
workspace = "00000000-0000-4000-8000-000000000001"
resource = "SOURCE-REPO"
```

Omit `resource` for an entry point that does not represent a specific resource. With neither
`database` nor `preview`, the locator resolves the workspace through the device registry.
Missing bindings return `workspace_not_bound`; changed store identities are rejected. Redirecting
an existing binding requires `workspace register --replace --database PATH`.

Bindings live in `DPM_CONFIG_DIR`, otherwise `XDG_CONFIG_HOME/dpm`, otherwise `HOME/.config/dpm`.
These directories must be absolute. `workspace_list` and `workspace_register` expose identical data
through MCP; device configuration does not append project operations or change project revisions.

Several local processes may open the same store. A checkout/worktree does not fork task ownership.
An independent project needs a new workspace identity; a Git fork alone does not establish that choice.
Copying SQLite to another device is not collaboration; do not use live database file syncing.

## Resources and Git evidence

A plan's `resources` map gives repositories, folders, document collections and other resources stable
IDs. Tasks may name zero, one or several read/write requirements. Keys, labels and remotes do not
replace resource identity. These requirements describe work scope, not filesystem permissions.
All readiness and ranking queries still consider the full workspace graph.

`attach-git-head KEY` uses the selected locator's repository and resource. When selecting a bare
database, run in the desired checkout and supply `--resource RESOURCE-KEY`. MCP `attach_git_head`
accepts the same `resource` argument. The resource must be a repository named by the task; a resource
conflicting with the locator is rejected. Evidence records both resource ID and commit SHA.

## This repository

The committed [.dpm/project.toml](../.dpm/project.toml) points at the prepared self-host plan with
`preview = "../examples/self-host/dpm-alpha.json"`. `cargo run` opens its read-only TUI using normal
discovery. Task updates belong in that plan's contracts; queries and export do not create SQLite state.
Execution commands return `read_only_project`. `demo` explicitly initializes this sole example elsewhere;
it never starts its gated tasks. Future semantic plan editing and sync are tracked by `CORE-20` and
`SYNC-10`, available through `explain`.
