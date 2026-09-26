# DPM publication readiness

DPM — DAG Project Manager targets the public repository `luofang34/dpm`. This document separates
local source readiness from registry/distribution availability. The self-host plan remains prepared,
gated and unstarted; implementing DPM does not complete its acceptance contracts automatically.

## Source layout

The [completed plan](exec/0001-dpm-projects.md) covers code naming, project discovery and the explicitly
authorized replacement of local source history. Root package `dpm` owns the executable and bundled
self-host seed; `dpm-sdk` is the library facade. Internal path dependencies carry version requirements.

## Before source publication

- Review the two local source commits and repeat full verification if source or dependencies change.
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

Local validation on macOS arm64 with the pinned Rust 1.98.1 toolchain passed:

- Full `./ci.sh`: format, repository/source guards, strict Clippy, 76 Rust tests, strict API docs,
  release build, 24 self-host contract checks and synthetic CLI/MCP lifecycle/discovery checks.
- Bare `cargo run` and the locally installed `dpm` opened the preview in actual terminals, including
  from a nested source directory. Gantt left/right navigation, inspector/help and terminal cleanup
  passed, with preview files and local state unchanged.
- `cargo install --path .` into a temporary prefix and source package inventory passed. Local SQLite
  state is excluded. Registry installation and downloadable release archives remain unverified.
- `cargo audit` reported zero vulnerabilities and no warnings for the checked lockfile.
- The local history has two clean source commits; the external recovery bundle/source archive and
  consistent SQLite backups preserve the pre-rewrite checkout. Self-host remains unstarted.

No hosted CI run, remote push, crates.io publication or package-manager integration is claimed.
