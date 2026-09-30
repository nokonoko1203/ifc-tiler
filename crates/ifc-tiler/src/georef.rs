//! Resolution of the georeferencing.
//!
//! From the raw values read from the IFC (`RawGeoref`) and the user's options (`GeorefOptions`),
//! decides one way (`Placement`) to place the local coordinates (IFC world coordinates, m) on the globe.
//! The priority is `--origin` → `--map-conversion` → `IfcMapConversion` → the latitude/longitude of `IfcSite`.
//! CRSs are kept as strings; parsing and conversion are done in `geodesy` (PROJ).

/// Raw georeferencing data read from the IFC. Lengths and angles are in the units written in the file.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawGeoref {
    pub map_conversion: Option<MapConversion>,
    pub site: Option<SiteReference>,
    /// `TrueNorth` of the model's 3D context (the direction of true north in the local XY plane).
    pub true_north: Option<[f64; 2]>,
    /// Length unit of the project [m].
    pub length_unit_m: f64,
}

/// `IfcMapConversion` / `IfcMapConversionScaled`.
#[derive(Clone, Debug, PartialEq)]
pub struct MapConversion {
    pub eastings: f64,
    pub northings: f64,
    pub orthogonal_height: f64,
    pub x_axis_abscissa: Option<f64>,
    pub x_axis_ordinate: Option<f64>,
    pub scale: Option<f64>,
    /// Per-axis factors of `IfcMapConversionScaled`. 1 if absent.
    pub factors: [f64; 3],
    pub target: Crs,
}

/// The target CRS (`IfcProjectedCRS`). Anything other than a projected CRS is read as `Missing`.
#[derive(Clone, Debug, PartialEq)]
pub enum Crs {
    Projected { name: Option<String>, map_unit_m: Option<f64> },
    Missing,
}

/// `RefLatitude` / `RefLongitude` / `RefElevation` of `IfcSite`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SiteReference {
    pub latitude_deg: Option<f64>,
    pub longitude_deg: Option<f64>,
    /// In project length units.
    pub elevation: Option<f64>,
}

/// The user's options.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeorefOptions {
    /// A CRS PROJ can parse (`EPSG:6677`, `EPSG:6677+6695`, …). On the map path it overrides the TargetCRS of
    /// `IfcMapConversion`; on the ENU path it is the CRS of the origin's latitude, longitude and height.
    pub crs: Option<String>,
    /// Latitude and longitude [degrees] and elevation [m].
    pub origin: Option<[f64; 3]>,
    /// Map coordinates of the local origin (easting, northing, elevation [m]) and the counterclockwise rotation of the local X axis from east [degrees]. Requires `--crs`.
    pub map_conversion: Option<[f64; 4]>,
}

/// The CRS of the origin's latitude and longitude on the ENU path when there is no `--crs` (WGS84).
const DEFAULT_GEOGRAPHIC_CRS: &str = "EPSG:4326";

