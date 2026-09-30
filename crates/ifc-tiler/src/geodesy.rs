//! 局所座標から地心直交座標（ECEF）への変換と、タイルを置く根のENUフレーム。
//!
//! 地図座標・経緯度からECEFへの変換はPROJに任せる（EPSGコードで表せるCRSなら何でもよい）。
//! 地図経路では頂点ごとに変換する。建物の範囲でも接平面（ENU）で近似すると、
//! 子午線収差で100 mあたり数cmずれるため。

use geocentric::{geocentric_to_geodetic, geodetic_to_geocentric};

use crate::georef::Placement;
use crate::proj::{Context, Kind, Object};

/// WGS84（`EPSG:4978`の楕円体）。
const A: f64 = 6_378_137.0;
const INV_F: f64 = 298.257_223_563;

fn e_sq() -> f64 {
    let f = 1.0 / INV_F;
    f * (2.0 - f)
}

/// 鉛直のCRSがないときに補う標高の基準（EGM2008の標高）。
const DEFAULT_VERTICAL_CRS: &str = "EPSG:3855";

/// 変換、その警告、基準点のECEF（変換できなければ`None`）。
type Attempt = (Projector, Vec<String>, Option<[f64; 3]>);

/// 局所座標 [m] → ECEF [m]。
pub struct Projector {
    /// 入力CRS → `EPSG:4978`。軸順は（東, 北）または（経度, 緯度）にそろえてある。
    pj: Object,
    placement: Placement,
    /// ENU経路の原点のECEFと基底。
    enu: Option<Frame>,
}

impl Projector {
    /// 返り値の2つ目は警告。
    ///
    /// point-tilerと同じく、グリッド（ジオイドなど）をcdn.proj.orgから取得してキャッシュする。
    /// 取得できないと変換そのものが失敗するため、そのときはネットワークを切って作り直し、
    /// PROJにグリッドを使わない近似の変換を選ばせる（近似になったことは警告する）。
    pub fn new(placement: &Placement) -> Result<(Self, Vec<String>), String> {
        let reference = match placement {
            Placement::Grid(g) => g.to_map([0.0; 3]),
            Placement::Enu(e) => [e.longitude_deg, e.latitude_deg, e.orthometric_height],
        };
        let (mut p, mut warnings, origin) = match Self::with_network(placement, true, reference)? {
            (p, w, Some(origin)) => (p, w, origin),
            (_, _, None) => match Self::with_network(placement, false, reference)? {
                (p, w, Some(origin)) => (p, w, origin),
                (_, _, None) => {
                    return Err(format!(
                        "座標 {reference:?} を変換できない（CRSの定義域外）。--crs やジオリファレンスを確かめる"
                    ));
                }
            },
        };
        if let Some(name) = p.last_ballpark() {
            warnings.push(format!(
                "座標変換（{name}）がグリッドを使わない近似になり、高さや位置が数m以上ずれている可能性がある。\
                 ネットワークに接続するか、projsyncで必要なグリッドを取得する"
            ));
        }
        if let Placement::Enu(e) = placement {
            p.enu = Some(Frame { origin, ..Frame::at_geodetic(e.latitude_deg, e.longitude_deg, 0.0) });
        }
        Ok((p, warnings))
    }

    /// 変換を作り、基準点を変換してみる。基準点を変換できなければ3つ目が`None`。
    fn with_network(placement: &Placement, network: bool, reference: [f64; 3]) -> Result<Attempt, String> {
        let mut warnings = Vec::new();
        let ctx = Context::new().ok_or("PROJのコンテキストを作れない")?;
        ctx.set_network(network);
        ctx.enable_grid_cache();
        let pj = source_to_ecef(&ctx, placement, &mut warnings)?;
        let p = Self { pj, placement: placement.clone(), enu: None };
        let origin = p.trans(reference);
        Ok((p, warnings, origin))
    }

    fn trans(&self, p: [f64; 3]) -> Option<[f64; 3]> {
        self.pj.trans(p)
    }

