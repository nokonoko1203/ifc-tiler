//! 自作IFC（testdata/handmade）を変換し、出力を読み戻して確かめる（受け入れ条件A1・A2・A4）。
//!
//! 既定の出力（量子化＋meshopt）をmeshoptで復号し、既知点の位置と部材の属性を見る。
//! 期待値は testdata/handmade/expected.json（国土地理院の計算結果）。

mod common;

use std::path::PathBuf;

use common::{Feature, options, out_dir, read_tileset, root};
use ifc_tiler::geodesy::GeoidModel;
use ifc_tiler::georef::SiteCoords;
use ifc_tiler::{Options, convert};
use serde_json::Value;

const CUBE: &str = "既知点立方体";

fn run(file: &str, name: &str, opts: &Options) -> PathBuf {
    let out = out_dir(&format!("handmade/{name}"));
    convert(&root().join("testdata/handmade").join(file), &out, opts).expect("変換できる");
    out
}

/// 国土地理院の既知点（GRS80、楕円体高 = 正標高3 m + ジオイド高）のECEF。
fn known_point(geoid_key: &str) -> [f64; 3] {
    let exp: Value =
        serde_json::from_slice(&std::fs::read(root().join("testdata/handmade/expected.json")).unwrap()).unwrap();
    let k = &exp["known_point"];
    let (lat, lon) = (k["latitude"].as_f64().unwrap().to_radians(), k["longitude"].as_f64().unwrap().to_radians());
    let h = k["orthometric_height_m"].as_f64().unwrap() + k[geoid_key].as_f64().unwrap();
    let (a, f) = (6_378_137.0, 1.0 / 298.257_222_101);
    let e2 = f * (2.0 - f);
    let n = a / (1.0 - e2 * lat.sin().powi(2)).sqrt();
    [(n + h) * lat.cos() * lon.cos(), (n + h) * lat.cos() * lon.sin(), (n * (1.0 - e2) + h) * lat.sin()]
}

/// 立方体の頂点のうち既知点に最も近いもの（局所原点の角）までの距離 [m]。
fn cube_error(features: &[Feature], geoid_key: &str) -> f64 {
    let kp = known_point(geoid_key);
    let cube = features
        .iter()
        .find(|f| f.props.get("name").and_then(Value::as_str) == Some(CUBE))
        .expect("既知点立方体がある");
    cube.ecef.iter().map(|p| (0..3).map(|i| (p[i] - kp[i]).powi(2)).sum::<f64>().sqrt()).fold(f64::MAX, f64::min)
}

#[test]
fn map_conversion_known_point() {
    let f = read_tileset(&run("ifc4_map_conversion.ifc", "mapconv", &options()));
    let e = cube_error(&f, "geoid_height_jpgeo2024_m");
    assert!(e < 0.01, "{e}");
}

#[test]
fn wrong_geoid_model_is_detectable() {
    let opts = Options { geoid: GeoidModel::Gsigeo2011, ..options() };
    let f = read_tileset(&run("ifc4_map_conversion.ifc", "mapconv_gsigeo2011", &opts));
    // 期待値（JPGEO2024）とは約0.1 mずれ、GSIGEO2011の期待値とは一致する
    assert!((cube_error(&f, "geoid_height_jpgeo2024_m") - 0.0989).abs() < 0.01);
    assert!(cube_error(&f, "geoid_height_gsigeo2011_m") < 0.01);
}

#[test]
fn site_lat_lon_known_point() {
    let f = read_tileset(&run("ifc2x3_site_latlon.ifc", "site_latlon", &options()));
    assert!(cube_error(&f, "geoid_height_jpgeo2024_m") < 0.01);
}

#[test]
fn plateau_grid_known_point_and_enu_mistake() {
    let mut grid = options();
    grid.georef.site_coords = SiteCoords::Grid;
    grid.georef.crs_epsg = Some(6677);
    let f = read_tileset(&run("ifc2x3_plateau_origin.ifc", "plateau_grid", &grid));
    assert!(cube_error(&f, "geoid_height_jpgeo2024_m") < 0.01);

    let out = out_dir("handmade/plateau_enu");
    let r = convert(&root().join("testdata/handmade/ifc2x3_plateau_origin.ifc"), &out, &options()).unwrap();
    assert_eq!(r.warnings.len(), 1, "{:?}", r.warnings);
    assert!(cube_error(&read_tileset(&out), "geoid_height_jpgeo2024_m") > 100.0);
}

/// 受け入れ条件A11: 局所座標が平面直角座標の値のIFCを、`--map-conversion 0,0 --crs`だけで置ける。
#[test]
fn map_conversion_option_places_plan_coordinates() {
    let mut opts = options();
    opts.georef.map_conversion = Some([0.0, 0.0, 0.0, 0.0]);
    opts.georef.crs_epsg = Some(6677);
    let f = read_tileset(&run("ifc2x3_plateau_origin.ifc", "plateau_map_conversion", &opts));
    assert!(cube_error(&f, "geoid_height_jpgeo2024_m") < 0.01);

    opts.georef.crs_epsg = None;
    let out = out_dir("handmade/map_conversion_no_crs");
    let e = convert(&root().join("testdata/handmade/ifc2x3_plateau_origin.ifc"), &out, &opts).unwrap_err();
    assert!(matches!(e, ifc_tiler::Error::Input(_)), "{e}");
}

/// `--map-conversion`は`--origin`・`--site-coords grid`と同時に指定できない（終了コード2）。
#[test]
fn map_conversion_conflicts_are_input_errors() {
    let input = root().join("testdata/handmade/ifc2x3_plateau_origin.ifc");
    for extra in [&["--origin", "35,139"][..], &["--site-coords", "grid"][..]] {
        let out = out_dir("handmade/map_conversion_conflict");
        let status = std::process::Command::new(env!("CARGO_BIN_EXE_ifc_tiler"))
            .arg(&input)
            .arg("-o")
            .arg(&out)
            .args(["--map-conversion", "0,0", "--crs", "EPSG:6677"])
            .args(extra)
            .output()
            .unwrap()
            .status;
        assert_eq!(status.code(), Some(2), "{extra:?}");
    }
}

#[test]
fn element_semantics() {
    let features = read_tileset(&run("ifc4_map_conversion.ifc", "semantics", &options()));
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
