//! Columns of the element metadata (the class and property table of `EXT_structural_metadata`)
//!
//! One `element` class covers the whole tileset, and column types are decided from the values of all elements
//! Each GLB only gets columns with at least one value in that tile (3d-tiles-validator requires
//! every column of the class to be in the property table, and a string column that is empty for all elements cannot be encoded)

use std::collections::{BTreeMap, HashSet};

use serde_json::{Map, Value as Json, json};

use crate::units::{Quantity, UnitScales};

/// A value of `IfcLogical` (IfcBoolean / IfcLogical)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Logical {
    False,
    True,
    Unknown,
}

/// A single property value
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Logical(Logical),
    Int(i64),
    Real(f64),
    Text(String),
}

impl Value {
    fn to_text(&self) -> String {
        match self {
            Self::Logical(Logical::False) => "FALSE".into(),
            Self::Logical(Logical::True) => "TRUE".into(),
            Self::Logical(Logical::Unknown) => "UNKNOWN".into(),
            Self::Int(i) => i.to_string(),
            Self::Real(f) => f.to_string(),
            Self::Text(s) => s.clone(),
        }
    }
}

/// One entry of a Pset / Qto
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    pub set: String,
    pub name: String,
    pub value: Value,
    /// Set if the value measures this quantity, in project units
    pub quantity: Option<Quantity>,
}

/// The values of one element (fixed columns and Pset / Qto)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ElementRecord {
    pub express_id: u32,
    pub ifc_class: String,
    pub global_id: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub object_type: Option<String>,
    pub tag: Option<String>,
    pub predefined_type: Option<String>,
    pub type_name: Option<String>,
    pub storey_name: Option<String>,
    pub storey_global_id: Option<String>,
    pub building_name: Option<String>,
    pub properties: Vec<Property>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Uint32,
    /// Integer. INT64 is avoided because CesiumJS returns it as a BigInt, which never matches noData (a JSON number)
    Int32,
    Float64,
    Text,
    Logical,
}

const NO_DATA_F64: f64 = -9999.0;
const NO_DATA_I32: i32 = i32::MIN;
const LOGICAL_ENUM: &str = "IfcLogical";
const LOGICAL_NOT_SET: u8 = 255;

#[derive(Clone, Debug, PartialEq)]
struct Column {
    id: String,
    /// Original name (`Pset name.property name`), `None` if identical to the ID
    name: Option<String>,
    description: Option<String>,
    semantic: Option<&'static str>,
    required: bool,
    kind: Kind,
}

impl Column {
    fn class_property(&self) -> Json {
        let mut p = Map::new();
        if let Some(n) = &self.name {
            p.insert("name".into(), n.clone().into());
        }
        if let Some(d) = &self.description {
            p.insert("description".into(), d.clone().into());
        }
        if let Some(s) = self.semantic {
            p.insert("semantic".into(), s.into());
        }
        match self.kind {
            Kind::Uint32 => {
                p.insert("type".into(), "SCALAR".into());
                p.insert("componentType".into(), "UINT32".into());
            }
            Kind::Int32 => {
                p.insert("type".into(), "SCALAR".into());
                p.insert("componentType".into(), "INT32".into());
                p.insert("noData".into(), NO_DATA_I32.into());
            }
            Kind::Float64 => {
                p.insert("type".into(), "SCALAR".into());
                p.insert("componentType".into(), "FLOAT64".into());
                p.insert("noData".into(), NO_DATA_F64.into());
            }
            Kind::Text => {
                p.insert("type".into(), "STRING".into());
                if !self.required {
                    p.insert("noData".into(), "".into());
                }
            }
            Kind::Logical => {
                p.insert("type".into(), "ENUM".into());
                p.insert("enumType".into(), LOGICAL_ENUM.into());
                p.insert("noData".into(), "NOT_SET".into());
            }
        }
        if self.required {
            p.insert("required".into(), true.into());
        }
        Json::Object(p)
    }
}

/// Columns and values of all elements. `rows[i][j]` is the value of column `j` of element `i`
#[derive(Clone, Debug, PartialEq)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<Option<Value>>>,
}

