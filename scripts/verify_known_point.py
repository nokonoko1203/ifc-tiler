"""出力tilesetを読み戻し、既知点立方体の角が期待位置にあるかを pyproj で検算する。

    uv run --no-project --python 3.12 --with pyproj scripts/verify_known_point.py out/xxx/tileset.json \
        [--geoid jpgeo2024|gsigeo2011] [--tolerance-m 0.01]

期待値は testdata/handmade/expected.json（国土地理院の計算結果）。meshopt圧縮は読めないので、
ifc2tiles は --no-compress で変換しておく（圧縮した出力の既知点はRustの結合テストで確かめる）。
立方体（名前「既知点立方体」）の底面の頂点のうち、IX系で E+N が最小の角を局所原点とみなす。
"""

import argparse
import json
import struct
import sys
from pathlib import Path

from pyproj import Transformer

ROOT = Path(__file__).resolve().parent.parent
CUBE_NAME = "既知点立方体"


def read_glb(path: Path) -> tuple[dict, bytes]:
    data = path.read_bytes()
    jlen = struct.unpack_from("<I", data, 12)[0]
    js = json.loads(data[20 : 20 + jlen])
    bin_off = 20 + jlen
    blen = struct.unpack_from("<I", data, bin_off)[0]
    return js, data[bin_off + 8 : bin_off + 8 + blen]


def accessor(js: dict, binary: bytes, idx: int) -> list:
    """byteStride を考慮して読む（KHR_mesh_quantization の UNSIGNED_SHORT にも対応）。meshopt圧縮は未対応。"""
    a = js["accessors"][idx]
    v = js["bufferViews"][a["bufferView"]]
    if "EXT_meshopt_compression" in v.get("extensions", {}):
        raise SystemExit("meshopt-compressed GLB is not supported by this script")
    ncomp = {"SCALAR": 1, "VEC3": 3}[a["type"]]
    fmt, size = {5126: ("f", 4), 5125: ("I", 4), 5123: ("H", 2)}[a["componentType"]]
    stride = v.get("byteStride", ncomp * size)
    base = v["byteOffset"] + a.get("byteOffset", 0)
    out = [struct.unpack_from(f"<{ncomp}{fmt}", binary, base + i * stride) for i in range(a["count"])]
    return out if ncomp > 1 else [x[0] for x in out]


def strings(js: dict, binary: bytes, prop: dict, count: int) -> list[str]:
    vv = js["bufferViews"][prop["values"]]
    ov = js["bufferViews"][prop["stringOffsets"]]
    offs = struct.unpack_from(f"<{count + 1}I", binary, ov["byteOffset"])
    base = vv["byteOffset"]
    return [binary[base + offs[i] : base + offs[i + 1]].decode() for i in range(count)]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("tileset")
    ap.add_argument("--geoid", default="jpgeo2024", choices=["jpgeo2024", "gsigeo2011", "none"])
    ap.add_argument("--tolerance-m", type=float, default=0.01)
    args = ap.parse_args()
    exp = json.loads((ROOT / "testdata/handmade/expected.json").read_text())["known_point"]
    geoid = {"jpgeo2024": exp["geoid_height_jpgeo2024_m"], "gsigeo2011": exp["geoid_height_gsigeo2011_m"], "none": 0.0}[args.geoid]
    exp_h = exp["orthometric_height_m"] + geoid

    ts_path = Path(args.tileset)
    ts = json.loads(ts_path.read_text())
    m = ts["root"]["transform"]  # 列優先 ENU→ECEF
    pts = []
    def contents(tile):
        if "content" in tile:
            yield tile["content"]["uri"]
        for c in tile.get("children", []):
            yield from contents(c)
    for uri in contents(ts["root"]):
        js, binary = read_glb(ts_path.parent / uri)
        meta = js["extensions"]["EXT_structural_metadata"]["propertyTables"][0]
        names = strings(js, binary, meta["properties"]["name"], meta["count"])
        if CUBE_NAME not in names:
            continue
        fid_cube = names.index(CUBE_NAME)
        nm = js["nodes"][0].get("matrix", [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1])  # 量子化の復元行列（列優先）
        for prim in js["meshes"][0]["primitives"]:
            pos = accessor(js, binary, prim["attributes"]["POSITION"])
            fid = accessor(js, binary, prim["attributes"]["_FEATURE_ID_0"])
            for p, f in zip(pos, fid):
                if int(f) == fid_cube:
                    p = [nm[0] * p[0] + nm[4] * p[1] + nm[8] * p[2] + nm[12], nm[1] * p[0] + nm[5] * p[1] + nm[9] * p[2] + nm[13], nm[2] * p[0] + nm[6] * p[1] + nm[10] * p[2] + nm[14]]
                    e, n, u = p[0], -p[2], p[1]  # glTF Y-up → ENU
                    pts.append([m[0] * e + m[4] * n + m[8] * u + m[12], m[1] * e + m[5] * n + m[9] * u + m[13], m[2] * e + m[6] * n + m[10] * u + m[14]])
    if not pts:
        print("cube not found")
        return 1
    to_geo = Transformer.from_crs("EPSG:4978", "EPSG:4979", always_xy=True)
    to_grid = Transformer.from_crs("EPSG:4326", "EPSG:6677", always_xy=True)
    corners = []
    for x, y, z in pts:
        lon, lat, h = to_geo.transform(x, y, z)
        east, north = to_grid.transform(lon, lat)
        corners.append((east, north, h, lat, lon))
    # 底面（最低点から0.5 m以内）の頂点のうち、E+N が最小の角
    h_min = min(c[2] for c in corners)
    east, north, h, lat, lon = min((c for c in corners if c[2] < h_min + 0.5), key=lambda c: c[0] + c[1])
    d_e, d_n, d_h = east - exp["y_east_m"], north - exp["x_north_m"], h - exp_h
    ok = max(abs(d_e), abs(d_n), abs(d_h)) <= args.tolerance_m
    print(json.dumps({
        "tileset": str(ts_path), "geoid": args.geoid,
        "corner": {"east": round(east, 4), "north": round(north, 4), "ellipsoidal_h": round(h, 4), "lat": round(lat, 9), "lon": round(lon, 9)},
        "expected": {"east": exp["y_east_m"], "north": exp["x_north_m"], "ellipsoidal_h": round(exp_h, 4)},
        "diff_m": {"east": round(d_e, 4), "north": round(d_n, 4), "h": round(d_h, 4)},
        "ok": ok,
    }, ensure_ascii=False))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
