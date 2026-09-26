# Project discovery and local state

`dpm` opens the nearest configured project's TUI. `dpm --json` returns project status instead of
starting a terminal UI. CLI and `dpm-mcp` share the same discovery and opening implementation.

## Selection rules

1. `--database PATH` (alias `--db`) opens that SQLite file directly, without project discovery.
2. `--project DIR` opens exactly `DIR/.dpm/project.toml`. It never falls back to a parent project.
3. With neither option, inspect the current directory and its parents for `.dpm/project.toml`.
   The closest project wins. Inspect a Git root itself, then stop: do not cross a `.git` directory
   or worktree `.git` file into an unrelated outer project.

The two explicit options are mutually exclusive. A missing/invalid locator in the nearest `.dpm`
directory, unsupported version or missing source is an error, not permission to open a parent
project. No project means an actionable `project_not_found` error; no example is implicitly loaded
and no directory/database is created by a query. Discovery does not search child directories.

## Ordinary projects

In your project's directory, run:

```sh
dpm init "My project"
dpm status --json
dpm
```

Initialization writes:

```text
.dpm/
  project.toml    # shareable project locator
  .gitignore      # excludes runtime files
  state.sqlite    # local authoritative plan and operation log; do not commit
```

The versioned locator is TOML:

```toml
version = 1
database = "state.sqlite"
```

Source paths are relative to the locator directory, not the command's working directory. Absolute
paths are supported for local configuration but are not portable. The locator accepts exactly one
nonempty `database` or `preview` field, plus `version`; unknown fields are rejected.

`init`, `import PLAN.json` and `demo` are explicit initialization commands. They target the current
directory, or the exact `--project DIR`, and can create a nested independent project. They never
replace an existing database or preview. A cloned database locator with no local database can be
initialized explicitly with `import` or `init`. Existing malformed configuration must be fixed,
not silently replaced. If initialization is interrupted, inspect any incomplete `.dpm` directory;
DPM never deletes unknown state to retry automatically.

Only `project.toml` and `.gitignore` belong in Git. A locator does not synchronize the plan itself:
use `export` and `import` to transfer a snapshot; exported JSON does not include operation history.
SQLite remains authoritative for normal operation. A tracked configuration pointing at a missing
local database therefore requires explicit initialization after cloning.

## This repository's example

The committed [.dpm/project.toml](../.dpm/project.toml) contains:

```toml
version = 1
preview = "../examples/self-host/dpm-alpha.json"
```

Run `cargo run` in this repository, or run the built/installed `dpm` from any of its subdirectories.
The same discovery rules select the self-host example; no executable special-cases the repository
name, checkout path or remote. The console labels it `PREVIEW read-only`.

The plan is validated and loaded in memory. Queries/export work normally, and CLI/MCP mutations
return `read_only_project`, even if a task would otherwise be eligible. No SQLite file is created,
no open decision is resolved, and no task is claimed. To later operate on an explicitly authorized
copy, import it into a separate database/project; preview mode never becomes writable implicitly.

`dpm demo` remains an explicit way to initialize the sole bundled self-host plan elsewhere. It does
not replace this repository's preview or start its gated tasks. There are no other bundled demos.

## Agent and existing-database use

An MCP client can launch `dpm-mcp --actor agent:reader` with its working directory inside a project,
or specify `--project DIR` / `--database PATH`. Discovery, preview restrictions, engine commands and
queries are identical to CLI. `attach-git-head` / `attach_git_head` capture the selected project's
Git HEAD; an explicit bare database has no project root and uses the process's current repository.

Existing SQLite databases retain their serialization and operation history. Open any such file
with `dpm --database PATH status --json`, or point a database locator at it. Discovery recognizes
an unconfigured legacy workspace and asks for explicit selection; it never overwrites it or
creates a second empty project that hides its history. A code rename is not an operation-log migration.

## Multiple repositories and non-code work

A project directory need not be a Git repository. Run `init` in a planning/document directory to
keep a workspace there, then use `--project DIR` from any other checkout to query or operate on it.
For example, for an already initialized central workspace:

```sh
dpm --project /path/to/product-planning status --json
dpm --project /path/to/product-planning explain TASK-KEY --json
```

The graph can contain nested projects and tasks from multiple repositories, as well as procurement,
design and other non-code work. They share work IDs and dependencies in one workspace. The upward
search's Git boundary prevents accidental selection; it does not restrict graph membership.

Current Git evidence capture uses the selected project directory. If the central planning directory
is separate from the repository whose HEAD is needed, explicitly select the central database while
running from that repository:

```sh
dpm --database /path/to/product-planning/.dpm/state.sqlite attach-git-head TASK-KEY --actor agent:worker
```

This is a manual local workflow. There is no typed repository binding or automatic checkout selection
yet. Several processes on the same machine may open the same local workspace; copying its database
onto another device does not establish collaboration. Do not put live SQLite files on a shared
network/cloud folder as a sync mechanism.

The [workspace and collaboration design](workspaces-and-collaboration.md) defines the planned
separation between TOML/JSON interchange, durable SQLite state, device-local bindings and semantic
operation exchange. Today TOML is only the locator format; plan import/export still uses JSON.
