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
- `verify-store [PATH]` reads the store without writing to it or creating any file next to it, so it
  also works in a read-only directory. When no `-wal`, `-shm` or `-journal` file exists it opens the
  file as immutable; otherwise another process is using it or it holds committed pages in its WAL,
  and it reads through the side files that are already there.

All three report canonical absolute paths. Any destination that already exists, including a
leftover `-wal`, `-shm` or `-journal` side file, fails with `target_exists`; a destination whose
name itself ends in `-wal`, `-shm` or `-journal` fails with `invalid_request`, because SQLite would
treat it as another database's side file and later delete it. Damaged files fail with
`corrupt_store`, naming the file and the damaged record. These are device-local operator commands,
so they are CLI-only; MCP tools do not read or write arbitrary local paths.

### What verification guarantees

`verify-store`, and `restore` before and after copying, check:

- SQLite page integrity (`PRAGMA integrity_check`);
- the schema version and the exact set of schema objects that version names: every table, index,
  trigger and view is compared with its normalized SQL, so an extra trigger, view, index or table,
  a missing one or an altered definition is `corrupt_store`. Statistics tables created by a manual
  `ANALYZE` are also refused. Every open checks the same layout, and every write re-checks it inside
  its transaction, so a planted trigger never runs inside a DPM write;
- that the snapshot and every operation decode, with the damaged file, record (`snapshot` or
  `operation sequence N`) and column in the error;
- that the snapshot validates and its revision column matches its JSON;
- that local sequences have no gaps, every operation's resulting revision is its base revision + 1,
  consecutive operations chain, the first operation starts at the recorded history origin (or, with
  no operations, the snapshot is still at the origin), and the last operation produced the
  snapshot revision.

They do not detect an edit that keeps every record valid and consistent, such as a changed work
title inside the snapshot or a rewritten command, nor a deliberate rewrite of the origin, sequences
and revisions together: records are not signed. Verification detects accidental damage and
inconsistent tampering, not an adversary with write access to the file.

## Store schema version

The store records its layout version in SQLite's `user_version` header. Every open checks it, and
the exact layout it names, before writing anything:

- a newer version than this binary supports fails with `unsupported_schema_version` and leaves the
  file untouched; use the release that wrote it, and take a backup with that release before
  upgrading or downgrading;
- version 2 is the current layout: the snapshot, the operation log and `history_origin`, the
  revision the history starts from, written when the store is initialized;
- version 1, and header 0 (stores created before versioning), name the same layout without
  `history_origin`. Such a store opens normally and read-only commands leave it byte-for-byte
  untouched. Its next write first checks that its history is contiguous and ends at the snapshot,
  then records the first operation's base revision (or the snapshot revision, without operations)
  as the origin and stamps version 2, in the same transaction as the write. Operations lost from
  the start of such a history before that first write cannot be detected; `verify-store` reports
  `origin_revision: null` until then;
- an SQLite file with any other layout is refused rather than modified, and a refused `import` or
  `init` onto an existing store writes nothing.

A future layout change raises the version and adds an upgrade that runs inside the first write
transaction; an older version without an upgrade is refused with instructions.

Compatibility policy: the version check protects only binaries that read the header. Binaries that
know version 1 refuse version 2 with `unsupported_schema_version`. Binaries from before versioning
never read the header and ignore tables they do not query; they can still append operations to a
version 2 store, which keeps its recorded origin valid because they only append. A future layout
change whose data older binaries would misread must therefore also change the layout in a way every
older binary fails on loudly, for example by renaming a table those binaries query, rather than
relying on the version number alone.

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
A bound path that no longer exists returns `workspace_store_missing`, and a bound store that now
contains another workspace returns `workspace_identity_mismatch`; both name the binding and the
remedy instead of a raw storage error. One file holds one store, so registering a store at a path
already bound to another workspace is refused with `workspace_path_bound`. Redirecting an existing
binding, or rebinding such a path (which removes the stale identity), requires
`workspace register --replace --database PATH`. Because a locator names the workspace and resource,
not a path, moving or renaming a checkout directory keeps its bindings and every stable ID.

Bindings live in `DPM_CONFIG_DIR`, otherwise `XDG_CONFIG_HOME/dpm`, otherwise `HOME/.config/dpm`.
These directories must be absolute. `workspace list` reports, for each binding, `workspace`,
`database`, `store.status` (`ok`, `missing`, `identity_mismatch` with the `found` identity, or
`unreadable` with the `code` and `message` opening it returns) and `shared_with`, the other
identities bound to the same path. `workspace_list` and `workspace_register` expose identical data
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