    /// 直前の変換がグリッドを使わない近似（ballpark）だったら、その名前。
    fn last_ballpark(&self) -> Option<String> {
        // 候補が1つだけの変換では直前の操作がなく、変換自身を見る
        let last = self.pj.last_used_operation();
        let op = last.as_ref().unwrap_or(&self.pj);
        op.has_ballpark_transformation().then(|| op.name())
    }

    /// 局所座標 [m] → ECEF [m]。地図座標を変換できなければ、その地図座標を返す。
    pub fn to_ecef(&self, p: [f64; 3]) -> Result<[f64; 3], [f64; 3]> {
        match (&self.placement, &self.enu) {
            (Placement::Enu(e), Some(frame)) => Ok(frame.to_ecef(e.to_enu(p))),
            (Placement::Grid(g), _) => {
                let map = g.to_map(p);
                self.trans(map).ok_or(map)
            }
            (Placement::Enu(_), None) => unreachable!("ENUのフレームはnewで作る"),
        }
    }

    /// 点`c`の近くで、局所座標の向きを`frame`の座標の向きへ移す回転（列が局所x・y・z軸の行き先）。
    pub fn rotation_at(&self, c: [f64; 3], frame: &Frame) -> Result<[[f64; 3]; 3], [f64; 3]> {
        let o = frame.to_local(self.to_ecef(c)?);
        let axis = |i: usize| {
            let mut q = c;
            q[i] += 1.0;
            self.to_ecef(q).map(|e| sub(frame.to_local(e), o))
        };
        Ok(orthonormalize([axis(0)?, axis(1)?, axis(2)?]))
    }
}

/// 入力CRS→`EPSG:4978`の変換を作る。鉛直のCRSがなければEGM2008の標高とみなす。
fn source_to_ecef(ctx: &Context, placement: &Placement, warnings: &mut Vec<String>) -> Result<Object, String> {
    let (crs, want_projected) = match placement {
        Placement::Grid(g) => (g.crs.as_str(), true),
        Placement::Enu(e) => (e.crs.as_str(), false),
    };
    let mut src = ctx.create(crs).ok_or_else(|| format!("CRSを解釈できない（{crs}）。--crs EPSG:xxxx で指定する"))?;
    let kind = src.kind();
    let horizontal = if kind == Kind::Compound { src.sub_crs(0).map_or(Kind::Other, |h| h.kind()) } else { kind };
    let geographic = matches!(horizontal, Kind::Geographic2d | Kind::Geographic3d);
    let ok = if want_projected { horizontal == Kind::Projected } else { geographic };
    if !ok {
        let what = if want_projected {
            "地図座標のCRSには投影座標系"
        } else {
            "緯度・経度のCRSには地理座標系"
        };
        return Err(format!(
            "{crs}は使えない。{what}（例: {}）を指定する",
            if want_projected { "EPSG:32654" } else { "EPSG:4326" }
        ));
    }
    if matches!(kind, Kind::Projected | Kind::Geographic2d) {
        let compound = ctx
            .create(DEFAULT_VERTICAL_CRS)
            .and_then(|vertical| ctx.compound_crs(&format!("{crs} + EGM2008 height"), &src, &vertical))
            .ok_or_else(|| format!("{crs}にEGM2008の標高を組み合わせられない"))?;
        src = compound;
        warnings.push(format!(
            "CRS（{crs}）に高さの基準がないため、高さをEGM2008の標高とみなした。\
             高さの基準を含むCRS（例: EPSG:6677+6695）を --crs で指定すると、その国のジオイドを使う"
        ));
    }
    let op = ctx
        .create("EPSG:4978")
        .and_then(|dst| ctx.crs_to_crs(&src, &dst))
        .ok_or_else(|| format!("{crs}から地心座標への変換を作れない"))?;
    op.normalize_for_visualization().ok_or_else(|| format!("{crs}の軸順をそろえられない"))
}

/// ECEF → [緯度, 経度, 楕円体高]。
fn to_geodetic(p: [f64; 3]) -> [f64; 3] {
    let (lon, lat, h) = geocentric_to_geodetic(A, e_sq(), p[0], p[1], p[2]);
    [lat, lon, h]
}

