#!/usr/bin/env bash
# testdata の全IFCを変換し、validatorにかけて、1行1ファイルのJSONで結果を出す（受け入れ条件A3）。
# ジオリファレンスが使えないファイル（終了コード2）は --origin（東京駅付近）で再試行する。
#   scripts/batch-convert.sh [ifc2tilesのパス] > out/batch.jsonl
set -uo pipefail
cd "$(dirname "$0")/.."
bin="${1:-target/release/ifc2tiles}"
outroot=out/batch
rm -rf "$outroot"; mkdir -p "$outroot"
find testdata/handmade testdata/external/spec testdata/external/cert testdata/external/ifclite -name '*.ifc' -print0 | sort -z |
while IFS= read -r -d '' f; do
  n=$(echo "$f" | sed 's#testdata/##; s#[/ ()]#_#g; s#\.ifc$##')
  out="$outroot/$n"
  mode=native
  "$bin" "$f" -o "$out" >"$out.log" 2>&1; code=$?
  if [[ $code == 2 ]]; then
    mode=origin
    "$bin" "$f" -o "$out" --origin 35.681236,139.767125,3 >>"$out.log" 2>&1; code=$?
  fi
  if [[ $code != 0 ]]; then
    python3 -c "import json,sys; print(json.dumps({'file': sys.argv[1], 'ok': False, 'exit': int(sys.argv[3]), 'error': open(sys.argv[2]).read()[-300:]}, ensure_ascii=False))" "$f" "$out.log" "$code"
    continue
  fi
  v=$(scripts/validate-tiles.sh "$out/tileset.json")
  errs=$(echo "$v" | grep -o '"numErrors": [0-9]*' | head -1 | grep -o '[0-9]*$')
  warns=$(echo "$v" | grep -o '"numWarnings": [0-9]*' | head -1 | grep -o '[0-9]*$')
  python3 - "$f" "$out" "$mode" "$errs" "$out.log" "$warns" <<'PY'
import json, sys, os
f, out, mode, errs, log, warns = sys.argv[1:]
r = json.load(open(os.path.join(out, "ifc2tiles-report.json")))
print(json.dumps({"file": f, "ok": True, "mode": mode, "validator_errors": int(errs), "validator_warnings": int(warns), "elements": r["elements"], "excluded": r["excluded"],
  "without_mesh": r["withoutMesh"], "columns": r["columns"], "tiles": len(r["tiles"]), "bytes": r["totalBytes"],
  "ms": r["timingMs"], "warnings": r["warnings"], "source": r["conversion"]["source"], "placement": r["conversion"]["placement"]["kind"]}, ensure_ascii=False))
PY
done
