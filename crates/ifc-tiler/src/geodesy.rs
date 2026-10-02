//! Local coordinates → ECEF conversion, and the root ENU frame the tiles sit in
//!
//! PROJ converts map coordinates and latitude/longitude to ECEF (any CRS expressible as an EPSG code works)
//! On the map path every vertex is converted; even within a building, a tangent plane (ENU) approximation
//! would drift a few centimetres per 100 m because of meridian convergence

use geocentric::{geocentric_to_geodetic, geodetic_to_geocentric};

use crate::georef::Placement;
use crate::proj::{Context, Kind, Object};

/// WGS84 (the ellipsoid of `EPSG:4978`)
const A: f64 = 6_378_137.0;
const INV_F: f64 = 298.257_223_563;

fn e_sq() -> f64 {
    let f = 1.0 / INV_F;
    f * (2.0 - f)
}

/// Height reference used when the CRS has no vertical part (EGM2008 height)
const DEFAULT_VERTICAL_CRS: &str = "EPSG:3855";

/// How to get grids, appended to the approximation warning
/// The bundled PROJ is built without curl and libtiff, so it can neither download nor read grids
#[cfg(not(feature = "bundled"))]
const GRID_HINT: &str = "connect to the network, or fetch the required grids with projsync";
#[cfg(feature = "bundled")]
const GRID_HINT: &str = "this build cannot use grids; to use them, build from source with a system PROJ";

/// The transformation, its warnings, and the ECEF of the reference point (`None` if it cannot be transformed)
type Attempt = (Projector, Vec<String>, Option<[f64; 3]>);

/// Local coordinates [m] → ECEF [m]
pub struct Projector {
    /// Input CRS → `EPSG:4978`. The axis order is normalized to (east, north) or (longitude, latitude)
    pj: Object,
    placement: Placement,
    /// ECEF origin and basis of the ENU path
    enu: Option<Frame>,
}

