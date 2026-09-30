//! 実務規模のモデル（BURKWIL_DX_B.ifc、IFC2x3、29 MB）の受け入れ条件A5・A7・A13。
//! `scripts/fetch-testdata.sh`で取得していない場合は何もしない。

mod common;

use std::collections::BTreeMap;

use common::{Feature, options, out_dir, read_tileset, root};
use ifc_tiler::convert;

#[test]
fn walls_are_pickable_with_properties() {
    let input = root().join("testdata/external/ifclite/BURKWIL_DX_B.ifc");
    if !input.exists() {
        eprintln!("skip: {} がない", input.display());
        return;
    }
    let mut opts = options();
    opts.georef.origin = Some([47.5, 9.0, 400.0]);
    let out = out_dir("burkwil");
    let report = convert(&input, &out, &opts).unwrap();
    assert!(report.total_bytes() <= 10_000_000, "{}", report.total_bytes());

    let features = read_tileset(&out);
    let mut classes: BTreeMap<&str, usize> = BTreeMap::new();
    for f in &features {
        *classes.entry(f.props["ifcClass"].as_str().unwrap()).or_default() += 1;
    }
    // 多層壁は部品をまとめて1つの壁になり、540本すべてがPsetか数量を持つ
    assert_eq!(classes["IfcWall"], 540, "{classes:?}");
    let walls = features.iter().filter(|f| f.props["ifcClass"] == "IfcWall");
    assert!(walls.clone().all(|f| f.props.keys().any(|k| k.contains("__") || k.starts_with("p_"))));
    assert!(walls.clone().all(|f| !f.ecef.is_empty()));
    assert!(!classes.contains_key("IfcBuildingElementPart") && !classes.contains_key("IfcOpeningElement"));
}

/// 受け入れ条件A13: インスタンス化しても、全部材の属性と形状の範囲（外接箱）がインスタンス化しない場合と同じ。
#[test]
fn instancing_keeps_every_feature_in_place() {
    let input = root().join("testdata/external/ifclite/BURKWIL_DX_B.ifc");
    if !input.exists() {
        eprintln!("skip: {} がない", input.display());
        return;
    }
    let mut opts = options();
    opts.georef.origin = Some([47.5, 9.0, 400.0]);
    let run = |instancing: bool, name: &str| {
        let out = out_dir(name);
        let report = convert(&input, &out, &ifc_tiler::Options { instancing, ..opts.clone() }).unwrap();
        (report, read_tileset(&out))
    };
    let (with, a) = run(true, "burkwil_instancing");
    let (without, b) = run(false, "burkwil_no_instancing");
    assert!(with.tiles.iter().map(|t| t.instances).sum::<usize>() > 0);
    assert_eq!(without.tiles.iter().map(|t| t.instances).sum::<usize>(), 0);
    assert!(with.total_bytes() < without.total_bytes());

    let key = |f: &Feature| f.props["globalId"].as_str().unwrap().to_string();
    let bbox = |f: &Feature| {
        f.ecef.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(lo, hi), p| {
            (std::array::from_fn(|c| lo[c].min(p[c])), std::array::from_fn(|c| hi[c].max(p[c])))
        })
    };
    let b: BTreeMap<String, &Feature> = b.iter().map(|f| (key(f), f)).collect();
    assert_eq!(a.len(), b.len());
    for f in &a {
        let g = b[&key(f)];
        assert_eq!(f.props, g.props);
        let ((l1, h1), (l2, h2)) = (bbox(f), bbox(g));
        // 量子化の刻み（最大タイルで0.6 mm程度）と、同形の判定の丸め（0.5 mm）の範囲で一致する
        assert!((0..3).all(|c| (l1[c] - l2[c]).abs() < 0.003 && (h1[c] - h2[c]).abs() < 0.003), "{}", key(f));
    }
}
