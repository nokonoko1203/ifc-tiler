//! 3D Tilesのfeatureにする部材の決定。
//!
//! 形状を持つ製品から、開口・室・構造解析・型などを除き、集約の部品は親の部材にまとめる
//! （多層壁の親は軸線しか持たず、Psetは親にある。部品にはPsetがない）。

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::metadata::ElementRecord;
use crate::source::{Product, SourceModel};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SemanticsOptions {
    pub include_spaces: bool,
    pub keep_parts: bool,
}

/// featureになる部材。
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub record: ElementRecord,
    /// 所属階（`Semantics::storeys`の番号）。
    pub storey: usize,
    /// `SourceModel::meshes`の番号。
    pub meshes: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Storey {
    pub name: String,
    pub global_id: Option<String>,
    pub elevation_m: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Semantics {
    pub elements: Vec<Element>,
    /// 標高の昇順。所属階のない部材は`(unassigned)`。
    pub storeys: Vec<Storey>,
    /// 除外した製品のクラス別件数。
    pub excluded: BTreeMap<String, usize>,
    /// 形状表現を持つのにメッシュが出なかった製品のクラス別件数。
    pub without_mesh: BTreeMap<String, usize>,
}

const SPATIAL: [&str; 20] = [
    "IfcProject",
    "IfcSite",
    "IfcBuilding",
    "IfcBuildingStorey",
    "IfcSpace",
    "IfcSpatialZone",
    "IfcExternalSpatialElement",
    "IfcFacility",
    "IfcFacilityPart",
    "IfcFacilityPartCommon",
    "IfcBridge",
    "IfcBridgePart",
    "IfcRoad",
    "IfcRoadPart",
    "IfcRailway",
    "IfcRailwayPart",
    "IfcMarineFacility",
    "IfcMarinePart",
    "IfcTunnel",
    "IfcTunnelPart",
];

fn is_spatial(class: &str) -> bool {
    SPATIAL.contains(&class)
}

/// featureにしない製品か。
pub fn is_excluded(class: &str, include_spaces: bool) -> bool {
    matches!(class, "IfcOpeningElement" | "IfcOpeningStandardCase" | "IfcVirtualElement" | "IfcAnnotation" | "IfcGrid")
        || class.starts_with("IfcStructural")
        || class.ends_with("Type")
        || class.ends_with("Style")
        || (class == "IfcSpace" && !include_spaces)
}

pub fn build(model: &SourceModel, opts: SemanticsOptions) -> Semantics {
    let mut s = Semantics::default();
    let mut excluded_ids: HashSet<u32> = HashSet::new();
    let mut by_owner: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, m) in model.meshes.iter().enumerate() {
        let Some(p) = model.products.get(&m.element) else { continue };
        if is_excluded(&p.class, opts.include_spaces) {
            if excluded_ids.insert(m.element) {
                *s.excluded.entry(p.class.clone()).or_default() += 1;
            }
            continue;
        }
        let owner = if opts.keep_parts { m.element } else { owner_of(model, m.element) };
        by_owner.entry(owner).or_default().push(i);
    }

    let meshed: HashSet<u32> = model.meshes.iter().map(|m| m.element).collect();
    for (id, p) in &model.products {
        if p.has_representation
            && !meshed.contains(id)
            && !by_owner.contains_key(id)
            && !is_excluded(&p.class, opts.include_spaces)
        {
            *s.without_mesh.entry(p.class.clone()).or_default() += 1;
        }
    }

    let mut storey_index: HashMap<Option<u32>, usize> = HashMap::new();
    let mut storey_ids: Vec<Option<u32>> = Vec::new();
    for (owner, meshes) in by_owner {
        let p = &model.products[&owner];
        let (storey, building) = spatial_ancestors(model, owner);
        let storey_product = storey.map(|id| &model.products[&id]);
        let record = ElementRecord {
            express_id: owner,
            ifc_class: p.class.clone(),
            global_id: p.global_id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            object_type: p.object_type.clone(),
            tag: p.tag.clone(),
            predefined_type: p.predefined_type.clone(),
            type_name: model.type_name.get(&owner).cloned(),
            storey_name: storey_product.and_then(|s| s.name.clone()),
            storey_global_id: storey_product.and_then(|s| s.global_id.clone()),
            building_name: building.and_then(|b| model.products[&b].name.clone()),
            properties: p.properties.clone(),
        };
        let k = *storey_index.entry(storey).or_insert_with(|| {
            storey_ids.push(storey);
            storey_ids.len() - 1
        });
        s.elements.push(Element { record, storey: k, meshes });
    }

    // 階を標高の昇順に並べ替え、部材の階番号を付け直す
    let storey_of = |id: &Option<u32>| id.and_then(|i| model.products.get(&i));
    let mut order: Vec<usize> = (0..storey_ids.len()).collect();
    order.sort_by(|&a, &b| {
        let key = |i: usize| {
            let p = storey_of(&storey_ids[i]);
            (p.is_none(), p.and_then(|p| p.elevation_m).unwrap_or(f64::INFINITY), storey_ids[i])
        };
        let (ka, kb) = (key(a), key(b));
        ka.0.cmp(&kb.0).then(ka.1.total_cmp(&kb.1)).then(ka.2.cmp(&kb.2))
    });
    let mut new_index = vec![0; order.len()];
    for (new, &old) in order.iter().enumerate() {
        new_index[old] = new;
        let p = storey_of(&storey_ids[old]);
        s.storeys.push(Storey {
            name: p.and_then(|p| p.name.clone()).unwrap_or_else(|| "(unassigned)".into()),
            global_id: p.and_then(|p| p.global_id.clone()),
            elevation_m: p.and_then(|p| p.elevation_m),
        });
    }
    for e in &mut s.elements {
        e.storey = new_index[e.storey];
    }
    s
}