/// A way to place local coordinates (m) on the globe.
#[derive(Clone, Debug, PartialEq)]
pub enum Placement {
    /// Local coordinates → map coordinates (east, north) and elevation.
    Grid(GridPlacement),
    /// East, north and height (ENU) around an origin.
    Enu(EnuPlacement),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridPlacement {
    /// The CRS of the map coordinates (a projected CRS; it may include a height reference).
    pub crs: String,
    /// Map coordinates [m] and elevation [m] of the local origin.
    pub origin: [f64; 3],
    /// Counterclockwise rotation of the local X axis from map east [rad].
    pub rotation: f64,
    /// Scale applied to the local coordinates after conversion to metres.
    pub scale: f64,
    pub factors: [f64; 3],
}

impl GridPlacement {
    /// Local coordinates [m] → (east, north, elevation) [m].
    pub fn to_map(&self, p: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.rotation.sin_cos();
        let x = self.scale * self.factors[0] * p[0];
        let y = self.scale * self.factors[1] * p[1];
        let z = self.scale * self.factors[2] * p[2];
        [self.origin[0] + x * c - y * s, self.origin[1] + x * s + y * c, self.origin[2] + z]
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EnuPlacement {
    /// The CRS of the origin's latitude, longitude and height (a geographic CRS; it may include a height reference).
    pub crs: String,
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub orthometric_height: f64,
    /// Counterclockwise rotation of the local X axis from east [rad].
    pub rotation: f64,
}

impl EnuPlacement {
    /// Local coordinates [m] → ENU around the origin [m].
    pub fn to_enu(&self, p: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.rotation.sin_cos();
        [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
    }
}

/// The result of the resolution.
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub placement: Placement,
    pub warnings: Vec<String>,
}

/// When placing with ENU, warn if any geometry is farther than this from the local origin [m].
const FAR_FROM_ORIGIN_M: f64 = 1000.0;

/// Resolves the georeferencing. `reach_m` is the maximum horizontal distance of the geometry from the local origin.
pub fn resolve(raw: &RawGeoref, opts: &GeorefOptions, reach_m: f64) -> Result<Resolved, String> {
    let mut warnings = Vec::new();
    let placement = if let Some([lat, lon, h]) = opts.origin {
        Placement::Enu(EnuPlacement {
            crs: geographic_crs(opts),
            latitude_deg: lat,
            longitude_deg: lon,
            orthometric_height: h,
            rotation: 0.0,
        })
    } else if let Some(given) = opts.map_conversion {
        given_map_conversion(raw, given, opts, &mut warnings)?
    } else if let Some(mc) = &raw.map_conversion {
        map_conversion(raw, mc, opts, &mut warnings)?
    } else if let Some((lat, lon)) = site_lat_lon(raw.site.as_ref(), &mut warnings) {
        let elevation = raw.site.as_ref().and_then(|s| s.elevation).unwrap_or(0.0) * raw.length_unit_m;
        site(raw, lat, lon, elevation, opts)
    } else {
        return Err("no georeferencing (neither IfcMapConversion nor the latitude/longitude of IfcSite); \
                    specify an origin with --origin LAT,LON[,H] to place the model as east, north and height around that point"
            .into());
    };
    if matches!(placement, Placement::Enu(_)) && opts.origin.is_none() && reach_m > FAR_FROM_ORIGIN_M {
        warnings.push(format!(
            "the geometry extends up to {reach_m:.0} m from the local origin; if the local coordinates are map coordinates, \
             specify --map-conversion 0,0 --crs EPSG:xxxx"
        ));
    }
    Ok(Resolved { placement, warnings })
}

fn map_conversion(
    raw: &RawGeoref,
    mc: &MapConversion,
    opts: &GeorefOptions,
    warnings: &mut Vec<String>,
) -> Result<Placement, String> {
    let (name, map_unit_m) = match &mc.target {
        Crs::Projected { name, map_unit_m } => (name.as_deref(), map_unit_m.unwrap_or(raw.length_unit_m)),
        Crs::Missing => (None, raw.length_unit_m),
    };
    let crs = crs_for(opts.crs.as_deref(), name)?;
    let spec_scale = mc.scale.unwrap_or(1.0) * map_unit_m / raw.length_unit_m;
    let scale = effective_scale(spec_scale, mc.scale, map_unit_m, raw.length_unit_m, warnings);
    let abscissa = mc.x_axis_abscissa.unwrap_or(1.0);
    let ordinate = mc.x_axis_ordinate.unwrap_or(0.0);
    Ok(Placement::Grid(GridPlacement {
        crs,
        origin: [mc.eastings * map_unit_m, mc.northings * map_unit_m, mc.orthogonal_height * map_unit_m],
        rotation: ordinate.atan2(abscissa),
        scale,
        factors: mc.factors,
    }))
}

/// The map coordinate reference given by `--map-conversion`. Local coordinates (m) are used as is, with a scale of 1.
fn given_map_conversion(
    raw: &RawGeoref,
    [e, n, h, rotation_deg]: [f64; 4],
    opts: &GeorefOptions,
    warnings: &mut Vec<String>,
) -> Result<Placement, String> {
    let crs = opts.crs.clone().ok_or("--map-conversion requires --crs EPSG:xxxx")?;
    if raw.map_conversion.is_some() {
        warnings.push("placed with --map-conversion instead of the file's IfcMapConversion".into());
    }
    Ok(Placement::Grid(GridPlacement {
        crs,
        origin: [e, n, h],
        rotation: rotation_deg.to_radians(),
        scale: 1.0,
        factors: [1.0; 3],
    }))
}

/// Decides the effective scale of `Scale`. If the effective scale is off by a factor of 100 or more and its inverse is 1,
/// it is assumed to have been written as the inverse (as in some official samples) and the inverse is used.
fn effective_scale(
    spec_scale: f64,
    scale: Option<f64>,
    map_unit_m: f64,
    length_unit_m: f64,
    warnings: &mut Vec<String>,
) -> f64 {
    let inverse = scale.filter(|s| *s != 0.0).map(|s| map_unit_m / (s * length_unit_m));
    if spec_scale.log10().abs() > 2.0
        && let Some(inv) = inverse
        && inv.log10().abs() < 0.01
    {
        warnings
            .push(format!("the effective scale of IfcMapConversion.Scale is {spec_scale}; assuming it was written as the inverse and using {inv}"));
        return inv;
    }
    if (spec_scale - 1.0).abs() > 0.01 {
        warnings.push(format!(
            "the effective scale of IfcMapConversion.Scale is {spec_scale}, which differs from 1 by 1% or more"
        ));
    }
    spec_scale
}

fn site_lat_lon(site: Option<&SiteReference>, warnings: &mut Vec<String>) -> Option<(f64, f64)> {
    let s = site?;
    let (lat, lon) = (s.latitude_deg?, s.longitude_deg?);
    if lat == 0.0 && lon == 0.0 {
        warnings.push("the latitude/longitude of IfcSite is (0, 0), so it is treated as unset".into());
        return None;
    }
    if let Some(what) = known_default(lat, lon) {
        warnings.push(format!(
            "the latitude/longitude of IfcSite ({lat:.6}, {lon:.6}) matches {what} and is probably not the real location; \
             place the model again with --origin LAT,LON[,H] or --map-conversion E,N --crs EPSG:xxxx"
        ));
    }
    Some((lat, lon))
}

/// East, north and height around the latitude/longitude of IfcSite. TrueNorth points north in the local XY plane,
/// so the angle of the local X axis from east is 90° − atan2(ty, tx).
fn site(raw: &RawGeoref, lat: f64, lon: f64, elevation: f64, opts: &GeorefOptions) -> Placement {
    let rotation = raw.true_north.map_or(0.0, |[tx, ty]| std::f64::consts::FRAC_PI_2 - ty.atan2(tx));
    Placement::Enu(EnuPlacement {
        crs: geographic_crs(opts),
        latitude_deg: lat,
        longitude_deg: lon,
        orthometric_height: elevation,
        rotation,
    })
}

fn geographic_crs(opts: &GeorefOptions) -> String {
    opts.crs.clone().unwrap_or_else(|| DEFAULT_GEOGRAPHIC_CRS.into())
}

/// Latitudes and longitudes of IfcSite that look like authoring-tool defaults [degrees]. The same values were seen in several unrelated files
/// (values written in degrees, minutes, seconds and millionths of a second, converted to degrees).
const KNOWN_DEFAULTS: [(f64, f64, &str); 2] = [
    // (42,24,53,508911), (-71,-15,-29,-58837)
    (42.414_863_586_4, -71.258_071_899_2, "Revit's default location (Massachusetts, USA)"),
    // (35,41,6,4943), (139,45,3,625488)
    (35.685_001_373_1, 139.751_007_080_0, "Tokyo in Revit's city list"),
];

fn known_default(lat: f64, lon: f64) -> Option<&'static str> {
    KNOWN_DEFAULTS.iter().find(|(a, b, _)| (lat - a).abs() < 1e-6 && (lon - b).abs() < 1e-6).map(|&(_, _, w)| w)
}

/// Decides the CRS of the map coordinates. `--crs` takes priority; otherwise the EPSG code is read from the IFC's CRS name.
fn crs_for(override_crs: Option<&str>, name: Option<&str>) -> Result<String, String> {
    if let Some(c) = override_crs {
        return Ok(c.to_string());
    }
    name.and_then(parse_epsg)
        .map(|code| format!("EPSG:{code}"))
        .ok_or_else(|| format!("unknown CRS ({}); specify it with --crs EPSG:xxxx", name.unwrap_or("no name")))
}

/// Extracts the code from `EPSG:6677`, `EPSG: 6677`, `urn:ogc:def:crs:EPSG::6677` and the like.
fn parse_epsg(s: &str) -> Option<u32> {
    let upper = s.to_ascii_uppercase();
    let rest = &upper[upper.find("EPSG")? + 4..];
    let digits: String = rest.trim_start_matches([':', ' ']).chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_mm() -> RawGeoref {
        RawGeoref { length_unit_m: 0.001, ..Default::default() }
    }

    fn opts() -> GeorefOptions {
        GeorefOptions::default()
    }

    fn mc(scale: f64, map_unit_m: Option<f64>) -> MapConversion {
        MapConversion {
            eastings: -5992.9196,
            northings: -35363.2377,
            orthogonal_height: 3.0,
            x_axis_abscissa: Some(1.0),
            x_axis_ordinate: Some(0.0),
            scale: Some(scale),
            factors: [1.0; 3],
            target: Crs::Projected { name: Some("EPSG:6677".into()), map_unit_m },
        }
    }

    fn grid(r: &Resolved) -> GridPlacement {
        match &r.placement {
            Placement::Grid(g) => g.clone(),
            Placement::Enu(_) => panic!("grid expected"),
        }
    }

    #[test]
    fn epsg_is_parsed_from_common_spellings() {
        assert_eq!(parse_epsg("EPSG:6677"), Some(6677));
        assert_eq!(parse_epsg("epsg: 10170"), Some(10170));
        assert_eq!(parse_epsg("urn:ogc:def:crs:EPSG::6677"), Some(6677));
        assert_eq!(parse_epsg("JGD2011 / Japan Plane Rectangular CS IX"), None);
    }

    #[test]
    fn map_conversion_in_millimetre_project_with_metre_map_unit() {
        let raw = RawGeoref { map_conversion: Some(mc(0.001, Some(1.0))), ..raw_mm() };
        let r = resolve(&raw, &opts(), 0.0).unwrap();
        let g = grid(&r);
        assert_eq!(g.crs, "EPSG:6677");
        assert!((g.scale - 1.0).abs() < 1e-12);
        assert_eq!(g.to_map([0.0, 0.0, 0.0]), [-5992.9196, -35363.2377, 3.0]);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn omitted_map_unit_means_project_unit() {
        // No MapUnit means project units (mm). Eastings is also written in mm
        let mut m = mc(1.0, None);
        m.eastings *= 1000.0;
        m.northings *= 1000.0;
        m.orthogonal_height *= 1000.0;
        let raw = RawGeoref { map_conversion: Some(m), ..raw_mm() };
        let g = grid(&resolve(&raw, &opts(), 0.0).unwrap());
        assert!((g.scale - 1.0).abs() < 1e-12);
        assert!((g.origin[0] + 5992.9196).abs() < 1e-9);
    }

    #[test]
    fn inverted_scale_is_detected() {
        // By the spec Scale would be 0.001, but 1000 is written
        let raw = RawGeoref { map_conversion: Some(mc(1000.0, Some(1.0))), ..raw_mm() };
        let r = resolve(&raw, &opts(), 0.0).unwrap();
        assert!((grid(&r).scale - 1.0).abs() < 1e-12);
        assert_eq!(r.warnings.len(), 1);
    }

    #[test]
    fn rotation_and_axis_factors() {
        let mut m = mc(0.001, Some(1.0));
        m.x_axis_abscissa = Some(0.0);
        m.x_axis_ordinate = Some(2.0); // Need not be normalized. 90°
        m.factors = [2.0, 1.0, 1.0];
        let raw = RawGeoref { map_conversion: Some(m), ..raw_mm() };
        let g = grid(&resolve(&raw, &opts(), 0.0).unwrap());
        let p = g.to_map([1.0, 0.0, 0.0]);
        assert!((p[0] + 5992.9196).abs() < 1e-9 && (p[1] + 35363.2377 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn crs_option_overrides_the_file_and_unknown_names_are_rejected() {
        let mut m = mc(0.001, Some(1.0));
        m.target =
            Crs::Projected { name: Some("JGD2011 / Japan Plane Rectangular CS IX".into()), map_unit_m: Some(1.0) };
        let raw = RawGeoref { map_conversion: Some(m), ..raw_mm() };
        assert!(resolve(&raw, &opts(), 0.0).unwrap_err().contains("--crs"));
        let o = GeorefOptions { crs: Some("EPSG:6677+6695".into()), ..opts() };
        assert_eq!(grid(&resolve(&raw, &o, 0.0).unwrap()).crs, "EPSG:6677+6695");
    }

    #[test]
    fn enu_uses_crs_option_or_wgs84() {
        let raw = RawGeoref {
            site: Some(SiteReference { latitude_deg: Some(35.0), longitude_deg: Some(139.0), elevation: None }),
            ..raw_mm()
        };
        let Placement::Enu(e) = resolve(&raw, &opts(), 0.0).unwrap().placement else { panic!() };
        assert_eq!(e.crs, DEFAULT_GEOGRAPHIC_CRS);
        let o = GeorefOptions { crs: Some("EPSG:6697".into()), ..opts() };
        let Placement::Enu(e) = resolve(&raw, &o, 0.0).unwrap().placement else { panic!() };
        assert_eq!(e.crs, "EPSG:6697");
    }

    #[test]
    fn site_at_zero_zero_is_ignored() {
        let raw = RawGeoref {
            site: Some(SiteReference { latitude_deg: Some(0.0), longitude_deg: Some(0.0), elevation: None }),
            ..raw_mm()
        };
        let e = resolve(&raw, &opts(), 0.0).unwrap_err();
        assert!(e.contains("--origin"));
    }

    #[test]
    fn site_enu_uses_elevation_in_project_units_and_true_north() {
        let raw = RawGeoref {
            site: Some(SiteReference { latitude_deg: Some(35.0), longitude_deg: Some(139.0), elevation: Some(3000.0) }),
            true_north: Some([1.0, 0.0]), // local +X is north
            ..raw_mm()
        };
        let r = resolve(&raw, &opts(), 0.0).unwrap();
        let Placement::Enu(p) = r.placement else { panic!() };
        assert_eq!(p.orthometric_height, 3.0);
        let enu = p.to_enu([1.0, 0.0, 0.0]);
        assert!(enu[0].abs() < 1e-12 && (enu[1] - 1.0).abs() < 1e-12, "{enu:?}");
    }

    #[test]
    fn far_geometry_in_enu_is_warned() {
        let raw = RawGeoref {
            site: Some(SiteReference { latitude_deg: Some(36.0), longitude_deg: Some(139.8), elevation: None }),
            ..raw_mm()
        };
        assert_eq!(resolve(&raw, &opts(), 35_000.0).unwrap().warnings.len(), 1);
        assert!(resolve(&raw, &opts(), 500.0).unwrap().warnings.is_empty());
    }

    #[test]
    fn origin_option_wins() {
        let raw = RawGeoref { map_conversion: Some(mc(0.001, Some(1.0))), ..raw_mm() };
        let o = GeorefOptions { origin: Some([35.0, 139.0, 10.0]), ..opts() };
        let r = resolve(&raw, &o, 1e6).unwrap();
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn given_map_conversion_needs_crs_and_wins_over_the_file() {
        let raw = RawGeoref { map_conversion: Some(mc(0.001, Some(1.0))), ..raw_mm() };
        let no_crs = GeorefOptions { map_conversion: Some([0.0, 0.0, 0.0, 0.0]), ..opts() };
        assert!(resolve(&raw, &no_crs, 0.0).unwrap_err().contains("--crs"));
        let o =
            GeorefOptions { crs: Some("EPSG:6677".into()), map_conversion: Some([100.0, 200.0, 3.0, 90.0]), ..opts() };
        let r = resolve(&raw, &o, 0.0).unwrap();
        assert_eq!(r.warnings.len(), 1);
        let g = grid(&r);
        assert_eq!((g.crs.as_str(), g.scale), ("EPSG:6677", 1.0));
        // Rotating the local X axis 90° from east moves local (1, 0) 1 m to the north
        let p = g.to_map([1.0, 0.0, 2.0]);
        assert!((p[0] - 100.0).abs() < 1e-9 && (p[1] - 201.0).abs() < 1e-9 && (p[2] - 5.0).abs() < 1e-9, "{p:?}");
    }

    #[test]
    fn known_default_site_locations_are_warned() {
        let site = |lat, lon| RawGeoref {
            site: Some(SiteReference { latitude_deg: Some(lat), longitude_deg: Some(lon), elevation: None }),
            ..raw_mm()
        };
        let boston = resolve(&site(42.414_863_586_4, -71.258_071_899_2), &opts(), 0.0).unwrap();
        assert!(boston.warnings.iter().any(|w| w.contains("Revit's default location")), "{:?}", boston.warnings);
        let tokyo = resolve(&site(35.685_001_4, 139.751_007_1), &opts(), 0.0).unwrap();
        assert!(tokyo.warnings.iter().any(|w| w.contains("Tokyo")), "{:?}", tokyo.warnings);
        assert!(resolve(&site(35.681_236, 139.767_125), &opts(), 0.0).unwrap().warnings.is_empty());
    }
}
