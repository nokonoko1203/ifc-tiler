#!/usr/bin/env bash
# 静的検査と自動テスト（受け入れ条件A8）。
#   scripts/check.sh
set -euo pipefail
cd "$(dirname "$0")/.."
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --release
uv run -q --no-project --python 3.12 scripts/check-crate-age.py --min-days 3
