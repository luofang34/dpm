# DPM publication readiness

DPM — DAG Project Manager targets the public repository `luofang34/dpm`. This document separates
local source readiness from registry/distribution availability. The self-host plan remains prepared,
gated and unstarted; implementing DPM does not complete its acceptance contracts automatically.

## Current work

The [active plan](exec/0001-dpm-projects.md) covers code naming, project discovery and the explicitly
authorized replacement of local source history. Root package `dpm` owns the executable and bundled
self-host seed; `dpm-sdk` is the library facade. Internal path dependencies carry version requirements.

## Before source publication

- Complete the active plan and full local verification, including CLI/MCP project-discovery parity.
- Keep the reviewed source history free of retired samples and local data. Recovery archives stay
  outside the repository. Preserve local SQLite snapshots and operation history during the rename.
- Create the public `luofang34/dpm` repository and push reviewed commits only when requested.
  Confirm actual hosted CI results; local verification is not hosted Linux CI evidence.

## Before a downloadable alpha or registry release

- Verify the chosen release tag, supported targets, extracted archives/binaries and checksums.
- Publish internal crates in dependency order before registry-installing `dpm`/`dpm-mcp`; package
  inventory and local installation alone do not prove a crates.io installation works.
- The pinned toolchain and declared minimum are both Rust 1.98.1. CI currently defines Linux only;
  do not advertise a lower MSRV or broader platform matrix without executing those checks.
- Re-run dependency advisory checks on the release lockfile. Keep AGPL-3.0-only licensing.

TUI is a read-only snapshot, reopened to see external changes. Scheduling supports FS/SS/FF/SF+lag;
execution conservatively waits for verified predecessors. Actor names are trusted local identities.
Server/sync/auth, plan editing/history/revert, calendars/resource leveling and native/web UI remain
separate milestones. They are not prerequisites for sharing the current MVP source.

## Package managers

No Homebrew/APT integration or registry publication is performed here. Short install commands are
possible only if the relevant repository accepts the package as `dpm`; naming a GitHub repository
does not reserve that name. Ubuntu has a historical
[Disk Pool Manager package](https://launchpad.net/ubuntu/bionic/+package/dpm).
See [Homebrew naming](https://docs.brew.sh/Taps) and
[Debian executable-name policy](https://www.debian.org/doc/debian-policy/ch-files.html#binaries).

## Verification evidence

Pending completion of the active plan. Do not treat older test results as validation of the rename
or project-discovery changes.