const FIXED: [(&str, Option<&str>); 12] = [
    ("expressId", None),
    ("ifcClass", None),
    ("globalId", Some("ID")),
    ("name", Some("NAME")),
    ("description", Some("DESCRIPTION")),
    ("objectType", None),
    ("tag", None),
    ("predefinedType", None),
    ("typeName", None),
    ("storeyName", None),
    ("storeyGlobalId", None),
    ("buildingName", None),
];

fn fixed_values(e: &ElementRecord) -> [Option<Value>; 12] {
    let t = |s: &Option<String>| s.as_ref().filter(|s| !s.is_empty()).map(|s| Value::Text(s.clone()));
    [
        Some(Value::Int(i64::from(e.express_id))),
        Some(Value::Text(e.ifc_class.clone())),
        t(&e.global_id),
        t(&e.name),
        t(&e.description),
        t(&e.object_type),
        t(&e.tag),
        t(&e.predefined_type),
        t(&e.type_name),
        t(&e.storey_name),
        t(&e.storey_global_id),
        t(&e.building_name),
    ]
}

impl Table {
    /// Decides the columns and converts values to SI units
    pub fn build(elements: &[ElementRecord], units: &UnitScales) -> Self {
        let mut columns: Vec<Column> = FIXED
            .iter()
            .enumerate()
            .map(|(i, (id, semantic))| Column {
                id: (*id).into(),
                name: None,
                description: None,
                semantic: *semantic,
                required: i < 2,
                kind: if i == 0 { Kind::Uint32 } else { Kind::Text },
            })
            .collect();
        let mut rows: Vec<Vec<Option<Value>>> = elements.iter().map(|e| fixed_values(e).to_vec()).collect();
        // (Pset name, property name) → values per element
        let mut by_key: BTreeMap<(&str, &str), Vec<(usize, &Property)>> = BTreeMap::new();
        for (i, e) in elements.iter().enumerate() {
            for p in &e.properties {
                if matches!(&p.value, Value::Text(s) if s.is_empty()) {
                    continue;
                }
                by_key.entry((&p.set, &p.name)).or_default().push((i, p));
            }
        }
        let mut used: HashSet<String> = columns.iter().map(|c| c.id.clone()).collect();
        for ((set, name), values) in by_key {
            let full = format!("{set}.{name}");
            let id = unique_id(property_id(set, name), &full, &mut used);
            let quantity = values.iter().find_map(|(_, p)| p.quantity);
            let converted: Vec<(usize, Value)> = values
                .into_iter()
                .map(|(i, p)| {
                    let v = match (p.value.clone(), p.quantity) {
                        (Value::Real(f), Some(q)) => Value::Real(f * units.si(q)),
                        (Value::Int(n), Some(q)) => Value::Real(n as f64 * units.si(q)),
                        (v, _) => v,
                    };
                    (i, v)
                })
                .collect();
            let kind = infer_kind(converted.iter().map(|(_, v)| v));
            for row in &mut rows {
                row.push(None);
            }
            let j = columns.len();
            for (i, v) in converted {
                rows[i][j] = Some(coerce(v, kind));
            }
            columns.push(Column {
                name: (id != full).then_some(full),
                id,
                description: quantity.filter(|_| kind == Kind::Float64).map(|q| format!("unit: {}", q.symbol())),
                semantic: None,
                required: false,
                kind,
            });
        }
        Self { columns, rows }
    }

    /// Class with all columns, for the `schema` of tileset.json
    pub fn full_class(&self) -> Json {
        let props: Map<String, Json> = self.columns.iter().map(|c| (c.id.clone(), c.class_property())).collect();
        json!({ "name": "IFC element", "properties": props })
    }

    pub fn uses_logical(&self) -> bool {
        self.columns.iter().any(|c| c.kind == Kind::Logical)
    }

    /// Encodes one tile (elements in the order of `rows`)
    /// `add_view` registers bytes as a bufferView and returns its index
    /// Returns the tile's class and property table
    pub fn encode(&self, rows: &[usize], add_view: &mut dyn FnMut(Vec<u8>) -> usize) -> (Json, Json) {
        let mut class_props = Map::new();
        let mut table_props = Map::new();
        for (j, col) in self.columns.iter().enumerate() {
            let values: Vec<Option<&Value>> = rows.iter().map(|&i| self.rows[i][j].as_ref()).collect();
            if !col.required && values.iter().all(Option::is_none) {
                continue;
            }
            class_props.insert(col.id.clone(), col.class_property());
            table_props.insert(col.id.clone(), encode_column(col.kind, &values, add_view));
        }
        let class = json!({ "properties": class_props });
        let table = json!({ "class": "element", "count": rows.len(), "properties": table_props });
        (class, table)
    }
}

