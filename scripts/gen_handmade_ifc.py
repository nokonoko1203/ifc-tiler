"""検証用の自作IFCを testdata/handmade/ に生成する。

既知点（東京駅付近、JGD2011 平面直角座標系IX系）に1 m立方体を置き、
ジオリファレンスの経路ごとに別ファイルを作る。期待値は expected.json に書く。

    uv run scripts/gen_handmade_ifc.py
"""

from __future__ import annotations

import json
import uuid
from pathlib import Path

# 国土地理院 測量計算サイト（bl2xy、geoid calcgh / calcgh2011、JGD2011、IX系）で求めた既知点
KNOWN_LAT = 35.681236
KNOWN_LON = 139.767125
KNOWN_X_NORTH = -35363.2377  # 平面直角座標 X（北）[m]
KNOWN_Y_EAST = -5992.9196  # 平面直角座標 Y（東）[m]
KNOWN_GRID_CONV_DEG = 0.038616667  # 子午線収差 [deg]
KNOWN_GEOID_JPGEO2024 = 36.7614  # ジオイド高 [m]（calcgh。2026年時点の既定はジオイド2024）
KNOWN_GEOID_GSIGEO2011 = 36.6625  # ジオイド高 [m]（calcgh2011）
KNOWN_ORTHO_HEIGHT = 3.0  # 立方体底面の標高（TP）[m]
ZONE9_ORIGIN_LAT = 36.0
ZONE9_ORIGIN_LON = 139.0 + 50.0 / 60.0

OUT = Path(__file__).resolve().parent.parent / "testdata" / "handmade"

_B64 = "0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_$"


def ifc_guid(name: str) -> str:
    """名前から決定的な22文字のIFC GlobalIdを作る。"""
    n = uuid.uuid5(uuid.NAMESPACE_URL, "ifc2tiles:" + name).int
    chars = []
    for _ in range(22):
        chars.append(_B64[n & 0x3F])
        n >>= 6
    return "".join(reversed(chars))


def step_str(s: str) -> str:
    """STEP文字列リテラル。非ASCIIは \\X2\\...\\X0\\ でエスケープする。"""
    out = []
    buf = []

    def flush():
        if buf:
            hexs = "".join(f"{u:04X}" for ch in buf for u in _utf16(ch))
            out.append(f"\\X2\\{hexs}\\X0\\")
            buf.clear()

    for ch in s:
        if ord(ch) < 0x80:
            flush()
            out.append("''" if ch == "'" else ("\\\\" if ch == "\\" else ch))
        else:
            buf.append(ch)
    flush()
    return "'" + "".join(out) + "'"


def _utf16(ch: str) -> list[int]:
    b = ch.encode("utf-16-be")
    return [int.from_bytes(b[i : i + 2], "big") for i in range(0, len(b), 2)]


def real(v: float) -> str:
    s = repr(float(v))
    return s if ("." in s or "E" in s or "e" in s) else s + "."


class Step:
    def __init__(self) -> None:
        self.lines: list[str] = []

    def add(self, entity: str, *attrs: str) -> str:
        ref = f"#{len(self.lines) + 1}"
        self.lines.append(f"{ref}={entity}({','.join(attrs)});")
        return ref

    def dump(self, path: Path, schema: str, name: str) -> None:
        header = [
            "ISO-10303-21;",
            "HEADER;",
            "FILE_DESCRIPTION(('ViewDefinition [ReferenceView]'),'2;1');",
            f"FILE_NAME({step_str(name)},'2026-09-26T00:00:00',('ifc2tiles'),('ifc2tiles'),'gen_handmade_ifc.py','gen_handmade_ifc.py','');",
            f"FILE_SCHEMA(('{schema}'));",
            "ENDSEC;",
            "DATA;",
        ]
        path.write_text("\n".join(header + self.lines + ["ENDSEC;", "END-ISO-10303-21;", ""]), encoding="ascii")


def refs(*r: str) -> str:
    return "(" + ",".join(r) + ")"


