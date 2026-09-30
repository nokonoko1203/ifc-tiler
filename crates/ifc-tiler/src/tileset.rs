//! tileset.jsonの組み立て。

use serde_json::{Map, Value, json};

use crate::geodesy::Frame;
use crate::metadata::{LOGICAL_ENUM_ID, Table, logical_enum};
use crate::semantics::Storey;
use crate::tiling::{Aabb, Node};

/// `trees[s]`は階`s`のタイルの木。`uri(s, node)`はノードのcontentのURI（ないノードは`None`）。
pub fn build(
    frame: &Frame,
    bounds: &Aabb,
    trees: &[Node],
    uri: &dyn Fn(usize, &Node) -> Option<String>,
    storeys: &[Storey],
    table: &Table,
    extras: Value,
) -> Value {
    let mut classes = Map::new();
    classes.insert(
        "storey".into(),
        json!({ "name": "IFC building storey", "properties": {
            "name": { "type": "STRING", "semantic": "NAME" },
            "globalId": { "type": "STRING", "semantic": "ID" },
            "elevation": { "type": "SCALAR", "componentType": "FLOAT64", "description": "unit: m" }
        } }),
    );
    classes.insert("element".into(), table.full_class());
    let mut schema = json!({ "id": "ifc_tiler", "classes": classes });
    if table.uses_logical() {
        schema["enums"] = json!({ LOGICAL_ENUM_ID: logical_enum() });
    }
    let groups: Vec<Value> = storeys
        .iter()
        .map(|s| {
            let mut p = Map::new();
            p.insert("name".into(), s.name.clone().into());
            if let Some(g) = &s.global_id {
                p.insert("globalId".into(), g.clone().into());
            }
            if let Some(e) = s.elevation_m {
                p.insert("elevation".into(), e.into());
            }
            json!({ "class": "storey", "properties": p })
        })
        .collect();
    let children: Vec<Value> = trees.iter().enumerate().map(|(s, t)| node(t, s, uri)).collect();
    let error = bounds.diagonal();
    json!({
        "asset": { "version": "1.1", "generator": concat!("ifc-tiler ", env!("CARGO_PKG_VERSION")), "extras": { "ifc_tiler": extras } },
        "schema": schema,
        "groups": groups,
        "geometricError": error,
        "root": {
            "transform": frame.transform(),
            "boundingVolume": { "box": bounding_box(bounds) },
            "geometricError": error,
            "refine": "ADD",
            "children": children,
        },
    })
}

fn node(n: &Node, storey: usize, uri: &dyn Fn(usize, &Node) -> Option<String>) -> Value {
    let mut v = json!({ "boundingVolume": { "box": bounding_box(&n.bounds) }, "geometricError": n.geometric_error });
    if let Some(u) = uri(storey, n) {
        v["content"] = json!({ "uri": u, "group": storey });
    }
    if !n.children.is_empty() {
        v["children"] = n.children.iter().map(|c| node(c, storey, uri)).collect();
    }
    v
}

/// 3D Tilesの`box`（中心と3本の半軸）。つぶれた箱を避けるため半長さは1 cm以上にする。
pub fn bounding_box(b: &Aabb) -> [f64; 12] {
    let c = b.center();
    let h = b.size().map(|s| (s / 2.0).max(0.01));
    [c[0], c[1], c[2], h[0], 0.0, 0.0, 0.0, h[1], 0.0, 0.0, 0.0, h[2]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::ElementRecord;
    use crate::units::UnitScales;

    #[test]
    fn tileset_structure() {
        let leaf = Node {
            path: "r0".into(),
            elements: vec![1],
            geometric_error: 0.0,
            bounds: Aabb { min: [0.0; 3], max: [1.0, 1.0, 0.0] },
            children: vec![],
        };
        let root = Node {
            path: "r".into(),
            elements: vec![0],
            geometric_error: 1.5,
            bounds: leaf.bounds,
            children: vec![leaf],
        };
        let uri = |s: usize, n: &Node| Some(format!("tiles/{s:03}_{}.glb", n.path));
        let table = Table::build(
            &[ElementRecord { express_id: 1, ifc_class: "IfcWall".into(), ..Default::default() }],
            &UnitScales::default(),
            true,
        );
        let storeys = [Storey { name: "1階".into(), global_id: None, elevation_m: Some(0.0) }];
        let ts = build(
            &Frame::at_geodetic(35.0, 139.0, 0.0),
            &root.bounds.clone(),
            &[root],
            &uri,
            &storeys,
            &table,
            json!({}),
        );
        assert_eq!(ts["asset"]["version"], "1.1");
        assert_eq!(ts["root"]["refine"], "ADD");
        let c = &ts["root"]["children"][0];
        assert_eq!(c["content"]["uri"], "tiles/000_r.glb");
        assert_eq!(c["content"]["group"], 0);
        assert_eq!(c["children"][0]["geometricError"], 0.0);
        // z方向がつぶれた箱も半長さ1 cm
        assert_eq!(c["boundingVolume"]["box"][11], 0.01);
        assert_eq!(ts["groups"][0]["properties"]["name"], "1階");
        assert!(ts["schema"]["classes"]["element"]["properties"]["expressId"]["required"].as_bool().unwrap());
    }
}