pub fn logical_enum() -> Json {
    json!({
        "valueType": "UINT8",
        "values": [
            { "name": "FALSE", "value": 0 },
            { "name": "TRUE", "value": 1 },
            { "name": "UNKNOWN", "value": 2 },
            { "name": "NOT_SET", "value": LOGICAL_NOT_SET }
        ]
    })
}

pub const LOGICAL_ENUM_ID: &str = LOGICAL_ENUM;

fn encode_column(kind: Kind, values: &[Option<&Value>], add_view: &mut dyn FnMut(Vec<u8>) -> usize) -> Json {
    let mut bytes = Vec::new();
    match kind {
        Kind::Uint32 => {
            for v in values {
                let n = match v {
                    Some(Value::Int(n)) => u32::try_from(*n).unwrap_or(0),
                    _ => 0,
                };
                bytes.extend(n.to_le_bytes());
            }
        }
        Kind::Int32 => {
            for v in values {
                let n = match v {
                    Some(Value::Int(n)) => i32::try_from(*n).expect("values of an Int32 column fit in 32 bits"),
                    _ => NO_DATA_I32,
                };
                bytes.extend(n.to_le_bytes());
            }
        }
        Kind::Float64 => {
            for v in values {
                let f = match v {
                    Some(Value::Real(f)) => *f,
                    _ => NO_DATA_F64,
                };
                bytes.extend(f.to_le_bytes());
            }
        }
        Kind::Logical => {
            for v in values {
                bytes.push(match v {
                    Some(Value::Logical(Logical::False)) => 0,
                    Some(Value::Logical(Logical::True)) => 1,
                    Some(Value::Logical(Logical::Unknown)) => 2,
                    _ => LOGICAL_NOT_SET,
                });
            }
        }
        Kind::Text => {
            let mut offsets = Vec::with_capacity(4 * (values.len() + 1));
            offsets.extend(0u32.to_le_bytes());
            for v in values {
                if let Some(Value::Text(s)) = v {
                    bytes.extend(s.as_bytes());
                }
                let end = u32::try_from(bytes.len()).expect("a string column does not exceed 4 GiB");
                offsets.extend(end.to_le_bytes());
            }
            let values = add_view(bytes);
            let offsets = add_view(offsets);
            return json!({ "values": values, "stringOffsets": offsets, "stringOffsetType": "UINT32" });
        }
    }
    json!({ "values": add_view(bytes) })
}

fn infer_kind<'a>(values: impl Iterator<Item = &'a Value> + Clone) -> Kind {
    if values.clone().all(|v| matches!(v, Value::Logical(_))) {
        Kind::Logical
    } else if values.clone().all(|v| matches!(v, Value::Int(n) if i32::try_from(*n).is_ok_and(|n| n != NO_DATA_I32))) {
        Kind::Int32
    } else if values.clone().all(|v| matches!(v, Value::Int(_) | Value::Real(_))) {
        Kind::Float64
    } else {
        Kind::Text
    }
}

fn coerce(v: Value, kind: Kind) -> Value {
    match (kind, v) {
        (Kind::Float64, Value::Int(n)) => Value::Real(n as f64),
        (Kind::Text, v) => Value::Text(v.to_text()),
        (_, v) => v,
    }
}

/// Fits `Pset name__property name` to the metadata ID rule (`^[a-zA-Z_][a-zA-Z0-9_]*$`)
fn property_id(set: &str, name: &str) -> String {
    let raw = format!("{set}__{name}");
    let ascii = raw.chars().filter(char::is_ascii_alphanumeric).count();
    if ascii * 2 < raw.chars().count() {
        // Names such as Japanese ones would collide after replacement, so they are hashed
        return format!("p_{:08x}", fnv1a(&format!("{set}.{name}")));
    }
    let mut id: String = raw.chars().map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' }).collect();
    if id.starts_with(|c: char| c.is_ascii_digit()) {
        id.insert(0, '_');
    }
    id
}

