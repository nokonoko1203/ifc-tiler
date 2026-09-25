//! 実務規模のモデル（BURKWIL_DX_B.ifc、IFC2x3、29 MB）の受け入れ条件A5・A7。
//! `scripts/fetch-testdata.sh`で取得していない場合は何もしない。

mod common;

use std::collections::BTreeMap;

use common::{options, out_dir, read_tileset, root};
use ifc2tiles::convert;

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
