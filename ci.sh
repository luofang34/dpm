#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")"
cargo fmt --all -- --check
python3 scripts/check_structure.py
python3 scripts/check_repository.py
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-targets --all-features --locked
RUSTDOCFLAGS='-D missing_docs -D rustdoc::broken_intra_doc_links' cargo doc --workspace --no-deps --all-features --locked
cargo build --workspace --release --locked
python3 scripts/smoke_self_host.py
python3 scripts/smoke_mvp.py
python3 scripts/smoke_agent.py
python3 scripts/smoke_projects.py
