//! ジオリファレンスの解決。
//!
//! IFCから読んだ生の値（`RawGeoref`）と利用者の指定（`GeorefOptions`）から、
//! 局所座標（IFCの世界座標、m）を地球上に置く方法（`Placement`）を1つ決める。
//! 優先順位は `--origin` → `--map-conversion` → `IfcMapConversion` → `IfcRigidOperation` → `IfcSite`の経緯度。

use jprect::JPRZone;

/// IFCから読んだジオリファレンスの生データ。長さ・角度の値はファイルに書かれた単位のまま。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawGeoref {
    pub map_conversion: Option<MapConversion>,
    pub rigid_operation: Option<RigidOperation>,
    pub site: Option<SiteReference>,
    /// モデルの3Dコンテキストの`TrueNorth`（局所XY平面で真北を指す向き）。
    pub true_north: Option<[f64; 2]>,
    /// プロジェクトの長さ単位 [m]。
    pub length_unit_m: f64,
    /// プロジェクトの平面角の単位 [rad]。
    pub plane_angle_rad: f64,
}

/// `IfcMapConversion` / `IfcMapConversionScaled`。
#[derive(Clone, Debug, PartialEq)]
pub struct MapConversion {
    pub eastings: f64,
    pub northings: f64,
    pub orthogonal_height: f64,
    pub x_axis_abscissa: Option<f64>,
    pub x_axis_ordinate: Option<f64>,
    pub scale: Option<f64>,
    /// `IfcMapConversionScaled`の軸別係数。ない場合は1。
    pub factors: [f64; 3],
    pub target: Crs,
}

/// `IfcRigidOperation`（IFC4.3）。
#[derive(Clone, Debug, PartialEq)]
pub struct RigidOperation {
    pub first_coordinate: f64,
    pub second_coordinate: f64,
    pub height: f64,
    pub target: Crs,
}

/// 変換先のCRS。
#[derive(Clone, Debug, PartialEq)]
pub enum Crs {
    Projected { name: Option<String>, map_unit_m: Option<f64> },
    Geographic { name: Option<String>, angle_unit_rad: Option<f64>, height_unit_m: Option<f64> },
    Missing,
}