fn unique_id(id: String, full: &str, used: &mut HashSet<String>) -> String {
    let id = if used.contains(&id) { format!("{id}_{:08x}", fnv1a(full)) } else { id };
    used.insert(id.clone());
    id
}

fn fnv1a(s: &str) -> u32 {
    s.bytes().fold(0x811c_9dc5, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prop(set: &str, name: &str, value: Value, quantity: Option<Quantity>) -> Property {
        Property { set: set.into(), name: name.into(), value, quantity }
    }

    fn element(id: u32, props: Vec<Property>) -> ElementRecord {
        ElementRecord { express_id: id, ifc_class: "IfcWall".into(), properties: props, ..Default::default() }
    }

    fn id_ok(id: &str) -> bool {
        let mut c = id.chars();
        c.next().is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
            && c.all(|x| x.is_ascii_alphanumeric() || x == '_')
    }

    #[test]
    fn ids_follow_metadata_rules() {
        for (set, name) in
            [("Pset_WallCommon", "IsExternal"), ("3D", "Höhe (m)"), ("ArchiCAD", "階高"), ("構造", "耐火性能")]
        {
            let id = property_id(set, name);
            assert!(id_ok(&id), "{id}");
        }
        assert_eq!(property_id("Pset_WallCommon", "IsExternal"), "Pset_WallCommon__IsExternal");
        assert_eq!(property_id("3D", "Höhe (m)"), "_3D__H_he__m_");
        assert!(property_id("構造", "耐火性能").starts_with("p_"));
        assert_ne!(property_id("構造", "耐火性能"), property_id("構造", "階高"));
    }

    #[test]
    fn colliding_ids_get_a_hash_suffix() {
        let t = Table::build(
            &[element(1, vec![prop("A B", "c", Value::Int(1), None), prop("A_B", "c", Value::Int(2), None)])],
            &UnitScales::default(),
        );
        let ids: Vec<&str> = t.columns.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains(&"A_B__c"));
        assert_eq!(ids.iter().filter(|i| i.starts_with("A_B__c")).count(), 2);
        assert!(ids.iter().all(|i| id_ok(i)));
    }

    #[test]
    fn kinds_are_inferred_across_elements() {
        let t = Table::build(
            &[
                element(
                    1,
                    vec![
                        prop("P", "b", Value::Logical(Logical::True), None),
                        prop("P", "n", Value::Int(1), None),
                        prop("P", "m", Value::Int(1), None),
                        prop("P", "s", Value::Int(1), None),
                    ],
                ),
                element(
                    2,
                    vec![
                        prop("P", "b", Value::Logical(Logical::False), None),
                        prop("P", "n", Value::Int(2), None),
                        prop("P", "m", Value::Real(2.5), None),
                        prop("P", "s", Value::Text("x".into()), None),
                    ],
                ),
            ],
            &UnitScales::default(),
        );
        let kind = |id: &str| t.columns.iter().find(|c| c.id == id).unwrap().kind;
        assert_eq!(kind("P__b"), Kind::Logical);
        assert_eq!(kind("P__n"), Kind::Int32);
        assert_eq!(kind("P__m"), Kind::Float64);
        assert_eq!(kind("P__s"), Kind::Text);
        let s = t.columns.iter().position(|c| c.id == "P__s").unwrap();
        assert_eq!(t.rows[0][s], Some(Value::Text("1".into())));
    }

    #[test]
    fn measures_are_converted_to_si() {
        let units = UnitScales { length: 0.001, area: 1e-6, volume: 1e-9, mass: 1.0 };
        let t = Table::build(
            &[element(
                1,
                vec![
                    prop("Qto", "Width", Value::Real(250.0), Some(Quantity::Length)),
                    prop("Qto", "Area", Value::Real(2e6), Some(Quantity::Area)),
                    prop("P", "Ratio", Value::Real(0.5), None),
                ],
            )],
            &units,
        );
        let get = |id: &str| {
            let j = t.columns.iter().position(|c| c.id == id).unwrap();
            (t.rows[0][j].clone(), t.columns[j].description.clone())
        };
        assert_eq!(get("Qto__Width"), (Some(Value::Real(0.25)), Some("unit: m".into())));
        assert_eq!(get("Qto__Area"), (Some(Value::Real(2.0)), Some("unit: m2".into())));
        assert_eq!(get("P__Ratio"), (Some(Value::Real(0.5)), None));
    }

    #[test]
    fn empty_strings_do_not_make_columns() {
        let t =
            Table::build(&[element(1, vec![prop("P", "e", Value::Text(String::new()), None)])], &UnitScales::default());
        assert!(t.columns.iter().all(|c| c.id != "P__e"));
    }

    #[test]
    fn tile_encoding_omits_columns_without_values() {
        let t = Table::build(
            &[
                element(1, vec![prop("P", "a", Value::Real(1.0), None)]),
                element(2, vec![prop("P", "b", Value::Text("x".into()), None)]),
            ],
            &UnitScales::default(),
        );
        let mut views: Vec<Vec<u8>> = Vec::new();
        let (class, table) = t.encode(&[1], &mut |b| {
            views.push(b);
            views.len() - 1
        });
        let props = class["properties"].as_object().unwrap();
        assert!(props.contains_key("expressId") && props.contains_key("ifcClass") && props.contains_key("P__b"));
        assert!(!props.contains_key("P__a") && !props.contains_key("name"));
        assert_eq!(table["count"], 1);
        // expressId(4) / ifcClass(7 value bytes + 8 offset bytes) / P__b(1 value byte + 8 offset bytes)
        let sizes: Vec<usize> = views.iter().map(Vec::len).collect();
        assert_eq!(sizes, vec![4, 7, 8, 1, 8]);
        assert_eq!(views[0], 2u32.to_le_bytes());
    }

    #[test]
    fn integers_outside_int32_become_float64() {
        // INT64 is avoided because CesiumJS returns a BigInt and noData does not apply
        let e = |id, n| element(id, vec![prop("P", "n", Value::Int(n), None)]);
        let kind = |t: &Table| t.columns.iter().find(|c| c.id == "P__n").unwrap().kind;
        let wide = Table::build(&[e(1, 1), e(2, 1 << 40)], &UnitScales::default());
        assert_eq!(kind(&wide), Kind::Float64);
        let j = wide.columns.iter().position(|c| c.id == "P__n").unwrap();
        assert_eq!(wide.rows[1][j], Some(Value::Real((1i64 << 40) as f64)));
        // A value equal to noData (i32::MIN) would be indistinguishable, so FLOAT64 is used
        let min = Table::build(&[e(1, i64::from(i32::MIN))], &UnitScales::default());
        assert_eq!(kind(&min), Kind::Float64);
        let max = Table::build(&[e(1, i64::from(i32::MAX))], &UnitScales::default());
        assert_eq!(kind(&max), Kind::Int32);
    }

    #[test]
    fn missing_values_use_no_data() {
        let t = Table::build(
            &[
                element(
                    1,
                    vec![
                        prop("P", "f", Value::Real(1.5), None),
                        prop("P", "b", Value::Logical(Logical::True), None),
                        prop("P", "i", Value::Int(7), None),
                    ],
                ),
                element(2, vec![]),
            ],
            &UnitScales::default(),
        );
        let mut views: Vec<Vec<u8>> = Vec::new();
        let (class, _) = t.encode(&[0, 1], &mut |b| {
            views.push(b);
            views.len() - 1
        });
        assert_eq!(class["properties"]["P__i"]["componentType"], "INT32");
        assert_eq!(class["properties"]["P__i"]["noData"], NO_DATA_I32);
        assert!(views.iter().any(|v| v[..] == [7i32.to_le_bytes(), NO_DATA_I32.to_le_bytes()].concat()));
        assert_eq!(class["properties"]["P__b"]["noData"], "NOT_SET");
        let f = views.iter().find(|v| v.len() == 16 && v[8..] == NO_DATA_F64.to_le_bytes()).is_some();
        assert!(f);
        assert!(views.iter().any(|v| v == &[1, LOGICAL_NOT_SET]));
    }
}