class Model:
    """IFC2X3 / IFC4 の共通部分を組み立てる小さなビルダ。長さ単位はmm。"""

    def __init__(self, schema: str) -> None:
        self.schema = schema
        self.s = Step()
        s = self.s
        if schema == "IFC2X3":
            person = s.add("IFCPERSON", "$", "'ifc2tiles'", "$", "$", "$", "$", "$", "$")
            org = s.add("IFCORGANIZATION", "$", "'ifc2tiles'", "$", "$", "$")
            po = s.add("IFCPERSONANDORGANIZATION", person, org, "$")
            app = s.add("IFCAPPLICATION", org, "'0.1'", "'gen_handmade_ifc'", "'gen'")
            self.oh = s.add("IFCOWNERHISTORY", po, app, "$", ".ADDED.", "$", "$", "$", "1790380800")
        else:
            self.oh = "$"
        self.origin = s.add("IFCCARTESIANPOINT", "(0.,0.,0.)")
        self.z = s.add("IFCDIRECTION", "(0.,0.,1.)")
        self.x = s.add("IFCDIRECTION", "(1.,0.,0.)")
        self.wcs = s.add("IFCAXIS2PLACEMENT3D", self.origin, "$", "$")
        self.ctx = s.add("IFCGEOMETRICREPRESENTATIONCONTEXT", "$", "'Model'", "3", "1.E-05", self.wcs, "$")
        self.body = s.add("IFCGEOMETRICREPRESENTATIONSUBCONTEXT", "'Body'", "'Model'", "*", "*", "*", "*", self.ctx, "$", ".MODEL_VIEW.", "$")
        mm = s.add("IFCSIUNIT", "*", ".LENGTHUNIT.", ".MILLI.", ".METRE.")
        m2 = s.add("IFCSIUNIT", "*", ".AREAUNIT.", "$", ".SQUARE_METRE.")
        m3 = s.add("IFCSIUNIT", "*", ".VOLUMEUNIT.", "$", ".CUBIC_METRE.")
        rad = s.add("IFCSIUNIT", "*", ".PLANEANGLEUNIT.", "$", ".RADIAN.")
        self.units = s.add("IFCUNITASSIGNMENT", refs(mm, m2, m3, rad))

    def g(self, name: str) -> str:
        return step_str(ifc_guid(f"{self.schema}:{name}"))

    def placement(self, rel: str, x: float, y: float, z: float) -> str:
        p = self.s.add("IFCCARTESIANPOINT", f"({real(x)},{real(y)},{real(z)})")
        a = self.s.add("IFCAXIS2PLACEMENT3D", p, self.z, self.x)
        return self.s.add("IFCLOCALPLACEMENT", rel, a)

    def box_items(self, dx: float, dy: float, dz: float) -> str:
        """原点を底面の角に置いた直方体（押出）の表現アイテム。"""
        s = self.s
        c2 = s.add("IFCCARTESIANPOINT", f"({real(dx / 2)},{real(dy / 2)})")
        p2 = s.add("IFCAXIS2PLACEMENT2D", c2, "$")
        prof = s.add("IFCRECTANGLEPROFILEDEF", ".AREA.", "$", p2, real(dx), real(dy))
        return s.add("IFCEXTRUDEDAREASOLID", prof, self.wcs, self.z, real(dz))

    def shape(self, item: str, rep_type: str = "'SweptSolid'") -> str:
        rep = self.s.add("IFCSHAPEREPRESENTATION", self.body, "'Body'", rep_type, refs(item))
        return self.s.add("IFCPRODUCTDEFINITIONSHAPE", "$", "$", refs(rep))

    def project(self, name: str) -> str:
        return self.s.add("IFCPROJECT", self.g("project"), self.oh, step_str(name), "$", "$", "$", "$", refs(self.ctx), self.units)

    def site(self, place: str, lat: str = "$", lon: str = "$", elev: str = "$") -> str:
        return self.s.add("IFCSITE", self.g("site"), self.oh, step_str("敷地"), "$", "$", place, "$", "$", ".ELEMENT.", lat, lon, elev, "$", "$")

    def building(self, place: str) -> str:
        return self.s.add("IFCBUILDING", self.g("building"), self.oh, step_str("検証棟"), "$", "$", place, "$", "$", ".ELEMENT.", "$", "$", "$")

    def storey(self, key: str, name: str, place: str, elev_mm: float) -> str:
        return self.s.add("IFCBUILDINGSTOREY", self.g(key), self.oh, step_str(name), "$", "$", place, "$", "$", ".ELEMENT.", real(elev_mm))

    def proxy(self, key: str, name: str, place: str, shape: str) -> str:
        if self.schema == "IFC2X3":
            return self.s.add("IFCBUILDINGELEMENTPROXY", self.g(key), self.oh, step_str(name), "$", "$", place, shape, "$", "$")
        return self.s.add("IFCBUILDINGELEMENTPROXY", self.g(key), self.oh, step_str(name), "$", "$", place, shape, "$", ".NOTDEFINED.")

    def aggregate(self, key: str, parent: str, *children: str) -> str:
        return self.s.add("IFCRELAGGREGATES", self.g(key), self.oh, "$", "$", parent, refs(*children))

    def contain(self, key: str, structure: str, *elements: str) -> str:
        return self.s.add("IFCRELCONTAINEDINSPATIALSTRUCTURE", self.g(key), self.oh, "$", "$", refs(*elements), structure)

    def pset(self, key: str, name: str, props: list[tuple[str, str]]) -> str:
        ps = [self.s.add("IFCPROPERTYSINGLEVALUE", step_str(n), "$", v, "$") for n, v in props]
        return self.s.add("IFCPROPERTYSET", self.g(key), self.oh, step_str(name), "$", refs(*ps))

    def define_props(self, key: str, pset: str, *objs: str) -> str:
        return self.s.add("IFCRELDEFINESBYPROPERTIES", self.g(key), self.oh, "$", "$", refs(*objs), pset)