impl Projector {
    /// Also returns warnings
    ///
    /// Like point-tiler, grids (geoids etc.) are downloaded from cdn.proj.org and cached
    /// If they cannot be downloaded the transformation itself fails, so it is rebuilt with the network disabled
    /// to let PROJ pick an approximate transformation that does not use grids (the approximation is reported as a warning)
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
                        "cannot transform the coordinates {reference:?} (outside the domain of the CRS); check --crs and the georeferencing"
                    ));
                }
            },
        };
        if let Some(name) = p.last_ballpark() {
            warnings.push(format!(
                "the coordinate transformation ({name}) is an approximation that does not use grids, so heights and positions may be off by several metres or more; {GRID_HINT}"
            ));
        }
        if let Placement::Enu(e) = placement {
            p.enu = Some(Frame { origin, ..Frame::at_geodetic(e.latitude_deg, e.longitude_deg, 0.0) });
        }
        Ok((p, warnings))
    }

    /// Builds the transformation and tries to transform the reference point. The third value is `None` if that fails
    fn with_network(placement: &Placement, network: bool, reference: [f64; 3]) -> Result<Attempt, String> {
        let mut warnings = Vec::new();
        let ctx = Context::new().ok_or("cannot create a PROJ context")?;
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

    /// Name of the last transformation if it was a grid-free approximation (ballpark)
    fn last_ballpark(&self) -> Option<String> {
        // A transformation with a single candidate has no last-used operation, so look at the transformation itself
        let last = self.pj.last_used_operation();
        let op = last.as_ref().unwrap_or(&self.pj);
        op.has_ballpark_transformation().then(|| op.name())
    }

    /// Local coordinates [m] → ECEF [m]. Returns the map coordinates as the error if they cannot be transformed
    pub fn to_ecef(&self, p: [f64; 3]) -> Result<[f64; 3], [f64; 3]> {
        match (&self.placement, &self.enu) {
            (Placement::Enu(e), Some(frame)) => Ok(frame.to_ecef(e.to_enu(p))),
            (Placement::Grid(g), _) => {
                let map = g.to_map(p);
                self.trans(map).ok_or(map)
            }
            (Placement::Enu(_), None) => unreachable!("the ENU frame is created in new"),
        }
    }

    /// Rotation near point `c` from the local axes to the axes of `frame` (columns are where the local x, y and z axes go)
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

/// Builds the transformation input CRS → `EPSG:4978`. If there is no vertical CRS, heights are treated as EGM2008 heights
fn source_to_ecef(ctx: &Context, placement: &Placement, warnings: &mut Vec<String>) -> Result<Object, String> {
    let (crs, want_projected) = match placement {
        Placement::Grid(g) => (g.crs.as_str(), true),
        Placement::Enu(e) => (e.crs.as_str(), false),
    };
    let mut src =
        ctx.create(crs).ok_or_else(|| format!("cannot parse the CRS ({crs}); specify it with --crs EPSG:xxxx"))?;
    let kind = src.kind();
    let horizontal = if kind == Kind::Compound { src.sub_crs(0).map_or(Kind::Other, |h| h.kind()) } else { kind };
    let geographic = matches!(horizontal, Kind::Geographic2d | Kind::Geographic3d);
    let ok = if want_projected { horizontal == Kind::Projected } else { geographic };
    if !ok {
        let what = if want_projected {
            "a projected CRS is required for map coordinates"
        } else {
            "a geographic CRS is required for latitude and longitude"
        };
        return Err(format!(
            "{crs} cannot be used: {what} (e.g. {})",
            if want_projected { "EPSG:32654" } else { "EPSG:4326" }
        ));
    }
    if matches!(kind, Kind::Projected | Kind::Geographic2d) {
        let compound = ctx
            .create(DEFAULT_VERTICAL_CRS)
            .and_then(|vertical| ctx.compound_crs(&format!("{crs} + EGM2008 height"), &src, &vertical))
            .ok_or_else(|| format!("cannot combine {crs} with EGM2008 heights"))?;
        src = compound;
        warnings.push(format!(
            "the CRS ({crs}) has no height reference, so heights are treated as EGM2008 heights; \
             specify a CRS that includes a height reference (e.g. EPSG:6677+6695) with --crs to use that country's geoid"
        ));
    }
    let op = ctx
        .create("EPSG:4978")
        .and_then(|dst| ctx.crs_to_crs(&src, &dst))
        .ok_or_else(|| format!("cannot build a transformation from {crs} to geocentric coordinates"))?;
    op.normalize_for_visualization().ok_or_else(|| format!("cannot normalize the axis order of {crs}"))
}

/// ECEF → [latitude, longitude, ellipsoidal height]
fn to_geodetic(p: [f64; 3]) -> [f64; 3] {
    let (lon, lat, h) = geocentric_to_geodetic(A, e_sq(), p[0], p[1], p[2]);
    [lat, lon, h]
}

fn geodetic(lat: f64, lon: f64, h: f64) -> [f64; 3] {
    let (x, y, z) = geodetic_to_geocentric(A, e_sq(), lon, lat, h);
    [x, y, z]
}

/// East-north-up frame with an ECEF point as its origin
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    origin: [f64; 3],
    /// Unit vectors of east, north and up (ECEF)
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

    /// 4×4 matrix from frame coordinates to ECEF (column-major; the `transform` of tileset.json)
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

/// Gram–Schmidt orthogonalization
fn orthonormalize([x, y, _]: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let x = normalize(x);
    let d = dot(y, x);
    let y = normalize([y[0] - d * x[0], y[1] - d * x[1], y[2] - d * x[2]]);
    let z = [x[1] * y[2] - x[2] * y[1], x[2] * y[0] - x[0] * y[2], x[0] * y[1] - x[1] * y[0]];
    [x, y, z]
}

/// Applies a rotation (an array of column vectors) to a vector
pub fn rotate(r: &[[f64; 3]; 3], v: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|i| r[0][i] * v[0] + r[1][i] * v[1] + r[2][i] * v[2])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::georef::{EnuPlacement, GridPlacement};

    // Geospatial Information Authority of Japan (GSI) survey calculation site (testdata/handmade/expected.json)
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
        // JGD2011 / Japan Plane Rectangular CS IX + JGD2011 height
        let (p, warnings) = Projector::new(&grid("EPSG:10170", [EAST, NORTH, 3.0])).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        let got = p.to_ecef([0.0, 0.0, 0.0]).unwrap();
        let want = geodetic(LAT, LON, 3.0 + GEOID_2024);
        // Allows for the rounding of the GSI values (1e-6 degrees ≈ 0.1 m)
        assert!(dist(got, want) < 0.15, "{}", dist(got, want));
    }

    #[test]
    fn enu_origin_is_at_geodetic_point() {
        // JGD2011 latitude/longitude + JGD2011 height
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
        // The difference between EGM2008 and JPGEO2024 is under 1 m in Tokyo
        let h = to_geodetic(p.to_ecef([0.0; 3]).unwrap())[2];
        assert!((h - (3.0 + GEOID_2024)).abs() < 1.0, "{h}");
    }

    /// Example from GeographicLib GeoConvert(1): 38n 444500 3688500 → 33:20:03.25N 044:24:13.06E
    /// (https://geographiclib.sourceforge.io/C++/doc/GeoConvert.1.html). Allows for the rounding of the seconds (0.005″ ≈ 0.15 m)
    #[test]
    fn utm_matches_geographiclib() {
        let (p, _) = Projector::new(&grid("EPSG:32638", [444_500.0, 3_688_500.0, 0.0])).unwrap();
        let [lat, lon, _] = to_geodetic(p.to_ecef([0.0; 3]).unwrap());
        let dms = |d: f64, m: f64, s: f64| d + m / 60.0 + s / 3600.0;
        assert!((lat - dms(33.0, 20.0, 3.25)).abs() < 0.006 / 3600.0, "{lat}");
        assert!((lon - dms(44.0, 24.0, 13.06)).abs() < 0.006 / 3600.0, "{lon}");
    }

    /// One line of GeographicLib's test data GeoidHeights.dat (values computed from NGA's spherical harmonics):
    /// 47.2612 8.32186 → EGM2008 48.0227 m. PROJ interpolates a 2.5′ grid, so a difference of a few centimetres is allowed
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
        // At the known point in zone IX (west of the central meridian) the meridian convergence is 0.038616667°
        // Map east (local +X) is rotated counterclockwise (toward north) from true east by that angle
        let (p, _) = Projector::new(&grid("EPSG:10170", [EAST, NORTH, 3.0])).unwrap();
        let frame = Frame::at_ecef(p.to_ecef([0.0; 3]).unwrap());
        let r = p.rotation_at([0.0; 3], &frame).unwrap();
        let angle = r[0][1].atan2(r[0][0]).to_degrees();
        assert!((angle - 0.038616667).abs() < 1e-5, "{angle}");
        let n = rotate(&r, [0.0, 0.0, 1.0]);
        assert!((n[2] - 1.0).abs() < 1e-6);
    }
}
