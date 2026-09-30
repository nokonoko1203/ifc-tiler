//! 自作IFC（testdata/handmade）を変換し、出力を読み戻して確かめる。
//!
//! 出力（量子化＋meshopt）を復号し、既知点の位置と部材の属性を見る。
//! 期待値は testdata/handmade/expected.json（国土地理院の計算結果と、日本以外はGeographicLibの計算例）。
//! 座標変換はPROJが行い、グリッド（ジオイドなど）はcdn.proj.orgから取得する（初回はネットワークが要る）。

mod common;

use std::path::PathBuf;

use common::{Feature, out_dir, read_tileset, root};
use ifc_tiler::GeorefOptions;
use ifc_tiler::convert;
use serde_json::Value;

const CUBE: &str = "既知点立方体";
/// JGD2011 / 平面直角座標系IX系 + JGD2011の標高。
const PLANE_IX_WITH_HEIGHT: &str = "EPSG:10170";
/// JGD2011の経緯度 + JGD2011の標高。
const JGD2011_WITH_HEIGHT: &str = "EPSG:6697";

fn with_crs(crs: &str) -> GeorefOptions {
    GeorefOptions { crs: Some(crs.into()), ..Default::default() }
}

fn expected() -> Value {
    serde_json::from_slice(&std::fs::read(root().join("testdata/handmade/expected.json")).unwrap()).unwrap()
}

/// WGS84の(緯度, 経度, 楕円体高) → ECEF。
fn ecef(lat_deg: f64, lon_deg: f64, h: f64) -> [f64; 3] {
    let (lat, lon) = (lat_deg.to_radians(), lon_deg.to_radians());
    let (a, f) = (6_378_137.0, 1.0 / 298.257_223_563);
    let e2 = f * (2.0 - f);
    let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    [(n + h) * lat.cos() * lon.cos(), (n + h) * lat.cos() * lon.sin(), (n * (1.0 - e2) + h) * lat.sin()]
}

/// ECEF → WGS84の楕円体高。緯度を反復で求める。
fn ellipsoidal_height(p: [f64; 3]) -> f64 {
    let (a, f) = (6_378_137.0, 1.0 / 298.257_223_563);
    let e2 = f * (2.0 - f);
    let r = p[0].hypot(p[1]);
    let mut lat = p[2].atan2(r * (1.0 - e2));
    let mut h = 0.0;
    for _ in 0..10 {
        let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
        h = r / lat.cos() - n;
        lat = p[2].atan2(r * (1.0 - e2 * n / (n + h)));
    }
    h
}

fn cube(features: &[Feature]) -> &Feature {
    features.iter().find(|f| f.props.get("name").and_then(Value::as_str) == Some(CUBE)).expect("既知点立方体がある")
}

fn nearest(features: &[Feature], target: [f64; 3]) -> f64 {
    cube(features)
        .ecef
        .iter()
        .map(|p| (0..3).map(|i| (p[i] - target[i]).powi(2)).sum::<f64>().sqrt())
        .fold(f64::MAX, f64::min)
}

fn run(file: &str, name: &str, opts: &GeorefOptions) -> PathBuf {
    let out = out_dir(&format!("handmade/{name}"));
    convert(&root().join("testdata/handmade").join(file), &out, opts).expect("変換できる");
    out
}

/// 国土地理院の既知点（楕円体高 = 標高3 m + ジオイド高）のECEF。
fn known_point(geoid_key: &str) -> [f64; 3] {
    let exp = expected();
    let k = &exp["known_point"];
    let h = k["orthometric_height_m"].as_f64().unwrap() + k[geoid_key].as_f64().unwrap();
    ecef(k["latitude"].as_f64().unwrap(), k["longitude"].as_f64().unwrap(), h)
}

/// 立方体の頂点のうち既知点に最も近いもの（局所原点の角）までの距離 [m]。
fn cube_error(features: &[Feature], geoid_key: &str) -> f64 {
    nearest(features, known_point(geoid_key))
}

