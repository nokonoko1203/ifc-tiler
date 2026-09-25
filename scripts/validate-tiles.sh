#!/usr/bin/env bash
# 3d-tiles-validator（tools/validator で固定版）で tileset を検証する。
#   scripts/validate-tiles.sh out/tileset.json [report.json]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
v="$root/tools/validator"
[[ -d "$v/node_modules" ]] || (cd "$v" && bun install --frozen-lockfile)
args=(--tilesetFile "$1")
[[ $# -ge 2 ]] && args+=(--reportFile "$2")
node "$v/node_modules/3d-tiles-validator/build/main.js" "${args[@]}"
