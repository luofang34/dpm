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
not a backup of operation history. Edit that export and use `plan diff` / `plan apply --reason`
to maintain live state through reviewed commands; the file revision is an atomic precondition.
`history` reads committed operations.

## Backup, restore and verification

JSON export is not a backup: it omits the operation history. Copying `state.sqlite` while any
process has it open is not a backup either, because committed pages may still live in
`state.sqlite-wal`. Use the commands below. `backup` and `verify-store` without a path use the store selected by
`--database`, `--project` or discovery; `restore` and `verify-store PATH` take explicit files.
None of them appends operations or changes revisions.

```sh
dpm backup --to /backups/plan-2026-09-26.sqlite
dpm verify-store /backups/plan-2026-09-26.sqlite
dpm restore --from /backups/plan-2026-09-26.sqlite --to /path/to/new/state.sqlite
dpm verify-store            # the selected workspace's live store
```

- `backup` copies one consistent snapshot of the live store with SQLite's online backup API while
  other processes keep writing. The file holds the snapshot, the full operation history and the
  schema version in one self-contained file (no `-wal`), and is verified before the command succeeds.
- `restore` verifies the backup, copies it to a path that must not exist yet, and verifies the
  result. It never overwrites live state and never changes locators or device bindings: point a
  locator's `database` at the restored file, or run `dpm workspace register --replace --database
  NEW_PATH`, once you have checked the reported workspace identity and revision.
- `verify-store [PATH]` opens the file read-only and checks SQLite page integrity, the schema
  version and layout, that the snapshot loads and validates, that operation revisions form one
  consecutive chain, and that the snapshot revision equals the last operation's result. It reports
  the workspace identity, revision and operation count.

Any destination that already exists, including a leftover `-wal`, `-shm` or `-journal` side file,
fails with `target_exists`; damaged files fail with `corrupt_store`. These are device-local operator
commands, so they are CLI-only; MCP tools do not read or write arbitrary local paths.

## Store schema version

The store records its layout version in SQLite's `user_version` header. Every open checks it
before writing anything:

- a newer version than this binary supports fails with `unsupported_schema_version` and leaves the
  file untouched; use the release that wrote it, and take a backup with that release before
  upgrading or downgrading;
- version 1 is the current layout; a store created before versioning (header 0 with exactly the
  version 1 tables) opens normally, read-only commands leave it untouched, and its next write
  records version 1 in the same transaction;
- an SQLite file with other tables is refused rather than modified.

A future layout change will migrate a known older version in one transaction when the store is
opened for writing; an older version without a migration is refused with instructions.

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
Missing bindings return `workspace_not_bound` with the workspace UUID and the register command; an
MCP process started on such a locator exits before serving and prints the same code and message.
Changed store identities are rejected. Redirecting an existing binding requires
`workspace register --replace --database PATH`. Because a locator names the workspace and resource,
not a path, moving or renaming a checkout directory keeps its bindings and every stable ID.

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
All readiness and ranking queries still consider the full workspace graph; `next --resource-key KEY`
only narrows the returned list after that and reports eligible work outside it ([scoped next](mcp.md#scoped-next)).

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
