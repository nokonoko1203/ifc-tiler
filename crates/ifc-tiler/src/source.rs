//! IFCの読み込み。ifc-liteを使うのはこのモジュールだけで、結果は変換器内部の型に詰め替える。
//!
//! - 形状: ifc-liteのメッシュを、IFCの世界座標（メートル、z上）に戻す。
//! - 属性: 部材ごとの属性とPset / Qto（型の値を継承し、部材側の値で上書き済み）。
//! - 関係: 集約（`IfcRelAggregates`）、空間への所属（`IfcRelContainedInSpatialStructure`）、型。
//! - ジオリファレンスと単位: ifc-liteの抽出関数は(0,0)の`IfcSite`も有効とみなすため、エンティティを直接読む。

use std::collections::HashMap;
use std::sync::Arc;

use ifc_lite_core::{
    AttributeValue, DecodedEntity, EntityDecoder, EntityScanner, ProjectUnits, keyword_eq, resolve_unit_by_ref,
};
use ifc_lite_export::{EntityRow, ModelOptions, stream_export_model_with_options};
use ifc_lite_processing::{
    MeshFrame, OpeningFilterMode, StreamingOptions, build_entity_index_parallel,
    process_geometry_streaming_filtered_with_options,
};

use crate::georef::{Crs, MapConversion, RawGeoref, SiteReference};
use crate::metadata::{Logical, Property, Value};
use crate::units::{Quantity, UnitScales};

/// IFCの製品（`IfcProduct`）1つ。空間要素も含む。
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Product {
    pub class: String,
    pub global_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: Option<String>,
    pub tag: Option<String>,
    pub predefined_type: Option<String>,
    /// `IfcBuildingStorey.Elevation` [m]。
    pub elevation_m: Option<f64>,
    pub properties: Vec<Property>,
}

