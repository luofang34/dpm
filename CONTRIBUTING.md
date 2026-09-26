# Contributing to DPM

DPM is published under `AGPL-3.0-only`. Contributions are welcome under the terms below.

## Contributor License Agreement

Every contributor must accept the [DPM Contributor License Agreement](CLA.md) before a
contribution can be merged. You keep your copyright. The agreement grants the maintainer the right
to also distribute DPM under other terms, which is what allows signed builds to be sold through the
Apple App Store and similar stores. The maintainer in turn commits to keeping every contribution
available under `AGPL-3.0-only`.

A Developer Certificate of Origin sign-off alone is not sufficient: it certifies provenance but
grants no relicensing right. When a coding agent produces a contribution, the human or organization
submitting it accepts the agreement and is responsible for its representations.

## Dependencies

Third-party code that ships in DPM binaries must use a permissive license (for example MIT,
Apache-2.0, BSD or ISC). Copyleft third-party code cannot be relicensed by the maintainer and would
block application-store builds. Tools that run as separate processes are evaluated separately.

## Making a change

Read [AGENTS.md](AGENTS.md) for the architectural invariants and engineering rules, and
[PLANS.md](PLANS.md) for when an ExecPlan is required. Keep one issue per change and include the
regression test or guard that protects it. Run the full local pipeline before submitting:

```sh
./ci.sh
```

State in the pull request which checks you ran and their result. If your environment cannot run a
check, say so instead of claiming it passed.