def dms(deg: float) -> str:
    """IfcCompoundPlaneAngleMeasure（度, 分, 秒, 百万分の一秒）。"""
    sign = -1 if deg < 0 else 1
    total = round(abs(deg) * 3600 * 1_000_000)
    d, rem = divmod(total, 3600 * 1_000_000)
    m, rem = divmod(rem, 60 * 1_000_000)
    s, us = divmod(rem, 1_000_000)
    return f"({sign * d},{sign * m},{sign * s},{sign * us})"


def build_ifc4_map_conversion() -> None:
    """IFC4: IfcMapConversionで既知点へ置く。建物要素・型・Pset・開口・インスタンスを含む。"""
    m = Model("IFC4")
    s = m.s
    # MapUnitを省略すると Eastings 等はプロジェクト単位（mm）と解釈されるため、METREを明示する
    metre = s.add("IFCSIUNIT", "*", ".LENGTHUNIT.", "$", ".METRE.")
    crs = s.add("IFCPROJECTEDCRS", "'EPSG:6677'", step_str("JGD2011 / 平面直角座標系IX系"), "'JGD2011'", "'TP'", "'TM'", "'IX'", metre)
    # IFC4.3本文の解釈: Scaleは単位変換（プロジェクトのmm→地図のm）なので0.001
    s.add("IFCMAPCONVERSION", m.ctx, crs, real(KNOWN_Y_EAST), real(KNOWN_X_NORTH), real(KNOWN_ORTHO_HEIGHT), "1.", "0.", "0.001")
    proj = m.project("ifc2tiles 検証モデル（IfcMapConversion）")
    site_pl = m.placement("$", 0, 0, 0)
    site = m.site(site_pl)
    bldg_pl = m.placement(site_pl, 0, 0, 0)
    bldg = m.building(bldg_pl)
    f1_pl = m.placement(bldg_pl, 0, 0, 0)
    f1 = m.storey("storey1", "1階", f1_pl, 0)
    f2_pl = m.placement(bldg_pl, 0, 0, 3000)
    f2 = m.storey("storey2", "2階", f2_pl, 3000)
    m.aggregate("agg-project", proj, site)
    m.aggregate("agg-site", site, bldg)
    m.aggregate("agg-building", bldg, f1, f2)

    # 既知点の1 m立方体（局所原点＝既知点、底面は標高3 m）
    cube = m.proxy("cube", "既知点立方体", m.placement(f1_pl, 0, 0, 0), m.shape(m.box_items(1000, 1000, 1000)))

    # 壁（型と部材の両方にPset_WallCommon。FireRatingは部材側で上書き）
    wall_pl = m.placement(f1_pl, 2000, 0, 0)
    wall = s.add("IFCWALL", m.g("wall"), m.oh, step_str("外壁A"), "$", "$", wall_pl, m.shape(m.box_items(6000, 200, 3000)), "'W-01'", ".STANDARD.")
    type_ps = m.pset("wall-type-pset", "Pset_WallCommon", [("IsExternal", "IFCBOOLEAN(.T.)"), ("FireRating", "IFCLABEL('TYPE-60')"), ("Reference", "IFCIDENTIFIER('WT-1')")])
    wtype = s.add("IFCWALLTYPE", m.g("wall-type"), m.oh, step_str("RC壁200"), "$", "$", refs(type_ps), "$", "$", "$", ".STANDARD.")
    s.add("IFCRELDEFINESBYTYPE", m.g("rel-wall-type"), m.oh, "$", "$", refs(wall), wtype)
    occ_ps = m.pset("wall-occ-pset", "Pset_WallCommon", [("FireRating", "IFCLABEL('OCC-120')"), ("LoadBearing", "IFCBOOLEAN(.F.)")])
    m.define_props("rel-wall-occ", occ_ps, wall)
    jp_ps = m.pset("wall-jp-pset", "構造情報", [("耐火性能", f"IFCLABEL({step_str('1時間耐火')})"), ("コンクリート強度", "IFCREAL(24.)")])
    m.define_props("rel-wall-jp", jp_ps, wall)
    q = s.add("IFCQUANTITYAREA", "'NetSideArea'", "$", "$", "16.", "$")
    qto = s.add("IFCELEMENTQUANTITY", m.g("wall-qto"), m.oh, "'Qto_WallBaseQuantities'", "$", "$", refs(q))
    m.define_props("rel-wall-qto", qto, wall)

    # 開口とドア
    open_pl = m.placement(wall_pl, 2000, -100, 0)
    opening = s.add("IFCOPENINGELEMENT", m.g("opening"), m.oh, "'Opening'", "$", "$", open_pl, m.shape(m.box_items(1000, 400, 2000)), "$", ".OPENING.")
    s.add("IFCRELVOIDSELEMENT", m.g("rel-void"), m.oh, "$", "$", wall, opening)
    door_pl = m.placement(open_pl, 0, 150, 0)
    door = s.add("IFCDOOR", m.g("door"), m.oh, step_str("片開き戸"), "$", "$", door_pl, m.shape(m.box_items(1000, 100, 2000)), "'D-01'", "2000.", "1000.", ".DOOR.", ".SINGLE_SWING_LEFT.", "$")
    s.add("IFCRELFILLSELEMENT", m.g("rel-fill"), m.oh, "$", "$", opening, door)

    # 2層を貫く柱（1階に所属し、2階からも参照）
    col = s.add("IFCCOLUMN", m.g("column"), m.oh, step_str("通し柱"), "$", "$", m.placement(f1_pl, 9000, 0, 0), m.shape(m.box_items(400, 400, 6000)), "'C-01'", ".COLUMN.")
    s.add("IFCRELREFERENCEDINSPATIALSTRUCTURE", m.g("rel-ref-column"), m.oh, "$", "$", refs(col), f2)

    # 同じ形状を2回配置する家具（IfcMappedItem）
    map_rep = s.add("IFCSHAPEREPRESENTATION", m.body, "'Body'", "'SweptSolid'", refs(m.box_items(600, 600, 700)))
    rmap = s.add("IFCREPRESENTATIONMAP", m.wcs, map_rep)
    furn = []
    for i, x in enumerate((2000, 3500)):
        op = s.add("IFCCARTESIANTRANSFORMATIONOPERATOR3D", "$", "$", m.origin, "1.", "$")
        item = s.add("IFCMAPPEDITEM", rmap, op)
        furn.append(s.add("IFCFURNITURE", m.g(f"desk{i}"), m.oh, step_str(f"机{i + 1}"), "$", "$", m.placement(f2_pl, x, 2000, 0), m.shape(item, "'MappedRepresentation'"), "$", ".TABLE."))

    # 室（IfcSpace）
    space = s.add("IFCSPACE", m.g("space"), m.oh, "'101'", "$", "$", m.placement(f1_pl, 2000, 200, 0), m.shape(m.box_items(6000, 4000, 2800)), step_str("事務室"), ".ELEMENT.", ".SPACE.", "$")
    m.aggregate("agg-storey1-space", f1, space)

    m.contain("contain-f1", f1, cube, wall, door, col)
    m.contain("contain-f2", f2, *furn)
    s.dump(OUT / "ifc4_map_conversion.ifc", "IFC4", "ifc4_map_conversion.ifc")