#[test]
fn map_conversion_known_point() {
    let f = read_tileset(&run("ifc4_map_conversion.ifc", "mapconv", &with_crs(PLANE_IX_WITH_HEIGHT)));
    let e = cube_error(&f, "geoid_height_jpgeo2024_m");
    assert!(e < 0.01, "{e}");
}

#[test]
fn site_lat_lon_known_point() {
    let f = read_tileset(&run("ifc2x3_site_latlon.ifc", "site_latlon", &with_crs(JGD2011_WITH_HEIGHT)));
    assert!(cube_error(&f, "geoid_height_jpgeo2024_m") < 0.01);
}

/// 局所座標が平面直角座標の値のIFCを、IfcSiteの経緯度を原点とするENUで置くと大きくずれ、警告が出る。
#[test]
fn plan_coordinates_in_enu_are_warned() {
    let out = out_dir("handmade/plateau_enu");
    let r =
        convert(&root().join("testdata/handmade/ifc2x3_plateau_origin.ifc"), &out, &GeorefOptions::default()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("離れている")), "{:?}", r.warnings);
    assert!(cube_error(&read_tileset(&out), "geoid_height_jpgeo2024_m") > 100.0);
}

/// 局所座標が平面直角座標の値のIFCを、`--map-conversion 0,0 --crs`だけで置ける。
#[test]
fn map_conversion_option_places_plan_coordinates() {
    let mut opts = GeorefOptions { map_conversion: Some([0.0, 0.0, 0.0, 0.0]), ..with_crs(PLANE_IX_WITH_HEIGHT) };
    let f = read_tileset(&run("ifc2x3_plateau_origin.ifc", "plateau_map_conversion", &opts));
    assert!(cube_error(&f, "geoid_height_jpgeo2024_m") < 0.01);

    opts.crs = None;
    let out = out_dir("handmade/map_conversion_no_crs");
    let e = convert(&root().join("testdata/handmade/ifc2x3_plateau_origin.ifc"), &out, &opts).unwrap_err();
    assert!(matches!(e, ifc_tiler::Error::Input(_)), "{e}");
}

/// 日本以外: UTM 38Nの地図座標に置いた立方体の水平位置が、GeographicLibの計算例と一致する。
#[test]
fn utm_map_conversion_matches_geographiclib() {
    let exp = expected();
    let u = &exp["outside_japan"]["utm_point"];
    let (e, n) = (u["easting_m"].as_f64().unwrap(), u["northing_m"].as_f64().unwrap());
    let opts = GeorefOptions { map_conversion: Some([e, n, 0.0, 0.0]), ..with_crs("EPSG:32638") };
    let f = read_tileset(&run("ifc4_map_conversion.ifc", "utm38n", &opts));
    // 高さはEGM2008で決まるので、各頂点の楕円体高で既知点を作り、水平の距離だけを見る
    let (lat, lon) = (u["latitude"].as_f64().unwrap(), u["longitude"].as_f64().unwrap());
    let err = cube(&f)
        .ecef
        .iter()
        .map(|&p| {
            let h = ellipsoidal_height(p);
            let q = ecef(lat, lon, h);
            (0..3).map(|i| (p[i] - q[i]).powi(2)).sum::<f64>().sqrt()
        })
        .fold(f64::MAX, f64::min);
    assert!(err < u["rounding_m"].as_f64().unwrap(), "{err}");
}