/// `IfcSite`の`RefLatitude` / `RefLongitude` / `RefElevation`。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SiteReference {
    pub latitude_deg: Option<f64>,
    pub longitude_deg: Option<f64>,
    /// プロジェクトの長さ単位のまま。
    pub elevation: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SiteCoords {
    /// 局所座標を、IfcSiteの経緯度を原点とする東・北・上とみなす。
    Enu,
    /// 局所座標を、IfcSiteの経緯度を投影した地図座標からのオフセットとみなす。
    Grid,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalePolicy {
    /// 実効倍率が100倍以上ずれ、逆数なら1になる場合は逆数を使う。
    Auto,
    /// 仕様どおりに使う。
    Spec,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GeorefOptions {
    pub crs_epsg: Option<u32>,
    pub site_coords: SiteCoords,
    /// 緯度・経度 [度]・正標高 [m]。
    pub origin: Option<[f64; 3]>,
    pub scale_policy: ScalePolicy,
    /// 局所原点の地図座標（東, 北, 正標高 [m]）と、局所X軸から東への回転（反時計回り）[度]。`--crs`が必要。
    pub map_conversion: Option<[f64; 4]>,
}

/// 局所座標（m）を地球上に置く方法。
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Placement {
    /// 局所座標→平面直角座標（東・北）と正標高。
    Grid(GridPlacement),
    /// 原点の東・北・上（ENU）。
    Enu(EnuPlacement),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GridPlacement {
    pub zone: JPRZone,
    pub epsg: u32,
    /// 局所原点の地図座標 [m] と正標高 [m]。
    pub origin: [f64; 3],
    /// 局所X軸から地図の東への回転（反時計回り）[rad]。
    pub rotation: f64,
    /// メートル化した局所座標に掛ける倍率。
    pub scale: f64,
    pub factors: [f64; 3],
}

impl GridPlacement {
    /// 局所座標 [m] → (東, 北, 正標高) [m]。
    pub fn to_map(&self, p: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.rotation.sin_cos();
        let x = self.scale * self.factors[0] * p[0];
        let y = self.scale * self.factors[1] * p[1];
        let z = self.scale * self.factors[2] * p[2];
        [self.origin[0] + x * c - y * s, self.origin[1] + x * s + y * c, self.origin[2] + z]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnuPlacement {
    pub latitude_deg: f64,
    pub longitude_deg: f64,
    pub orthometric_height: f64,
    /// 局所X軸から東への回転（反時計回り）[rad]。
    pub rotation: f64,
}

impl EnuPlacement {
    /// 局所座標 [m] → 原点のENU [m]。
    pub fn to_enu(&self, p: [f64; 3]) -> [f64; 3] {
        let (s, c) = self.rotation.sin_cos();
        [p[0] * c - p[1] * s, p[0] * s + p[1] * c, p[2]]
    }
}

/// 解決の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct Resolved {
    pub placement: Placement,
    /// 何を使って置いたか（レポート用）。
    pub source: &'static str,
    pub warnings: Vec<String>,
}

/// ENUで置いたとき、局所原点からこれ以上離れた形状があれば警告する [m]。
const FAR_FROM_ORIGIN_M: f64 = 1000.0;

/// ジオリファレンスを解決する。`reach_m`は形状の局所原点からの最大水平距離。
pub fn resolve(raw: &RawGeoref, opts: &GeorefOptions, reach_m: f64) -> Result<Resolved, String> {
    let mut warnings = Vec::new();
    let (placement, source) = if let Some([lat, lon, h]) = opts.origin {
        let p = EnuPlacement { latitude_deg: lat, longitude_deg: lon, orthometric_height: h, rotation: 0.0 };
        (Placement::Enu(p), "--origin")
    } else if let Some(given) = opts.map_conversion {
        (given_map_conversion(raw, given, opts, &mut warnings)?, "--map-conversion")
    } else if let Some(mc) = &raw.map_conversion {
        (map_conversion(raw, mc, opts, &mut warnings)?, "IfcMapConversion")
    } else if let Some(ro) = &raw.rigid_operation {
        (rigid_operation(raw, ro, opts, &mut warnings)?, "IfcRigidOperation")
    } else if let Some((lat, lon)) = site_lat_lon(raw.site.as_ref(), &mut warnings) {
        let elevation = raw.site.as_ref().and_then(|s| s.elevation).unwrap_or(0.0) * raw.length_unit_m;
        (site(raw, lat, lon, elevation, opts)?, "IfcSite")
    } else {
        return Err("ジオリファレンスがない（IfcMapConversion・IfcRigidOperation・IfcSiteの経緯度のいずれもない）。\
                    --origin LAT,LON[,H] で原点を指定すると、その点を中心に東・北・上で配置する"
            .into());
    };
    if matches!(placement, Placement::Enu(_)) && opts.origin.is_none() && reach_m > FAR_FROM_ORIGIN_M {
        warnings.push(format!(
            "形状が局所原点から最大{reach_m:.0} m離れている。局所座標が平面直角座標の値なら \
             --map-conversion 0,0 --crs EPSG:xxxx、IfcSiteの経緯度を投影した点からのオフセットなら \
             --site-coords grid --crs EPSG:xxxx を指定する"
        ));
    }
    Ok(Resolved { placement, source, warnings })
}

fn map_conversion(
    raw: &RawGeoref,
    mc: &MapConversion,
    opts: &GeorefOptions,
    warnings: &mut Vec<String>,
) -> Result<Placement, String> {
    let (name, map_unit_m) = match &mc.target {
        Crs::Projected { name, map_unit_m } => (name.as_deref(), map_unit_m.unwrap_or(raw.length_unit_m)),
        Crs::Geographic { .. } => {
            return Err("IfcMapConversionの変換先が地理座標系（IfcGeographicCRS）で、解釈できない".into());
        }
        Crs::Missing => (None, raw.length_unit_m),
    };
    let (zone, epsg) = zone_for(opts.crs_epsg, name, warnings)?;
    let spec_scale = mc.scale.unwrap_or(1.0) * map_unit_m / raw.length_unit_m;
    let scale = effective_scale(spec_scale, mc.scale, map_unit_m, raw.length_unit_m, opts.scale_policy, warnings);
    let abscissa = mc.x_axis_abscissa.unwrap_or(1.0);
    let ordinate = mc.x_axis_ordinate.unwrap_or(0.0);
    Ok(Placement::Grid(GridPlacement {
        zone,
        epsg,
        origin: [mc.eastings * map_unit_m, mc.northings * map_unit_m, mc.orthogonal_height * map_unit_m],
        rotation: ordinate.atan2(abscissa),
        scale,
        factors: mc.factors,
    }))
}

/// `--map-conversion`で与えた地図座標の基準。局所座標（m）をそのまま使い、倍率は1とする。
fn given_map_conversion(
    raw: &RawGeoref,
    [e, n, h, rotation_deg]: [f64; 4],
    opts: &GeorefOptions,
    warnings: &mut Vec<String>,
) -> Result<Placement, String> {
    let epsg = opts.crs_epsg.ok_or("--map-conversion には --crs EPSG:xxxx が必要")?;
    let (zone, epsg) = zone_for(Some(epsg), None, warnings)?;
    if raw.map_conversion.is_some() || raw.rigid_operation.is_some() {
        warnings.push("ファイルのIfcMapConversion・IfcRigidOperationは使わず、--map-conversionで置いた".into());
    }
    Ok(Placement::Grid(GridPlacement {
        zone,
        epsg,
        origin: [e, n, h],
        rotation: rotation_deg.to_radians(),
        scale: 1.0,
        factors: [1.0; 3],
    }))
}

/// `Scale`の実効倍率を決める。逆数で書かれたファイル（公式サンプルにもある）を検出する。
fn effective_scale(
    spec_scale: f64,
    scale: Option<f64>,
    map_unit_m: f64,
    length_unit_m: f64,
    policy: ScalePolicy,
    warnings: &mut Vec<String>,
) -> f64 {
    let inverse = scale.filter(|s| *s != 0.0).map(|s| map_unit_m / (s * length_unit_m));
    if policy == ScalePolicy::Auto
        && spec_scale.log10().abs() > 2.0
        && let Some(inv) = inverse
        && inv.log10().abs() < 0.01
    {
        warnings.push(format!(
            "IfcMapConversion.Scaleの実効倍率が{spec_scale}で、逆数で書かれているとみなし{inv}を使う\
             （仕様どおりにするには --scale-policy spec）"
        ));
        return inv;
    }
    if (spec_scale - 1.0).abs() > 0.01 {
        warnings.push(format!("IfcMapConversion.Scaleの実効倍率が{spec_scale}で、1から1%以上ずれている"));
    }
    spec_scale
}

fn rigid_operation(
    raw: &RawGeoref,
    ro: &RigidOperation,
    opts: &GeorefOptions,
    warnings: &mut Vec<String>,
) -> Result<Placement, String> {
    match &ro.target {
        Crs::Geographic { angle_unit_rad, height_unit_m, .. } => {
            let to_deg = angle_unit_rad.unwrap_or(raw.plane_angle_rad).to_degrees();
            let height = ro.height * height_unit_m.unwrap_or(raw.length_unit_m);
            Ok(Placement::Enu(EnuPlacement {
                longitude_deg: ro.first_coordinate * to_deg,
                latitude_deg: ro.second_coordinate * to_deg,
                orthometric_height: height,
                rotation: 0.0,
            }))
        }
        Crs::Projected { name, map_unit_m } => {
            let unit = map_unit_m.unwrap_or(raw.length_unit_m);
            let (zone, epsg) = zone_for(opts.crs_epsg, name.as_deref(), warnings)?;
            Ok(Placement::Grid(GridPlacement {
                zone,
                epsg,
                origin: [ro.first_coordinate * unit, ro.second_coordinate * unit, ro.height * unit],
                rotation: 0.0,
                scale: 1.0,
                factors: [1.0; 3],
            }))
        }
        Crs::Missing => Err("IfcRigidOperationの変換先CRSがない".into()),
    }
}

fn site_lat_lon(site: Option<&SiteReference>, warnings: &mut Vec<String>) -> Option<(f64, f64)> {
    let s = site?;
    let (lat, lon) = (s.latitude_deg?, s.longitude_deg?);
    if lat == 0.0 && lon == 0.0 {
        warnings.push("IfcSiteの経緯度が(0, 0)のため、未設定とみなした".into());
        return None;
    }
    if let Some(what) = known_default(lat, lon) {
        warnings.push(format!(
            "IfcSiteの経緯度（{lat:.6}, {lon:.6}）は{what}と一致し、実際の位置ではない可能性が高い。\
             --origin LAT,LON[,H] か --map-conversion E,N --crs EPSG:xxxx で置き直す"
        ));
    }
    Some((lat, lon))
}

fn site(raw: &RawGeoref, lat: f64, lon: f64, elevation: f64, opts: &GeorefOptions) -> Result<Placement, String> {
    match opts.site_coords {
        SiteCoords::Enu => {
            // TrueNorthは局所XY平面で北を指す。局所X軸から東への角度は 90° − atan2(ty, tx)。
            let rotation = raw.true_north.map_or(0.0, |[tx, ty]| std::f64::consts::FRAC_PI_2 - ty.atan2(tx));
            Ok(Placement::Enu(EnuPlacement {
                latitude_deg: lat,
                longitude_deg: lon,
                orthometric_height: elevation,
                rotation,
            }))
        }
        SiteCoords::Grid => {
            let epsg = opts.crs_epsg.ok_or("--site-coords grid には --crs EPSG:xxxx が必要")?;
            let (zone, epsg) = zone_for(Some(epsg), None, &mut Vec::new())?;
            let (e, n, _) = zone.projection().project_forward(lon, lat, 0.0).map_err(|e| format!("{e:?}"))?;
            Ok(Placement::Grid(GridPlacement {
                zone,
                epsg,
                origin: [e, n, elevation],
                rotation: 0.0,
                scale: 1.0,
                factors: [1.0; 3],
            }))
        }
    }
}

/// オーサリングツールの既定値とみられるIfcSiteの経緯度 [度]。無関係な複数のファイルで同じ値を確認したもの
/// （度分秒と百万分の1秒で書かれた値を度に直した）。
const KNOWN_DEFAULTS: [(f64, f64, &str); 2] = [
    // (42,24,53,508911), (-71,-15,-29,-58837)
    (42.414_863_586_4, -71.258_071_899_2, "Revitの既定の場所（米国マサチューセッツ州）"),
    // (35,41,6,4943), (139,45,3,625488)
    (35.685_001_373_1, 139.751_007_080_0, "Revitの都市リストの東京"),
];

fn known_default(lat: f64, lon: f64) -> Option<&'static str> {
    KNOWN_DEFAULTS.iter().find(|(a, b, _)| (lat - a).abs() < 1e-6 && (lon - b).abs() < 1e-6).map(|&(_, _, w)| w)
}

/// CRS名（`EPSG:6677`など）から平面直角座標系の系を決める。`--crs`があればそれを優先する。
fn zone_for(
    override_epsg: Option<u32>,
    name: Option<&str>,
    warnings: &mut Vec<String>,
) -> Result<(JPRZone, u32), String> {
    let epsg = match override_epsg {
        Some(e) => e,
        None => name
            .and_then(parse_epsg)
            .ok_or_else(|| format!("CRSが分からない（{}）。--crs EPSG:xxxx で指定する", name.unwrap_or("名前なし")))?,
    };
    if (30161..=30179).contains(&epsg) {
        return Err(format!("EPSG:{epsg}（旧日本測地系）は対応外"));
    }
    let zone = u16::try_from(epsg).ok().and_then(JPRZone::from_epsg).ok_or_else(|| {
        format!(
            "EPSG:{epsg}は対応外（日本の平面直角座標系のみ）。--origin LAT,LON[,H] で原点を指定すると、\
             その点を中心に東・北・上で配置する"
        )
    })?;
    if (2443..=2461).contains(&epsg) {
        warnings.push(format!("EPSG:{epsg}（JGD2000）をJGD2011と同じとみなした"));
    }
    Ok((zone, epsg))
}

/// `EPSG:6677`、`EPSG: 6677`、`urn:ogc:def:crs:EPSG::6677` などからコードを取り出す。
pub fn parse_epsg(s: &str) -> Option<u32> {
    let upper = s.to_ascii_uppercase();
    let rest = &upper[upper.find("EPSG")? + 4..];
    let digits: String = rest.trim_start_matches([':', ' ']).chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw_mm() -> RawGeoref {
        RawGeoref { length_unit_m: 0.001, plane_angle_rad: 1.0, ..Default::default() }
    }

    fn opts() -> GeorefOptions {
        GeorefOptions {
            crs_epsg: None,
            site_coords: SiteCoords::Enu,
            origin: None,
            scale_policy: ScalePolicy::Auto,
            map_conversion: None,
        }
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
        match r.placement {
            Placement::Grid(g) => g,
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
        assert_eq!(g.zone, JPRZone::Zone9);
        assert!((g.scale - 1.0).abs() < 1e-12);
        assert_eq!(g.to_map([0.0, 0.0, 0.0]), [-5992.9196, -35363.2377, 3.0]);
        assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    }

    #[test]
    fn omitted_map_unit_means_project_unit() {
        // MapUnitなし＝プロジェクト単位（mm）。Eastingsもmmで書かれている
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
    fn inverted_scale_is_detected_in_auto_policy_only() {
        // 仕様どおりならScale=0.001だが、1000と書かれている
        let raw = RawGeoref { map_conversion: Some(mc(1000.0, Some(1.0))), ..raw_mm() };
        let r = resolve(&raw, &opts(), 0.0).unwrap();
        assert!((grid(&r).scale - 1.0).abs() < 1e-12);
        assert_eq!(r.warnings.len(), 1);
        let spec = GeorefOptions { scale_policy: ScalePolicy::Spec, ..opts() };
        let r = resolve(&raw, &spec, 0.0).unwrap();
        assert!((grid(&r).scale - 1e6).abs() < 1e-3);
    }

    #[test]
    fn rotation_and_axis_factors() {
        let mut m = mc(0.001, Some(1.0));
        m.x_axis_abscissa = Some(0.0);
        m.x_axis_ordinate = Some(2.0); // 正規化されていなくてもよい。90°
        m.factors = [2.0, 1.0, 1.0];
        let raw = RawGeoref { map_conversion: Some(m), ..raw_mm() };
        let g = grid(&resolve(&raw, &opts(), 0.0).unwrap());
        let p = g.to_map([1.0, 0.0, 0.0]);
        assert!((p[0] + 5992.9196).abs() < 1e-9 && (p[1] + 35363.2377 - 2.0).abs() < 1e-9);
    }

    #[test]
    fn unsupported_and_tokyo_crs_are_rejected() {
        let mut m = mc(0.001, Some(1.0));
        m.target = Crs::Projected { name: Some("EPSG:32760".into()), map_unit_m: Some(1.0) };
        let raw = RawGeoref { map_conversion: Some(m), ..raw_mm() };
        assert!(resolve(&raw, &opts(), 0.0).unwrap_err().contains("--origin"));
        let o = GeorefOptions { crs_epsg: Some(30169), ..opts() };
        assert!(resolve(&raw, &o, 0.0).is_err());
        // --crs でTargetCRSを上書きできる
        let o = GeorefOptions { crs_epsg: Some(6677), ..opts() };
        assert!(resolve(&raw, &o, 0.0).is_ok());
    }

    #[test]
    fn jgd2000_is_accepted_with_warning() {
        let o = GeorefOptions { crs_epsg: Some(2451), ..opts() };
        let raw = RawGeoref { map_conversion: Some(mc(0.001, Some(1.0))), ..raw_mm() };
        let r = resolve(&raw, &o, 0.0).unwrap();
        assert_eq!(grid(&r).zone, JPRZone::Zone9);
        assert!(r.warnings[0].contains("JGD2000"));
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
            true_north: Some([1.0, 0.0]), // 局所+Xが北
            ..raw_mm()
        };
        let r = resolve(&raw, &opts(), 0.0).unwrap();
        let Placement::Enu(p) = r.placement else { panic!() };
        assert_eq!(p.orthometric_height, 3.0);
        let enu = p.to_enu([1.0, 0.0, 0.0]);
        assert!(enu[0].abs() < 1e-12 && (enu[1] - 1.0).abs() < 1e-12, "{enu:?}");
    }

    #[test]
    fn site_grid_projects_site_and_adds_offsets() {
        let raw = RawGeoref {
            site: Some(SiteReference {
                latitude_deg: Some(36.0),
                longitude_deg: Some(139.0 + 50.0 / 60.0),
                elevation: None,
            }),
            ..raw_mm()
        };
        let o = GeorefOptions { site_coords: SiteCoords::Grid, crs_epsg: Some(6677), ..opts() };
        let g = grid(&resolve(&raw, &o, 0.0).unwrap());
        let p = g.to_map([10.0, 20.0, 3.0]);
        assert!((p[0] - 10.0).abs() < 1e-6 && (p[1] - 20.0).abs() < 1e-6 && p[2] == 3.0, "{p:?}");
        let no_crs = GeorefOptions { site_coords: SiteCoords::Grid, ..opts() };
        assert!(resolve(&raw, &no_crs, 0.0).is_err());
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
    fn rigid_operation_to_geographic_crs() {
        let raw = RawGeoref {
            rigid_operation: Some(RigidOperation {
                first_coordinate: 14.0902217,
                second_coordinate: 46.3623297,
                height: 475.0,
                target: Crs::Geographic {
                    name: Some("EPSG:4258".into()),
                    angle_unit_rad: Some(1f64.to_radians()),
                    height_unit_m: Some(1.0),
                },
            }),
            ..raw_mm()
        };
        let Placement::Enu(p) = resolve(&raw, &opts(), 0.0).unwrap().placement else { panic!() };
        assert!((p.longitude_deg - 14.0902217).abs() < 1e-12);
        assert!((p.latitude_deg - 46.3623297).abs() < 1e-12);
        assert_eq!(p.orthometric_height, 475.0);
    }

    #[test]
    fn origin_option_wins() {
        let raw = RawGeoref { map_conversion: Some(mc(0.001, Some(1.0))), ..raw_mm() };
        let o = GeorefOptions { origin: Some([35.0, 139.0, 10.0]), ..opts() };
        let r = resolve(&raw, &o, 1e6).unwrap();
        assert_eq!(r.source, "--origin");
        assert!(r.warnings.is_empty());
    }

    #[test]
    fn given_map_conversion_needs_crs_and_wins_over_the_file() {
        let raw = RawGeoref { map_conversion: Some(mc(0.001, Some(1.0))), ..raw_mm() };
        let no_crs = GeorefOptions { map_conversion: Some([0.0, 0.0, 0.0, 0.0]), ..opts() };
        assert!(resolve(&raw, &no_crs, 0.0).unwrap_err().contains("--crs"));
        let o = GeorefOptions { crs_epsg: Some(6677), map_conversion: Some([100.0, 200.0, 3.0, 90.0]), ..opts() };
        let r = resolve(&raw, &o, 0.0).unwrap();
        assert_eq!(r.source, "--map-conversion");
        assert_eq!(r.warnings.len(), 1);
        let g = grid(&r);
        assert_eq!((g.epsg, g.scale), (6677, 1.0));
        // 局所X軸を東から90°回すと、局所の(1, 0)は北へ1 m
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
        assert!(boston.warnings.iter().any(|w| w.contains("Revitの既定の場所")), "{:?}", boston.warnings);
        let tokyo = resolve(&site(35.685_001_4, 139.751_007_1), &opts(), 0.0).unwrap();
        assert!(tokyo.warnings.iter().any(|w| w.contains("東京")), "{:?}", tokyo.warnings);
        assert!(resolve(&site(35.681_236, 139.767_125), &opts(), 0.0).unwrap().warnings.is_empty());
    }
}
