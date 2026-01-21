#!/bin/zsh
set -euo pipefail

cd "$(dirname "$0")"
cargo fmt
cargo clippy --all --all-targets -- -D warnings
cargo test --all --all-targets