/// 1部材の1メッシュ（IFCの世界座標、m）。
#[derive(Clone, Debug, PartialEq)]
pub struct Mesh {
    pub element: u32,
    pub color: [f32; 4],
    pub positions: Vec<[f64; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub indices: Vec<u32>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceModel {
    pub products: HashMap<u32, Product>,
    pub meshes: Vec<Mesh>,
    /// 部品→全体（`IfcRelAggregates`）。
    pub aggregate_parent: HashMap<u32, u32>,
    /// 部材→空間要素（`IfcRelContainedInSpatialStructure`）。
    pub container: HashMap<u32, u32>,
    /// 部材→型の名前（`IfcRelDefinesByType`）。
    pub type_name: HashMap<u32, String>,
    pub units: UnitScales,
    pub georef: RawGeoref,
}

pub fn read(bytes: &[u8]) -> SourceModel {
    let index = Arc::new(build_entity_index_parallel(bytes));
    let mut model = SourceModel::default();

    let opts = ModelOptions::default().with_inherit_type_properties(true).with_attributes(true);
    let mut rows = Vec::new();
    let scales = stream_export_model_with_options(bytes, &index, &opts, |row, _| rows.push(row));

    let mut decoder = EntityDecoder::with_arc_index(bytes, index);
    let declared = scales.project_id.map(|id| ProjectUnits::resolve(&mut decoder, id)).unwrap_or_default();
    let si = |measure: &str| declared.unit_for_measure(measure).map_or(1.0, |u| u.si_scale);
    model.units = UnitScales {
        length: scales.length_unit_scale,
        area: si("IFCAREAMEASURE"),
        volume: si("IFCVOLUMEMEASURE"),
        mass: si("IFCMASSMEASURE"),
    };
    for row in rows {
        let (id, product) = product(row, model.units.length);
        model.products.insert(id, product);
    }

    model.meshes = meshes(bytes);
    scan_relations_and_georef(bytes, &mut decoder, &mut model);
    model
}

fn product(row: EntityRow, length_m: f64) -> (u32, Product) {
    let attr = |name: &str| row.attributes.iter().find(|a| a.name == name).map(|a| a.value.clone());
    let mut properties = Vec::new();
    for ps in &row.property_sets {
        for p in &ps.properties {
            properties.push(Property {
                set: ps.name.clone(),
                name: p.name.clone(),
                value: parse_value(&p.value, &p.value_type),
                quantity: Quantity::of_measure(&p.value_type),
            });
        }
    }
    for qs in &row.quantity_sets {
        for q in &qs.quantities {
            properties.push(Property {
                set: qs.name.clone(),
                name: q.name.clone(),
                value: Value::Real(q.value),
                quantity: Quantity::of_quantity_kind(q.kind),
            });
        }
    }
    let product = Product {
        class: row.ifc_type,
        global_id: row.global_id,
        name: row.name,
        description: row.description,
        object_type: row.object_type,
        tag: attr("Tag"),
        predefined_type: attr("PredefinedType"),
        elevation_m: attr("Elevation").and_then(|v| v.parse::<f64>().ok()).map(|v| v * length_m),
        properties,
    };
    (row.express_id, product)
}

/// ifc-liteが文字列にした値を、値の型に従って戻す。
fn parse_value(value: &str, value_type: &str) -> Value {
    let t = value_type.to_ascii_uppercase();
    if t == "IFCBOOLEAN" || t == "IFCLOGICAL" {
        return match value {
            "true" => Value::Logical(Logical::True),
            "false" => Value::Logical(Logical::False),
            "unknown" => Value::Logical(Logical::Unknown),
            other => Value::Text(other.to_string()),
        };
    }
    let textual = ["LABEL", "TEXT", "IDENTIFIER", "ENUM", "DATE", "TIME", "URI", "GLOBALLYUNIQUEID"];
    if textual.iter().any(|k| t.contains(k)) && !t.ends_with("MEASURE") {
        return Value::Text(value.to_string());
    }
    if (t == "IFCINTEGER" || t == "IFCCOUNTMEASURE")
        && let Ok(i) = value.parse::<i64>()
    {
        return Value::Int(i);
    }
    match value.parse::<f64>() {
        Ok(f) if f.is_finite() => Value::Real(f),
        _ => Value::Text(value.to_string()),
    }
}

/// 形状を読み、IFCの世界座標に戻す。
fn meshes(bytes: &[u8]) -> Vec<Mesh> {
    let result = process_geometry_streaming_filtered_with_options(
        bytes,
        OpeningFilterMode::Default,
        StreamingOptions::default(),
        |_, _, _| {},
        |_| {},
        |_| {},
    );
    let frame = result.frame;
    result
        .meshes
        .into_iter()
        .filter(|m| !m.indices.is_empty())
        .map(|m| {
            // positions は MeshData.origin からの相対値で、MeshFrame の座標系にある
            let positions = m
                .positions
                .chunks_exact(3)
                .map(|c| {
                    to_world(
                        frame,
                        [m.origin[0] + f64::from(c[0]), m.origin[1] + f64::from(c[1]), m.origin[2] + f64::from(c[2])],
                    )
                })
                .collect();
            let normals = m.normals.chunks_exact(3).map(|c| rotate_to_world(frame, [c[0], c[1], c[2]])).collect();
            Mesh { element: m.express_id, color: m.color, positions, normals, indices: m.indices }
        })
        .collect()
}

fn to_world(frame: MeshFrame, p: [f64; 3]) -> [f64; 3] {
    match frame {
        // フレーム内の点は Rᵀ·(P − t)。P = R·p + t（列優先4×4）
        MeshFrame::SiteLocal { placement: m } => [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ],
        MeshFrame::ModelRtc { anchor } => [p[0] + anchor.0, p[1] + anchor.1, p[2] + anchor.2],
        MeshFrame::RawIfc => p,
    }
}

fn rotate_to_world(frame: MeshFrame, n: [f32; 3]) -> [f32; 3] {
    match frame {
        MeshFrame::SiteLocal { placement: m } => {
            let n = n.map(f64::from);
            [
                (m[0] * n[0] + m[4] * n[1] + m[8] * n[2]) as f32,
                (m[1] * n[0] + m[5] * n[1] + m[9] * n[2]) as f32,
                (m[2] * n[0] + m[6] * n[1] + m[10] * n[2]) as f32,
            ]
        }
        MeshFrame::ModelRtc { .. } | MeshFrame::RawIfc => n,
    }
}

/// 関係とジオリファレンスのエンティティを1回の走査で読む。
fn scan_relations_and_georef(bytes: &[u8], decoder: &mut EntityDecoder, model: &mut SourceModel) {
    let mut scanner = EntityScanner::new(bytes);
    let mut type_of: Vec<(Vec<u32>, u32)> = Vec::new();
    let mut map_conversion = None;
    let mut site = None;
    let mut true_north = None;
    while let Some((id, name, start, end)) = scanner.next_entity() {
        let is = |k: &str| keyword_eq(name, k);
        let wanted = is("IFCRELAGGREGATES")
            || is("IFCRELCONTAINEDINSPATIALSTRUCTURE")
            || is("IFCRELDEFINESBYTYPE")
            || (map_conversion.is_none() && (is("IFCMAPCONVERSION") || is("IFCMAPCONVERSIONSCALED")))
            || (site.is_none() && is("IFCSITE"))
            || (true_north.is_none() && is("IFCGEOMETRICREPRESENTATIONCONTEXT"));
        if !wanted {
            continue;
        }
        let Ok(e) = decoder.decode_at_with_id(id, start, end) else { continue };
        if is("IFCRELAGGREGATES") {
            let (Some(parent), Some(children)) = (e.get_ref(4), e.get_refs(5)) else { continue };
            for c in children {
                model.aggregate_parent.insert(c, parent);
            }
        } else if is("IFCRELCONTAINEDINSPATIALSTRUCTURE") {
            let (Some(children), Some(parent)) = (e.get_refs(4), e.get_ref(5)) else { continue };
            for c in children {
                model.container.insert(c, parent);
            }
        } else if is("IFCRELDEFINESBYTYPE") {
            if let (Some(children), Some(t)) = (e.get_refs(4), e.get_ref(5)) {
                type_of.push((children, t));
            }
        } else if is("IFCMAPCONVERSION") || is("IFCMAPCONVERSIONSCALED") {
            map_conversion = Some(read_map_conversion(&e, decoder));
        } else if is("IFCSITE") {
            site = Some(SiteReference {
                latitude_deg: e.get(9).and_then(compound_angle),
                longitude_deg: e.get(10).and_then(compound_angle),
                elevation: e.get_float(11),
            });
        } else if is("IFCGEOMETRICREPRESENTATIONCONTEXT")
            && e.get_string(1).is_some_and(|t| t.eq_ignore_ascii_case("Model"))
        {
            true_north = e.get_ref(5).and_then(|d| decoder.decode_by_id(d).ok()).and_then(|d| {
                let r: Vec<f64> = d.get_list(0)?.iter().filter_map(AttributeValue::as_float).collect();
                (r.len() >= 2 && (r[0] != 0.0 || r[1] != 0.0)).then(|| [r[0], r[1]])
            });
        }
    }
    for (children, t) in type_of {
        let Some(name) = decoder.decode_by_id(t).ok().and_then(|d| d.get_string(2).map(str::to_string)) else {
            continue;
        };
        for c in children {
            model.type_name.insert(c, name.clone());
        }
    }
    model.georef = RawGeoref { map_conversion, site, true_north, length_unit_m: model.units.length };
}

fn read_map_conversion(e: &DecodedEntity, decoder: &mut EntityDecoder) -> MapConversion {
    let factor = |i| e.get_float(i).unwrap_or(1.0);
    MapConversion {
        eastings: e.get_float(2).unwrap_or(0.0),
        northings: e.get_float(3).unwrap_or(0.0),
        orthogonal_height: e.get_float(4).unwrap_or(0.0),
        x_axis_abscissa: e.get_float(5),
        x_axis_ordinate: e.get_float(6),
        scale: e.get_float(7),
        factors: [factor(8), factor(9), factor(10)],
        target: read_crs(e.get_ref(1), decoder),
    }
}

fn read_crs(id: Option<u32>, decoder: &mut EntityDecoder) -> Crs {
    let Some(e) = id.and_then(|i| decoder.decode_by_id(i).ok()) else { return Crs::Missing };
    if !e.ifc_type.name().eq_ignore_ascii_case("IFCPROJECTEDCRS") {
        return Crs::Missing;
    }
    let name = e.get_string(0).map(str::to_string);
    let map_unit_m = e.get_ref(6).and_then(|u| resolve_unit_by_ref(decoder, u)).map(|(_, u, _)| u.si_scale);
    Crs::Projected { name, map_unit_m }
}

/// `IfcCompoundPlaneAngleMeasure`（度・分・秒・百万分の1秒。各要素は同じ符号）→ 度。
fn compound_angle(v: &AttributeValue) -> Option<f64> {
    let parts: Vec<f64> = v.as_list()?.iter().filter_map(AttributeValue::as_float).collect();
    let [d, rest @ ..] = parts.as_slice() else { return None };
    let scale = [60.0, 3600.0, 3.6e9];
    Some(d + rest.iter().zip(scale).map(|(x, s)| x / s).sum::<f64>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_typed_by_ifc_value_type() {
        assert_eq!(parse_value("true", "IFCBOOLEAN"), Value::Logical(Logical::True));
        assert_eq!(parse_value("unknown", "IFCLOGICAL"), Value::Logical(Logical::Unknown));
        assert_eq!(parse_value("123", "IFCLABEL"), Value::Text("123".into()));
        assert_eq!(parse_value("3", "IFCINTEGER"), Value::Int(3));
        assert_eq!(parse_value("250", "IFCPOSITIVELENGTHMEASURE"), Value::Real(250.0));
        assert_eq!(parse_value("REI60", "IFCREAL"), Value::Text("REI60".into()));
    }

    #[test]
    fn compound_angles() {
        let v = AttributeValue::List(vec![
            AttributeValue::Integer(35),
            AttributeValue::Integer(40),
            AttributeValue::Integer(52),
            AttributeValue::Integer(449_600),
        ]);
        assert!((compound_angle(&v).unwrap() - 35.681236).abs() < 1e-9);
        let neg = AttributeValue::List(vec![AttributeValue::Integer(-35), AttributeValue::Integer(-30)]);
        assert_eq!(compound_angle(&neg), Some(-35.5));
    }
}
