"""Cargo.lock の crates.io 由来のパッケージが、最小公開期間を満たすか検査する。

    uv run --no-project --python 3.12 scripts/check-crate-age.py [--min-days 3]

条件を満たさない版があれば一覧を出して終了コード1で終わる。
crates.io のAPI利用規約に合わせ、1秒に1リクエスト程度に抑える。
"""

import argparse
import datetime as dt
import json
import sys
import time
import tomllib
import urllib.request
from pathlib import Path

REGISTRY = "registry+https://github.com/rust-lang/crates.io-index"
UA = "ifc2tiles-dev (scripts/check-crate-age.py)"


def published_at(name: str, version: str) -> dt.datetime:
    req = urllib.request.Request(f"https://crates.io/api/v1/crates/{name}/{version}", headers={"User-Agent": UA})
    with urllib.request.urlopen(req, timeout=30) as r:
        v = json.load(r)["version"]
    return dt.datetime.fromisoformat(v["created_at"].replace("Z", "+00:00"))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--min-days", type=float, default=3.0)
    ap.add_argument("--lock", default=str(Path(__file__).resolve().parent.parent / "Cargo.lock"))
    args = ap.parse_args()
    lock = tomllib.loads(Path(args.lock).read_text())
    pkgs = sorted({(p["name"], p["version"]) for p in lock["package"] if p.get("source") == REGISTRY})
    now = dt.datetime.now(dt.timezone.utc)
    young = []
    for name, version in pkgs:
        age = (now - published_at(name, version)).total_seconds() / 86400
        if age < args.min_days:
            young.append((name, version, age))
        time.sleep(1.0)
    print(f"checked {len(pkgs)} crates (min {args.min_days} days)")
    for name, version, age in young:
        print(f"  TOO NEW: {name} {version} ({age:.2f} days)")
    return 1 if young else 0


if __name__ == "__main__":
    sys.exit(main())
