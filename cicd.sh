#!/bin/zsh
set -euo pipefail

cd "$(dirname "$0")"

if [[ -z "${CARGO_TARGET_DIR:-}" ]]; then
  export CARGO_TARGET_DIR="$PWD/.vizier/tmp/cargo-target"
fi
mkdir -p "$CARGO_TARGET_DIR"

cargo fmt
cargo clippy --all --all-targets -- -D warnings
cargo test --all --all-targets