fn geodetic(lat: f64, lon: f64, h: f64) -> [f64; 3] {
    let (x, y, z) = geodetic_to_geocentric(A, e_sq(), lon, lat, h);
    [x, y, z]
}

/// ECEFの点を原点とする東・北・上の直交フレーム。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    origin: [f64; 3],
    /// 東・北・上の単位ベクトル（ECEF）。
    basis: [[f64; 3]; 3],
}

impl Frame {
    pub fn at_geodetic(lat: f64, lon: f64, h: f64) -> Self {
        let (sl, cl) = lat.to_radians().sin_cos();
        let (so, co) = lon.to_radians().sin_cos();
        Self {
            origin: geodetic(lat, lon, h),
            basis: [[-so, co, 0.0], [-sl * co, -sl * so, cl], [cl * co, cl * so, sl]],
        }
    }

    pub fn at_ecef(p: [f64; 3]) -> Self {
        let [lat, lon, _] = to_geodetic(p);
        Self { origin: p, ..Self::at_geodetic(lat, lon, 0.0) }
    }

    fn to_ecef(self, l: [f64; 3]) -> [f64; 3] {
        let b = &self.basis;
        std::array::from_fn(|i| self.origin[i] + l[0] * b[0][i] + l[1] * b[1][i] + l[2] * b[2][i])
    }

    pub fn to_local(self, p: [f64; 3]) -> [f64; 3] {
        let d = sub(p, self.origin);
        self.basis.map(|b| dot(b, d))
    }

    /// フレーム座標→ECEFの4×4行列（列優先。tileset.jsonの`transform`）。
    pub fn transform(&self) -> [f64; 16] {
        let [e, n, u] = self.basis;
        let o = self.origin;
        [e[0], e[1], e[2], 0.0, n[0], n[1], n[2], 0.0, u[0], u[1], u[2], 0.0, o[0], o[1], o[2], 1.0]
    }
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(a: [f64; 3]) -> [f64; 3] {
    let l = dot(a, a).sqrt();
    a.map(|v| v / l)
}

/// グラム・シュミットの直交化。
fn orthonormalize([x, y, _]: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let x = normalize(x);
    let d = dot(y, x);
    let y = normalize([y[0] - d * x[0], y[1] - d * x[1], y[2] - d * x[2]]);
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    [x, y, z]
}

/// 回転（列ベクトルの配列）をベクトルに掛ける。
pub fn rotate(r: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| r[0][i] * v[0] + r[1][i] * v[1] + r[2][i] * v[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::georef::{EnuPlacement, GridPlacement};

    // 国土地理院 測量計算サイト（testdata/handmade/expected.json）
    const LAT: f64 = 35.681236;
    const LON: f64 = 139.767125;
    const EAST: f64 = -5992.9196;
    const NORTH: f64 = -35363.2377;
    const GEOID_2024: f64 = 36.7614;

    fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
        dot(sub(a, b), sub(a, b)).sqrt()
    }

    fn grid(crs: &str, origin: [f64; 3]) -> Placement {
        Placement::Grid(GridPlacement { crs: crs.into(), origin, rotation: 0.0, scale: 1.0, factors: [1.0; 3] })
    }

    fn enu(crs: &str, lat: f64, lon: f64, h: f64) -> Placement {
        Placement::Enu(EnuPlacement {
            crs: crs.into(),
            latitude_deg: lat,
            longitude_deg: lon,
            orthometric_height: h,
            rotation: 0.0,
        })
    }

    #[test]
    fn grid_known_point_matches_gsi() {
        // JGD2011 / 平面直角座標系IX系 + JGD2011の標高
        let (p, warnings) = Projector::new(&grid("EPSG:10170", [EAST, NORTH, 3.0])).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let got = p.to_ecef([0.0, 0.0, 0.0]).unwrap();
        let want = geodetic(LAT, LON, 3.0 + GEOID_2024);
        // 国土地理院の値の丸め（1e-6度≈0.1 m）を踏まえる
        assert!(dist(got, want) < 0.15, "{}", dist(got, want));
    }

    #[test]
    fn enu_origin_is_at_geodetic_point() {
        // JGD2011の経緯度 + JGD2011の標高
        let (p, warnings) = Projector::new(&enu("EPSG:6697", LAT, LON, 3.0)).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let want = geodetic(LAT, LON, 3.0 + GEOID_2024);
        assert!(dist(p.to_ecef([0.0; 3]).unwrap(), want) < 0.01, "{}", dist(p.to_ecef([0.0; 3]).unwrap(), want));
    }

    #[test]
    fn crs_without_height_uses_egm2008_with_warning() {
        let (p, warnings) = Projector::new(&grid("EPSG:6677", [EAST, NORTH, 3.0])).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("EGM2008"));
        // EGM2008とJPGEO2024の差は東京で1 m未満
        let h = to_geodetic(p.to_ecef([0.0; 3]).unwrap())[2];
        assert!((h - (3.0 + GEOID_2024)).abs() < 1.0, "{h}");
    }