def build_ifc2x3_plateau_origin() -> None:
    """IFC2X3: PLATEAU BIM活用マニュアル（令和3年）の方式。

    プロジェクト原点＝平面直角座標系IX系の原点。その経緯度をIfcSiteに書き、
    形状は平面直角座標の値（大きな座標）をそのまま持つ。
    """
    m = Model("IFC2X3")
    proj = m.project("ifc2tiles 検証モデル（PLATEAU方式）")
    site_pl = m.placement("$", 0, 0, 0)
    site = m.site(site_pl, dms(ZONE9_ORIGIN_LAT), dms(ZONE9_ORIGIN_LON), "0.")
    bldg_pl = m.placement(site_pl, 0, 0, 0)
    bldg = m.building(bldg_pl)
    f1_pl = m.placement(bldg_pl, 0, 0, 0)
    f1 = m.storey("storey1", "1階", f1_pl, 0)
    m.aggregate("agg-project", proj, site)
    m.aggregate("agg-site", site, bldg)
    m.aggregate("agg-building", bldg, f1)
    # IFCのx=東(Y)、y=北(X)として既知点へ置く
    cube_pl = m.placement(f1_pl, KNOWN_Y_EAST * 1000, KNOWN_X_NORTH * 1000, KNOWN_ORTHO_HEIGHT * 1000)
    cube = m.proxy("cube", "既知点立方体", cube_pl, m.shape(m.box_items(1000, 1000, 1000)))
    m.contain("contain-f1", f1, cube)
    m.s.dump(OUT / "ifc2x3_plateau_origin.ifc", "IFC2X3", "ifc2x3_plateau_origin.ifc")


