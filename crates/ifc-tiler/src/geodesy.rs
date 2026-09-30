//! 局所座標から地心直交座標（ECEF）への変換と、タイルを置く根のENUフレーム。
//!
//! 地図経路では頂点ごとに平面直角座標を逆投影する。建物の範囲でも接平面（ENU）で近似すると、
//! 子午線収差で100 mあたり約7 cmずれるため（調査12）。

use std::cell::Cell;

use geocentric::{geocentric_to_geodetic, geodetic_to_geocentric};
use japan_geoid::Geoid as _;
use japan_geoid::gsi::{MemoryGrid, load_embedded_gsigeo2011, load_embedded_jpgeo2024_hrefconv2024};

use crate::georef::Placement;

/// GRS80（JGD2011の楕円体）。
const A: f64 = 6_378_137.0;
const INV_F: f64 = 298.257_222_101;

fn e_sq() -> f64 {
    let f = 1.0 / INV_F;
    f * (2.0 - f)
}

/// 正標高→楕円体高に使うジオイドモデル。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GeoidModel {
    /// JPGEO2024＋Hrefconv2024（国土地理院「ジオイド2024日本とその周辺」）。
    Jpgeo2024,
    Gsigeo2011,
    /// 正標高をそのまま楕円体高とみなす。
    None,
}

impl GeoidModel {
    pub fn name(self) -> &'static str {
        match self {
            Self::Jpgeo2024 => "JPGEO2024+Hrefconv2024",
            Self::Gsigeo2011 => "GSIGEO2011",
            Self::None => "none",
        }
    }
}

/// 局所座標 [m] → ECEF [m]。
pub struct Projector {
    placement: Placement,
    geoid: Option<MemoryGrid<'static>>,
    /// ENU経路の原点のECEFと基底。
    enu: Option<Frame>,
    /// ジオイドの範囲外で0とした点の数。
    geoid_misses: Cell<usize>,
}

impl Projector {
    pub fn new(placement: Placement, model: GeoidModel) -> Self {
        let geoid = match model {
            GeoidModel::Jpgeo2024 => Some(load_embedded_jpgeo2024_hrefconv2024()),
            GeoidModel::Gsigeo2011 => Some(load_embedded_gsigeo2011()),
            GeoidModel::None => None,
        };
        let mut p = Self { placement, geoid, enu: None, geoid_misses: Cell::new(0) };
        if let Placement::Enu(e) = placement {
            let h = e.orthometric_height + p.geoid_height(e.longitude_deg, e.latitude_deg);
            p.enu = Some(Frame::at_geodetic(e.latitude_deg, e.longitude_deg, h));
        }
        p
    }

    pub fn geoid_misses(&self) -> usize {
        self.geoid_misses.get()
    }

    fn geoid_height(&self, lon: f64, lat: f64) -> f64 {
        let Some(g) = &self.geoid else { return 0.0 };
        let h = g.get_height(lon, lat);
        if h.is_finite() {
            h
        } else {
            self.geoid_misses.set(self.geoid_misses.get() + 1);
            0.0
        }
    }

    /// 局所座標 [m] → ECEF [m]。地図座標が投影の定義域外なら、その地図座標を返す。
    pub fn to_ecef(&self, p: [f64; 3]) -> Result<[f64; 3], [f64; 3]> {
        match (&self.placement, &self.enu) {
            (Placement::Enu(e), Some(frame)) => Ok(frame.to_ecef(e.to_enu(p))),
            (Placement::Grid(g), _) => {
                let [east, north, h] = g.to_map(p);
                let (lon, lat, _) =
                    g.zone.projection().project_inverse(east, north, 0.0).map_err(|_| [east, north, h])?;
                Ok(geodetic(lat, lon, h + self.geoid_height(lon, lat)))
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

/// ECEF → [緯度, 経度, 楕円体高]。
pub fn to_geodetic(p: [f64; 3]) -> [f64; 3] {
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
    pub origin: [f64; 3],
    /// 東・北・上の単位ベクトル（ECEF）。
    pub basis: [[f64; 3]; 3],
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

    pub fn to_ecef(&self, l: [f64; 3]) -> [f64; 3] {
        let b = &self.basis;
        std::array::from_fn(|i| self.origin[i] + l[0] * b[0][i] + l[1] * b[1][i] + l[2] * b[2][i])
    }

    pub fn to_local(&self, p: [f64; 3]) -> [f64; 3] {
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

pub fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
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
    use jprect::JPRZone;

    // 国土地理院 測量計算サイト（testdata/handmade/expected.json）
    const LAT: f64 = 35.681236;
    const LON: f64 = 139.767125;
    const EAST: f64 = -5992.9196;
    const NORTH: f64 = -35363.2377;
    const GEOID_2024: f64 = 36.7614;

    fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
        dot(sub(a, b), sub(a, b)).sqrt()
    }

    fn grid() -> Placement {
        Placement::Grid(GridPlacement {
            zone: JPRZone::Zone9,
            epsg: 6677,
            origin: [EAST, NORTH, 3.0],
            rotation: 0.0,
            scale: 1.0,
            factors: [1.0; 3],
        })
    }

    #[test]
    fn grid_known_point_matches_gsi() {
        let p = Projector::new(grid(), GeoidModel::Jpgeo2024);
        let got = p.to_ecef([0.0, 0.0, 0.0]).unwrap();
        let want = geodetic(LAT, LON, 3.0 + GEOID_2024);
        // 国土地理院の値の丸め（0.1 mm、1e-6度≈0.1 m）を踏まえ、1 cm以内
        assert!(dist(got, want) < 0.15, "{}", dist(got, want));
        assert_eq!(p.geoid_misses(), 0);
    }

    #[test]
    fn enu_origin_is_at_geodetic_point() {
        let e = Placement::Enu(EnuPlacement {
            latitude_deg: LAT,
            longitude_deg: LON,
            orthometric_height: 3.0,
            rotation: 0.0,
        });
        let p = Projector::new(e, GeoidModel::Jpgeo2024);
        let want = geodetic(LAT, LON, 3.0 + GEOID_2024);
        assert!(dist(p.to_ecef([0.0; 3]).unwrap(), want) < 1e-3);
    }

    #[test]
    fn geoid_outside_japan_counts_misses() {
        let e = Placement::Enu(EnuPlacement {
            latitude_deg: 46.36,
            longitude_deg: 14.09,
            orthometric_height: 0.0,
            rotation: 0.0,
        });
        let p = Projector::new(e, GeoidModel::Jpgeo2024);
        assert_eq!(p.geoid_misses(), 1);
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
        let p = Projector::new(grid(), GeoidModel::None);
        let frame = Frame::at_ecef(p.to_ecef([0.0; 3]).unwrap());
        let r = p.rotation_at([0.0; 3], &frame).unwrap();
        let angle = r[0][1].atan2(r[0][0]).to_degrees();
        assert!((angle - 0.038616667).abs() < 1e-5, "{angle}");
        let n = rotate(&r, [0.0, 0.0, 1.0]);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }
}