    /// GeographicLib GeoConvert(1)の例: 38n 444500 3688500 → 33:20:03.25N 044:24:13.06E
    /// （https://geographiclib.sourceforge.io/C++/doc/GeoConvert.1.html）。秒の丸め（0.005″≈0.15 m）を踏まえる。
    #[test]
    fn utm_matches_geographiclib() {
        let (p, _) = Projector::new(&grid("EPSG:32638", [444_500.0, 3_688_500.0, 0.0])).unwrap();
        let [lat, lon, _] = to_geodetic(p.to_ecef([0.0; 3]).unwrap());
        let dms = |d: f64, m: f64, s: f64| d + m / 60.0 + s / 3600.0;
        assert!((lat - dms(33.0, 20.0, 3.25)).abs() < 0.006 / 3600.0, "{lat}");
        assert!((lon - dms(44.0, 24.0, 13.06)).abs() < 0.006 / 3600.0, "{lon}");
    }

    /// GeographicLibのテストデータ GeoidHeights.dat（NGAの球面調和関数の計算値）の1行:
    /// 47.2612 8.32186 → EGM2008 48.0227 m。PROJは2.5′格子を補間するため、数cmの差を許す。
    #[test]
    fn egm2008_matches_geographiclib() {
        let (p, warnings) = Projector::new(&enu("EPSG:4326", 47.2612, 8.32186, 0.0)).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        let h = to_geodetic(p.to_ecef([0.0; 3]).unwrap())[2];
        assert!((h - 48.0227).abs() < 0.05, "{h}");
    }

    #[test]
    fn wrong_kind_of_crs_is_rejected() {
        assert!(Projector::new(&grid("EPSG:4326", [0.0; 3])).is_err());
        assert!(Projector::new(&enu("EPSG:6677", 35.0, 139.0, 0.0)).is_err());
        assert!(Projector::new(&grid("EPSG:999999", [0.0; 3])).is_err());
    }

    #[test]
    fn frame_round_trip_and_transform() {
        let f = Frame::at_ecef(geodetic(LAT, LON, 40.0));
        let q = [10.0, -20.0, 5.0];
        let back = f.to_local(f.to_ecef(q));
        assert!(dist(back, q) < 1e-6);
        let m = f.transform();
        let e = f.to_ecef(q);
        let viam: [f64; 3] = std::array::from_fn(|i| m[i] * q[0] + m[4 + i] * q[1] + m[8 + i] * q[2] + m[12 + i]);
        assert!(dist(e, viam) < 1e-6);
    }

    #[test]
    fn rotation_in_grid_includes_meridian_convergence() {
        // IX系の既知点（中央子午線の西）では子午線収差が0.038616667°。
        // 地図の東（局所+X）は、真東から反時計回り（北寄り）にその角度だけ回っている
        let (p, _) = Projector::new(&grid("EPSG:10170", [EAST, NORTH, 3.0])).unwrap();
        let frame = Frame::at_ecef(p.to_ecef([0.0; 3]).unwrap());
        let r = p.rotation_at([0.0; 3], &frame).unwrap();
        let angle = r[0][1].atan2(r[0][0]).to_degrees();
        assert!((angle - 0.038616667).abs() < 1e-5, "{angle}");
        let n = rotate(&r, [0.0, 0.0, 1.0]);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }
}