def build_ifc2x3_site_latlon() -> None:
    """IFC2X3: IfcSiteの経緯度＝既知点、形状は局所の小さな座標。"""
    m = Model("IFC2X3")
    proj = m.project("ifc2tiles 検証モデル（IfcSite経緯度）")
    site_pl = m.placement("$", 0, 0, 0)
    site = m.site(site_pl, dms(KNOWN_LAT), dms(KNOWN_LON), real(KNOWN_ORTHO_HEIGHT * 1000))
    bldg_pl = m.placement(site_pl, 0, 0, 0)
    bldg = m.building(bldg_pl)
    f1_pl = m.placement(bldg_pl, 0, 0, 0)
    f1 = m.storey("storey1", "1階", f1_pl, 0)
    m.aggregate("agg-project", proj, site)
    m.aggregate("agg-site", site, bldg)
    m.aggregate("agg-building", bldg, f1)
    cube = m.proxy("cube", "既知点立方体", m.placement(f1_pl, 0, 0, 0), m.shape(m.box_items(1000, 1000, 1000)))
    m.contain("contain-f1", f1, cube)
    m.s.dump(OUT / "ifc2x3_site_latlon.ifc", "IFC2X3", "ifc2x3_site_latlon.ifc")


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    build_ifc4_map_conversion()
    build_ifc2x3_plateau_origin()
    build_ifc2x3_site_latlon()
    expected = {
        "source": "国土地理院 測量計算サイト bl2xy、geoid calcgh（JPGEO2024）/ calcgh2011（GSIGEO2011）（2026-09-26取得）",
        "crs": "EPSG:6677 (JGD2011 / Japan Plane Rectangular CS IX)",
        "known_point": {
            "latitude": KNOWN_LAT,
            "longitude": KNOWN_LON,
            "x_north_m": KNOWN_X_NORTH,
            "y_east_m": KNOWN_Y_EAST,
            "grid_convergence_deg": KNOWN_GRID_CONV_DEG,
            "orthometric_height_m": KNOWN_ORTHO_HEIGHT,
            "geoid_height_jpgeo2024_m": KNOWN_GEOID_JPGEO2024,
            "geoid_height_gsigeo2011_m": KNOWN_GEOID_GSIGEO2011,
            "ellipsoidal_height_jpgeo2024_m": round(KNOWN_ORTHO_HEIGHT + KNOWN_GEOID_JPGEO2024, 4),
            "ellipsoidal_height_gsigeo2011_m": round(KNOWN_ORTHO_HEIGHT + KNOWN_GEOID_GSIGEO2011, 4),
        },
        "zone9_origin": {"latitude": ZONE9_ORIGIN_LAT, "longitude": ZONE9_ORIGIN_LON},
        "files": {
            "ifc4_map_conversion.ifc": "立方体の局所原点が既知点。IfcMapConversion(Scale=0.001)",
            "ifc2x3_plateau_origin.ifc": "IfcSite経緯度＝IX系原点。立方体は平面直角座標の値で配置",
            "ifc2x3_site_latlon.ifc": "IfcSite経緯度＝既知点。RefElevation=3000mm（プロジェクト単位と解釈）",
        },
        "ifc4_map_conversion_semantics": {
            "wall_Pset_WallCommon": {"IsExternal": True, "FireRating": "OCC-120", "Reference": "WT-1", "LoadBearing": False},
            "column_contained_in": "1階",
            "column_referenced_in": "2階",
            "desk_instances": 2,
            "opening_rendered": False,
        },
    }
    (OUT / "expected.json").write_text(json.dumps(expected, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