/// 集約を上へ辿り、空間要素の直下にある部材を返す。
fn owner_of(model: &SourceModel, id: u32) -> u32 {
    let mut cur = id;
    for _ in 0..32 {
        let Some(parent) = model.aggregate_parent.get(&cur) else { break };
        match model.products.get(parent) {
            Some(Product { class, .. }) if !is_spatial(class) => cur = *parent,
            _ => break,
        }
    }
    cur
}

/// 空間構造を上へ辿り、最初の階と建物を返す。
fn spatial_ancestors(model: &SourceModel, id: u32) -> (Option<u32>, Option<u32>) {
    let (mut storey, mut building) = (None, None);
    let mut cur = id;
    for _ in 0..32 {
        let Some(&parent) = model.container.get(&cur).or_else(|| model.aggregate_parent.get(&cur)) else { break };
        match model.products.get(&parent).map(|p| p.class.as_str()) {
            Some("IfcBuildingStorey") if storey.is_none() => storey = Some(parent),
            Some("IfcBuilding") if building.is_none() => building = Some(parent),
            None => break,
            _ => {}
        }
        cur = parent;
    }
    (storey, building)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Mesh;

    fn product(class: &str, name: &str) -> Product {
        Product { class: class.into(), name: Some(name.into()), has_representation: true, ..Default::default() }
    }

    fn mesh(element: u32) -> Mesh {
        Mesh {
            element,
            color: [1.0; 4],
            positions: vec![[0.0; 3]; 3],
            normals: vec![[0.0, 0.0, 1.0]; 3],
            indices: vec![0, 1, 2],
        }
    }

    /// 建物1 > 階(1階 0 m, 2階 3 m)。1階に多層壁（部品2つ）と開口、2階に柱。
    fn model() -> SourceModel {
        let mut m = SourceModel::default();
        for (id, p) in [
            (1, product("IfcBuilding", "建物")),
            (2, Product { elevation_m: Some(3.0), ..product("IfcBuildingStorey", "2階") }),
            (3, Product { elevation_m: Some(0.0), ..product("IfcBuildingStorey", "1階") }),
            (10, product("IfcWall", "壁")),
            (11, product("IfcBuildingElementPart", "部品A")),
            (12, product("IfcBuildingElementPart", "部品B")),
            (13, product("IfcOpeningElement", "開口")),
            (20, product("IfcColumn", "柱")),
            (21, product("IfcBuildingElementProxy", "形状なし")),
        ] {
            m.products.insert(id, p);
        }
        m.aggregate_parent.extend([(2, 1), (3, 1), (11, 10), (12, 10)]);
        m.container.extend([(10, 3), (20, 2), (21, 3)]);
        m.type_name.insert(20, "角柱".into());
        m.meshes = vec![mesh(11), mesh(12), mesh(13), mesh(20)];
        m
    }

    #[test]
    fn parts_are_grouped_into_the_parent() {
        let s = build(&model(), SemanticsOptions { include_spaces: false, keep_parts: false });
        let names: Vec<_> = s.elements.iter().map(|e| e.record.name.clone().unwrap()).collect();
        assert_eq!(names, ["壁", "柱"]);
        assert_eq!(s.elements[0].meshes, [0, 1]);
        assert_eq!(s.elements[0].record.storey_name.as_deref(), Some("1階"));
        assert_eq!(s.elements[0].record.building_name.as_deref(), Some("建物"));
        assert_eq!(s.elements[1].record.type_name.as_deref(), Some("角柱"));
        assert_eq!(s.excluded.get("IfcOpeningElement"), Some(&1));
        assert_eq!(s.without_mesh.get("IfcBuildingElementProxy"), Some(&1));
        assert!(!s.without_mesh.contains_key("IfcWall"));
    }

    #[test]
    fn keep_parts_leaves_parts_as_features() {
        let s = build(&model(), SemanticsOptions { include_spaces: false, keep_parts: true });
        assert_eq!(s.elements.len(), 3);
        // 部品は空間構造に直接所属していないが、親を経由して階が分かる
        assert!(s.elements.iter().all(|e| e.record.storey_name.is_some()));
        assert_eq!(s.without_mesh.get("IfcWall"), Some(&1));
    }

    #[test]
    fn storeys_are_sorted_by_elevation() {
        let s = build(&model(), SemanticsOptions { include_spaces: false, keep_parts: false });
        let names: Vec<_> = s.storeys.iter().map(|st| st.name.as_str()).collect();
        assert_eq!(names, ["1階", "2階"]);
        assert_eq!(s.elements[0].storey, 0);
        assert_eq!(s.elements[1].storey, 1);
    }

    #[test]
    fn exclusions() {
        assert!(is_excluded("IfcOpeningElement", false));
        assert!(is_excluded("IfcStructuralCurveMember", false));
        assert!(is_excluded("IfcWallType", false));
        assert!(is_excluded("IfcDoorStyle", false));
        assert!(is_excluded("IfcSpace", false));
        assert!(!is_excluded("IfcSpace", true));
        assert!(!is_excluded("IfcWall", false));
    }
}
