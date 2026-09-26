#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo fmt --all -- --check
python3 scripts/check_structure.py
python3 scripts/check_repository.py
if ! command -v cargo-deny >/dev/null 2>&1; then
    printf '%s\n' 'Install the required dependency checker: cargo install cargo-deny --locked --version 0.19.8' >&2
    exit 1
fi
python3 scripts/check_licenses.py
cargo deny --workspace --all-features --locked check licenses advisories
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
RUSTDOCFLAGS='-D missing_docs -D rustdoc::broken_intra_doc_links' cargo doc --workspace --no-deps --all-features --locked
cargo build --workspace --release --locked
python3 scripts/smoke_self_host.py
python3 scripts/smoke_mvp.py
python3 scripts/smoke_agent.py
python3 scripts/smoke_projects.py
python3 scripts/smoke_terminal.py
python3 scripts/smoke_plans.py
python3 scripts/smoke_tracking.py
python3 scripts/smoke_interchange.py
