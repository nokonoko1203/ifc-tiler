//! Deciding which elements become 3D Tiles features.
//!
//! From the products that have geometry, openings, spaces, structural analysis elements and types are excluded,
//! and aggregated parts are merged into their parent (a multi-layer wall's parent only has an axis, and its Psets are on the parent; parts have none).

use std::collections::{BTreeMap, HashMap};

use crate::metadata::ElementRecord;
use crate::source::{Product, SourceModel};

/// An element that becomes a feature.
#[derive(Clone, Debug, PartialEq)]
pub struct Element {
    pub record: ElementRecord,
    /// Storey the element belongs to (index into `Semantics::storeys`).
    pub storey: usize,
    /// Indices into `SourceModel::meshes`.
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
    /// In ascending order of elevation. Elements without a storey go to `(unassigned)`.
    pub storeys: Vec<Storey>,
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

/// Whether a product is excluded from becoming a feature.
fn is_excluded(class: &str) -> bool {
    matches!(class, "IfcOpeningElement" | "IfcOpeningStandardCase" | "IfcVirtualElement" | "IfcAnnotation" | "IfcGrid")
        || class.starts_with("IfcStructural")
        || class.ends_with("Type")
        || class.ends_with("Style")
        || class == "IfcSpace"
}

pub fn build(model: &SourceModel) -> Semantics {
    let mut s = Semantics::default();
    let mut by_owner: BTreeMap<u32, Vec<usize>> = BTreeMap::new();
    for (i, m) in model.meshes.iter().enumerate() {
        let Some(p) = model.products.get(&m.element) else { continue };
        if is_excluded(&p.class) {
            continue;
        }
        let owner = owner_of(model, m.element);
        by_owner.entry(owner).or_default().push(i);
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

    // Sort the storeys by elevation and renumber the elements' storeys
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

/// Follows aggregation upwards and returns the element directly under a spatial element.
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

/// Follows the spatial structure upwards and returns the first storey and building.
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
        Product { class: class.into(), name: Some(name.into()), ..Default::default() }
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

    /// Building 1 > storeys (1F at 0 m, 2F at 3 m). A multi-layer wall (two parts) and an opening on 1F, a column on 2F.
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
        let s = build(&model());
        let names: Vec<_> = s.elements.iter().map(|e| e.record.name.clone().unwrap()).collect();
        assert_eq!(names, ["壁", "柱"]);
        assert_eq!(s.elements[0].meshes, [0, 1]);
        assert_eq!(s.elements[0].record.storey_name.as_deref(), Some("1階"));
        assert_eq!(s.elements[0].record.building_name.as_deref(), Some("建物"));
        assert_eq!(s.elements[1].record.type_name.as_deref(), Some("角柱"));
    }

    #[test]
    fn storeys_are_sorted_by_elevation() {
        let s = build(&model());
        let names: Vec<_> = s.storeys.iter().map(|st| st.name.as_str()).collect();
        assert_eq!(names, ["1階", "2階"]);
        assert_eq!(s.elements[0].storey, 0);
        assert_eq!(s.elements[1].storey, 1);
    }

    #[test]
    fn exclusions() {
        assert!(is_excluded("IfcOpeningElement"));
        assert!(is_excluded("IfcStructuralCurveMember"));
        assert!(is_excluded("IfcWallType"));
        assert!(is_excluded("IfcDoorStyle"));
        assert!(is_excluded("IfcSpace"));
        assert!(!is_excluded("IfcWall"));
    }
}
