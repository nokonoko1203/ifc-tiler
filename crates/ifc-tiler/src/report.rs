//! 変換レポート（`ifc-tiler-report.json`）。

use std::collections::BTreeMap;

use serde_json::{Value, json};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TileReport {
    pub uri: String,
    pub features: usize,
    pub bytes: usize,
    /// primitiveの数（描画呼び出しの目安）。
    pub primitives: usize,
    /// インスタンスとして置いたメッシュの数。
    pub instances: usize,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub input: String,
    pub schema: String,
    pub elements: usize,
    pub storeys: usize,
    pub columns: usize,
    pub excluded: BTreeMap<String, usize>,
    pub without_mesh: BTreeMap<String, usize>,
    pub tiles: Vec<TileReport>,
    /// 変換の経緯（tileset.jsonの`asset.extras.ifc_tiler`にも入れる）。
    pub conversion: Value,
    pub warnings: Vec<String>,
    pub timing_ms: BTreeMap<&'static str, u128>,
}

impl Report {
    pub fn total_bytes(&self) -> usize {
        self.tiles.iter().map(|t| t.bytes).sum()
    }

    pub fn to_json(&self) -> Value {
        json!({
            "input": self.input,
            "schema": self.schema,
            "elements": self.elements,
            "storeys": self.storeys,
            "columns": self.columns,
            "excluded": self.excluded,
            "withoutMesh": self.without_mesh,
            "tiles": self.tiles.iter().map(|t| json!({
                "uri": t.uri, "features": t.features, "bytes": t.bytes, "primitives": t.primitives, "instances": t.instances,
            })).collect::<Vec<_>>(),
            "totalBytes": self.total_bytes(),
            "conversion": self.conversion,
            "warnings": self.warnings,
            "timingMs": self.timing_ms,
        })
    }
}
