//! ジオリファレンスの解決。
//!
//! IFCから読んだ生の値（`RawGeoref`）と利用者の指定（`GeorefOptions`）から、
//! 局所座標（IFCの世界座標、m）を地球上に置く方法（`Placement`）を1つ決める。
//! 優先順位は `--origin` → `--map-conversion` → `IfcMapConversion` → `IfcSite`の経緯度。
//! CRSは文字列のまま持ち、解釈と変換は`geodesy`（PROJ）で行う。

/// IFCから読んだジオリファレンスの生データ。長さ・角度の値はファイルに書かれた単位のまま。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RawGeoref {
    pub map_conversion: Option<MapConversion>,
    pub site: Option<SiteReference>,
    /// モデルの3Dコンテキストの`TrueNorth`（局所XY平面で真北を指す向き）。
    pub true_north: Option<[f64; 2]>,
    /// プロジェクトの長さ単位 [m]。
    pub length_unit_m: f64,
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

/// 変換先のCRS（`IfcProjectedCRS`）。投影CRS以外は`Missing`として読む。
#[derive(Clone, Debug, PartialEq)]
pub enum Crs {
    Projected { name: Option<String>, map_unit_m: Option<f64> },
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

/// 利用者の指定。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GeorefOptions {
    /// PROJが解釈できるCRS（`EPSG:6677`、`EPSG:6677+6695`など）。地図経路では`IfcMapConversion`の
    /// TargetCRSを上書きし、ENU経路では原点の緯度・経度・高さのCRSになる。
    pub crs: Option<String>,
    /// 緯度・経度 [度]・正標高 [m]。
    pub origin: Option<[f64; 3]>,
    /// 局所原点の地図座標（東, 北, 正標高 [m]）と、局所X軸から東への回転（反時計回り）[度]。`--crs`が必要。
    pub map_conversion: Option<[f64; 4]>,
}

/// ENU経路で`--crs`がないときの、原点の緯度・経度のCRS（WGS84）。
const DEFAULT_GEOGRAPHIC_CRS: &str = "EPSG:4326";

/// 局所座標（m）を地球上に置く方法。
#[derive(Clone, Debug, PartialEq)]
pub enum Placement {
    /// 局所座標→地図座標（東・北）と標高。
    Grid(GridPlacement),
    /// 原点の東・北・高さ（ENU）。
    Enu(EnuPlacement),
}

#[derive(Clone, Debug, PartialEq)]
pub struct GridPlacement {
    /// 地図座標のCRS（投影座標系。高さの基準を含んでもよい）。
    pub crs: String,
    /// 局所原点の地図座標 [m] と標高 [m]。
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

#[derive(Clone, Debug, PartialEq)]
pub struct EnuPlacement {
    /// 原点の緯度・経度・高さのCRS（地理座標系。高さの基準を含んでもよい）。
    pub crs: String,
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
    pub warnings: Vec<String>,
}

/// ENUで置いたとき、局所原点からこれ以上離れた形状があれば警告する [m]。
const FAR_FROM_ORIGIN_M: f64 = 1000.0;

/// ジオリファレンスを解決する。`reach_m`は形状の局所原点からの最大水平距離。
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
        return Err("ジオリファレンスがない（IfcMapConversion・IfcSiteの経緯度のどちらもない）。\
                    --origin LAT,LON[,H] で原点を指定すると、その点を中心に東・北・高さで配置する"
            .into());
    };
    if matches!(placement, Placement::Enu(_)) && opts.origin.is_none() && reach_m > FAR_FROM_ORIGIN_M {
        warnings.push(format!(
            "形状が局所原点から最大{reach_m:.0} m離れている。局所座標が地図座標の値なら \
             --map-conversion 0,0 --crs EPSG:xxxx を指定する"
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

/// `--map-conversion`で与えた地図座標の基準。局所座標（m）をそのまま使い、倍率は1とする。
fn given_map_conversion(
    raw: &RawGeoref,
    [e, n, h, rotation_deg]: [f64; 4],
    opts: &GeorefOptions,
    warnings: &mut Vec<String>,
) -> Result<Placement, String> {
    let crs = opts.crs.clone().ok_or("--map-conversion には --crs EPSG:xxxx が必要")?;
    if raw.map_conversion.is_some() {
        warnings.push("ファイルのIfcMapConversionは使わず、--map-conversionで置いた".into());
    }
    Ok(Placement::Grid(GridPlacement {
        crs,
        origin: [e, n, h],
        rotation: rotation_deg.to_radians(),
        scale: 1.0,
        factors: [1.0; 3],
    }))
}

/// `Scale`の実効倍率を決める。実効倍率が100倍以上ずれ、逆数なら1になる場合は、逆数で書かれている
/// （公式サンプルにもある）とみなして逆数を使う。
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
            .push(format!("IfcMapConversion.Scaleの実効倍率が{spec_scale}で、逆数で書かれているとみなし{inv}を使う"));
        return inv;
    }
    if (spec_scale - 1.0).abs() > 0.01 {
        warnings.push(format!("IfcMapConversion.Scaleの実効倍率が{spec_scale}で、1から1%以上ずれている"));
    }
    spec_scale
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

/// IfcSiteの経緯度を原点とする東・北・高さ。TrueNorthは局所XY平面で北を指すので、
/// 局所X軸から東への角度は 90° − atan2(ty, tx)。
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

/// 地図座標のCRSを決める。`--crs`があればそれを優先し、なければIFCのCRS名からEPSGコードを読む。
fn crs_for(override_crs: Option<&str>, name: Option<&str>) -> Result<String, String> {
    if let Some(c) = override_crs {
        return Ok(c.to_string());
    }
    name.and_then(parse_epsg)
        .map(|code| format!("EPSG:{code}"))
        .ok_or_else(|| format!("CRSが分からない（{}）。--crs EPSG:xxxx で指定する", name.unwrap_or("名前なし")))
}

/// `EPSG:6677`、`EPSG: 6677`、`urn:ogc:def:crs:EPSG::6677` などからコードを取り出す。
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
    fn inverted_scale_is_detected() {
        // 仕様どおりならScale=0.001だが、1000と書かれている
        let raw = RawGeoref { map_conversion: Some(mc(1000.0, Some(1.0))), ..raw_mm() };
        let r = resolve(&raw, &opts(), 0.0).unwrap();
        assert!((grid(&r).scale - 1.0).abs() < 1e-12);
        assert_eq!(r.warnings.len(), 1);
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
