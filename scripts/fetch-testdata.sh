#!/usr/bin/env bash
# testdata/sources.tsv に列挙した外部IFCを、commit固定で testdata/external/ へ取得する。
# 取得済みのファイルは sha256 を testdata/sources.sha256 と照合する。
set -euo pipefail
cd "$(dirname "$0")/.."
out=testdata/external
mkdir -p "$out"
while IFS=$'\t' read -r group repo commit path; do
  dest="$out/$group/$path"
  if [[ ! -f "$dest" ]]; then
    mkdir -p "$(dirname "$dest")"
    url="https://raw.githubusercontent.com/$repo/$commit/$(jq -rn --arg p "$path" '$p|@uri' | sed 's#%2F#/#g')"
    curl -fsSL --retry 3 -o "$dest" "$url"
  fi
done < testdata/sources.tsv
if [[ -f testdata/sources.sha256 ]]; then
  (cd "$out" && shasum -a 256 -c --quiet ../sources.sha256)
else
  (cd "$out" && find . -type f -name '*.ifc' -o -type f -name '*.IFC' | sort | xargs -I{} shasum -a 256 {}) > testdata/sources.sha256
  echo "wrote testdata/sources.sha256"
fi