/// 日本以外: 高さの基準がなければEGM2008の標高とみなし、ジオイド高がGeographicLibのテストデータと一致する。
#[test]
fn egm2008_height_matches_geographiclib() {
    let exp = expected();
    let g = &exp["outside_japan"]["egm2008_point"];
    let (lat, lon) = (g["latitude"].as_f64().unwrap(), g["longitude"].as_f64().unwrap());
    let opts = GeorefOptions { origin: Some([lat, lon, 3.0]), ..Default::default() };
    let out = out_dir("handmade/egm2008");
    let r = convert(&root().join("testdata/handmade/ifc2x3_site_latlon.ifc"), &out, &opts).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("EGM2008")), "{:?}", r.warnings);
    // PROJはEGM2008の2.5′格子を補間する（GeographicLibによる最大誤差0.135 m、RMS 3 mm）
    let err = nearest(&read_tileset(&out), ecef(lat, lon, 3.0 + g["geoid_height_egm2008_m"].as_f64().unwrap()));
    assert!(err < 0.05, "{err}");
}

/// `--map-conversion`は`--origin`と同時に指定できない（終了コード2）。
#[test]
fn map_conversion_conflicts_with_origin() {
    let input = root().join("testdata/handmade/ifc2x3_plateau_origin.ifc");
    let out = out_dir("handmade/map_conversion_conflict");
    let status = std::process::Command::new(env!("CARGO_BIN_EXE_ifc_tiler"))
        .arg(&input)
        .arg("-o")
        .arg(&out)
        .args(["--map-conversion", "0,0", "--crs", "EPSG:6677", "--origin", "35,139"])
        .output()
        .unwrap()
        .status;
    assert_eq!(status.code(), Some(2));
}

#[test]
fn element_semantics() {
    let features = read_tileset(&run("ifc4_map_conversion.ifc", "semantics", &GeorefOptions::default()));
    let by_name = |n: &str| {
        features
            .iter()
            .find(|f| f.props.get("name").and_then(Value::as_str) == Some(n))
            .unwrap_or_else(|| panic!("{n}"))
    };
    let get = |f: &Feature, k: &str| f.props.get(k).cloned().unwrap_or(Value::Null);

    let wall = by_name("外壁A");
    // 型のPsetを継承し（IsExternal、Reference）、部材側で上書きした値（FireRating）が勝つ
    assert_eq!(get(wall, "Pset_WallCommon__IsExternal"), "TRUE");
    assert_eq!(get(wall, "Pset_WallCommon__Reference"), "WT-1");
    assert_eq!(get(wall, "Pset_WallCommon__FireRating"), "OCC-120");
    assert_eq!(get(wall, "Pset_WallCommon__LoadBearing"), "FALSE");
    assert_eq!(get(wall, "Qto_WallBaseQuantities__NetSideArea"), 16.0);
    assert_eq!(get(wall, "typeName"), "RC壁200");
    assert_eq!(get(wall, "storeyName"), "1階");
    assert_eq!(get(wall, "tag"), "W-01");
    assert_eq!(get(wall, "predefinedType"), "STANDARD");

    // 日本語のPset名はハッシュのIDになり、元の名前はclassの`name`に残る
    let concrete = wall.props.iter().find(|(_, v)| v.as_f64() == Some(24.0)).map(|(k, _)| k.clone()).unwrap();
    assert!(concrete.starts_with("p_"), "{concrete}");

    // 所属は包含（1階）で決まり、参照（2階）ではない
    assert_eq!(get(by_name("通し柱"), "storeyName"), "1階");

    // 開口と室はfeatureにならない。机2台はそれぞれfeatureになる
    let classes: Vec<&str> = features.iter().filter_map(|f| f.props["ifcClass"].as_str()).collect();
    assert!(!classes.contains(&"IfcOpeningElement") && !classes.contains(&"IfcSpace"), "{classes:?}");
    assert_eq!(classes.iter().filter(|c| **c == "IfcFurniture").count(), 2);
    assert_eq!(get(by_name("机2"), "storeyName"), "2階");
    // 机は同じ形の複製（IfcMappedItem）だが、別の位置にある
    let (a, b) = (&by_name("机1").ecef, &by_name("机2").ecef);
    assert!(!a.is_empty() && !b.is_empty() && (a[0][0] - b[0][0]).abs() + (a[0][1] - b[0][1]).abs() > 0.1);
}
