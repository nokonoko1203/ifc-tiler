#!/usr/bin/env bash
# 自作IFCを変換し、validator と既知点の検算をまとめて行う（受け入れ条件A1・A2）。
#   scripts/acceptance-handmade.sh [ifc2tilesのパス]
# 既知点はpyprojで独立に検算するため --no-compress で変換する。圧縮した出力もvalidatorにかける。
set -uo pipefail
cd "$(dirname "$0")/.."
bin="${1:-target/release/ifc2tiles}"
fail=0
run() { # name input expect(ok|ng) [args...]
  local name=$1 input=$2 expect=$3; shift 3
  local out="out/acceptance/$name"
  rm -rf "$out" "$out.compressed"
  if ! "$bin" "testdata/handmade/$input" -o "$out" --no-compress "$@" >/dev/null 2>"$out.log" \
    || ! "$bin" "testdata/handmade/$input" -o "$out.compressed" "$@" >/dev/null 2>>"$out.log"; then
    echo "FAIL $name: conversion error"; tail -3 "$out.log"; fail=1; return
  fi
  local errs e2
  errs=$(scripts/validate-tiles.sh "$out/tileset.json" | grep -o '"numErrors": [0-9]*' | grep -o '[0-9]*$')
  e2=$(scripts/validate-tiles.sh "$out.compressed/tileset.json" | grep -o '"numErrors": [0-9]*' | grep -o '[0-9]*$')
  errs=$((errs + e2))
  local res
  res=$(uv run -q --no-project --python 3.12 --with pyproj scripts/verify_known_point.py "$out/tileset.json")
  local ok; ok=$(echo "$res" | python3 -c 'import json,sys; print(json.loads(sys.stdin.read())["ok"])')
  local diff; diff=$(echo "$res" | python3 -c 'import json,sys; d=json.loads(sys.stdin.read())["diff_m"]; print(d)')
  local warns; warns=$(python3 -c "import json; print(len(json.load(open('$out/ifc2tiles-report.json'))['conversion']['warnings']))")
  local got=ng; [[ "$ok" == True ]] && got=ok
  local status=PASS; { [[ "$errs" != 0 ]] || [[ "$got" != "$expect" ]]; } && { status=FAIL; fail=1; }
  echo "$status $name validator_errors=$errs known_point=$got(expected $expect) diff_m=$diff warnings=$warns"
}
mkdir -p out/acceptance
run mapconv ifc4_map_conversion.ifc ok
run mapconv_gsigeo2011_mismatch ifc4_map_conversion.ifc ng --geoid gsigeo2011
run site_latlon ifc2x3_site_latlon.ifc ok
run plateau_grid ifc2x3_plateau_origin.ifc ok --site-coords grid --crs EPSG:6677
run plateau_enu_wrong ifc2x3_plateau_origin.ifc ng
exit $fail
