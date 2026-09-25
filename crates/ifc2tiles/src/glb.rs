//! 1タイル分のGLB（glTF 2.0バイナリ）を作る。
//!
//! 色ごとに1つのprimitiveにまとめ、`_FEATURE_ID_0`で部材を区別する。既定では、同じ頂点を溶接し、
//! 位置をタイルの外接箱でUINT16に、法線をINT8に量子化してから`EXT_meshopt_compression`で符号化する
//! （実験で36.1 MB→6.3 MB。量子化の刻みは最大タイルで0.6 mm程度）。

use std::collections::HashMap;

use meshopt::{
    encode_index_buffer, encode_vertex_buffer, generate_vertex_remap, optimize_vertex_cache_in_place,
    optimize_vertex_fetch, remap_index_buffer, remap_vertex_buffer,
};
use serde_json::{Map, Value, json};

use crate::metadata::{LOGICAL_ENUM_ID, Table, logical_enum};
use crate::tiling::Aabb;

/// タイルに入れる部材1つ分のメッシュ（根のENU座標、z上）。
pub struct TileMesh<'a> {
    /// タイル内の部材番号（property tableの行）。
    pub feature: u32,
    pub color: [f32; 4],
    pub positions: &'a [[f64; 3]],
    pub normals: &'a [[f32; 3]],
    pub indices: &'a [u32],
}

/// メタデータの参照（`Table::encode`に渡す行番号とスキーマID）。
pub struct TileMetadata<'a> {
    pub table: &'a Table,
    pub rows: &'a [usize],
    pub schema_id: &'a str,
}

const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;
const BYTE: u32 = 5120;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// 同じ色の部材をまとめたprimitive（glTFのY上座標）。
struct Group {
    color: [f32; 4],
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    features: Vec<f32>,
    indices: Vec<u32>,
}

/// GLBのバイト列を作る。
pub fn write(meshes: &[TileMesh], meta: &TileMetadata, compress: bool) -> Vec<u8> {
    let groups = group_by_color(meshes);
    let mut w = Writer::default();

    // 位置の量子化は、タイル全体で1つの一様な倍率にする（非一様だと法線がゆがむ）
    let bounds = groups.iter().flat_map(|g| &g.positions).fold(Aabb::EMPTY, |mut b, p| {
        b.add(p.map(f64::from));
        b
    });
    let dequant = compress.then(|| {
        let extent = bounds.size().into_iter().fold(0.001, f64::max);
        (bounds.min, extent / 65535.0)
    });

    let mut primitives = Vec::new();
    let mut materials = Vec::new();
    for g in &groups {
        let attributes = match dequant {
            Some((min, step)) => w.quantized(g, min, step),
            None => w.float(g),
        };
        materials.push(material(g.color));
        let mut p = attributes;
        p.insert("material".into(), (materials.len() - 1).into());
        p.insert(
            "extensions".into(),
            json!({ "EXT_mesh_features": { "featureIds": [{
                "featureCount": meta.rows.len(), "attribute": 0, "propertyTable": 0, "label": "element"
            }] } }),
        );
        primitives.push(Value::Object(p));
    }

    // メタデータ（非圧縮のままbuffer 0へ）
    let (class, property_table) = meta.table.encode(meta.rows, &mut |bytes| w.plain_view(bytes));
    let mut schema = json!({ "id": meta.schema_id, "classes": { "element": class } });
    if meta.table.uses_logical() {
        schema["enums"] = json!({ LOGICAL_ENUM_ID: logical_enum() });
    }

    let mut node = json!({ "mesh": 0 });
    if let Some((min, step)) = dequant {
        node["matrix"] = json!([step, 0, 0, 0, 0, step, 0, 0, 0, 0, step, 0, min[0], min[1], min[2], 1]);
    }
    let mut used = vec!["EXT_mesh_features", "EXT_structural_metadata"];
    let mut required = Vec::new();
    if compress {
        used.extend(["EXT_meshopt_compression", "KHR_mesh_quantization"]);
        required.extend(["EXT_meshopt_compression", "KHR_mesh_quantization"]);
    }
    let mut buffers = vec![json!({ "byteLength": w.bin.len() })];
    if compress {
        buffers.push(
            json!({ "byteLength": w.fallback_len, "extensions": { "EXT_meshopt_compression": { "fallback": true } } }),
        );
    }
    let mut gltf = json!({
        "asset": { "version": "2.0", "generator": concat!("ifc2tiles ", env!("CARGO_PKG_VERSION")) },
        "extensionsUsed": used,
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [node],
        "meshes": [{ "primitives": primitives }],
        "materials": materials,
        "accessors": w.accessors,
        "bufferViews": w.views,
        "buffers": buffers,
        "extensions": { "EXT_structural_metadata": { "schema": schema, "propertyTables": [property_table] } },
    });
    if !required.is_empty() {
        gltf["extensionsRequired"] = json!(required);
    }
    pack(&gltf, &w.bin)
}

fn group_by_color(meshes: &[TileMesh]) -> Vec<Group> {
    let mut order: Vec<Group> = Vec::new();
    let mut index: HashMap<[u8; 4], usize> = HashMap::new();
    for m in meshes {
        let key = m.color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let gi = *index.entry(key).or_insert_with(|| {
            order.push(Group {
                color: key.map(|c| f32::from(c) / 255.0),
                positions: Vec::new(),
                normals: Vec::new(),
                features: Vec::new(),
                indices: Vec::new(),
            });
            order.len() - 1
        });
        let g = &mut order[gi];
        let base = u32::try_from(g.positions.len()).expect("1タイルの頂点数がu32に収まる");
        // z上（ENU）→ glTFのY上: (x, y, z) → (x, z, −y)
        g.positions.extend(m.positions.iter().map(|p| [p[0] as f32, p[2] as f32, -p[1] as f32]));
        g.normals.extend(m.normals.iter().map(|n| normalize([n[0], n[2], -n[1]])));
        g.features.extend(std::iter::repeat_n(m.feature as f32, m.positions.len()));
        g.indices.extend(m.indices.iter().map(|i| base + i));
    }
    order
}

fn normalize(n: [f32; 3]) -> [f32; 3] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 1e-12 { n.map(|v| v / l) } else { [0.0, 1.0, 0.0] }
}

fn material(c: [f32; 4]) -> Value {
    let mut m = json!({
        "pbrMetallicRoughness": { "baseColorFactor": c, "metallicFactor": 0.0, "roughnessFactor": 0.9 },
        "doubleSided": true,
    });
    if c[3] < 0.99 {
        m["alphaMode"] = "BLEND".into();
    }
    m
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FloatVertex {
    p: [f32; 3],
    n: [f32; 3],
    f: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QuantVertex {
    p: [u16; 4],
    n: [i8; 4],
    f: f32,
}

/// 同一頂点の溶接と、頂点キャッシュ・頂点フェッチの最適化。
fn weld<T: Copy + Default>(vertices: &[T], indices: &[u32]) -> (Vec<T>, Vec<u32>) {
    let (count, remap) = generate_vertex_remap(vertices, Some(indices));
    let mut idx = remap_index_buffer(Some(indices), count, &remap);
    let v = remap_vertex_buffer(vertices, count, &remap);
    optimize_vertex_cache_in_place(&mut idx, count);
    let v = optimize_vertex_fetch(&mut idx, &v);
    (v, idx)
}

#[derive(Default)]
struct Writer {
    bin: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
    /// meshoptのフォールバック（復号後）バッファの長さ。
    fallback_len: usize,
}

impl Writer {
    fn align_bin(&mut self) {
        while !self.bin.len().is_multiple_of(8) {
            self.bin.push(0);
        }
    }

    /// buffer 0 にそのまま置くbufferView。
    fn plain_view(&mut self, bytes: Vec<u8>) -> usize {
        self.align_bin();
        self.views.push(json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() }));
        self.bin.extend(bytes);
        self.views.len() - 1
    }

    fn vertex_view(&mut self, bytes: Vec<u8>, stride: usize) -> usize {
        let i = self.plain_view(bytes);
        self.views[i]["byteStride"] = stride.into();
        self.views[i]["target"] = ARRAY_BUFFER.into();
        i
    }

    /// meshoptで符号化したbufferView。`mode`は`ATTRIBUTES`か`TRIANGLES`。
    fn meshopt_view(&mut self, raw_len: usize, encoded: Vec<u8>, stride: usize, count: usize, mode: &str) -> usize {
        self.align_bin();
        let offset = self.bin.len();
        let len = encoded.len();
        self.bin.extend(encoded);
        let fallback_offset = self.fallback_len;
        self.fallback_len += raw_len.next_multiple_of(8);
        let mut v = json!({
            "buffer": 1, "byteOffset": fallback_offset, "byteLength": raw_len,
            "extensions": { "EXT_meshopt_compression": {
                "buffer": 0, "byteOffset": offset, "byteLength": len, "byteStride": stride, "count": count, "mode": mode
            } }
        });
        if mode == "ATTRIBUTES" {
            v["byteStride"] = stride.into();
            v["target"] = ARRAY_BUFFER.into();
        } else {
            v["target"] = ELEMENT_ARRAY_BUFFER.into();
        }
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, view: usize, component: u32, count: usize, ty: &str, extra: Value) -> usize {
        let mut a = json!({ "bufferView": view, "componentType": component, "count": count, "type": ty });
        if let (Some(a), Value::Object(e)) = (a.as_object_mut(), extra) {
            a.extend(e);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn indices(&mut self, idx: &[u32], vertex_count: usize, compress: bool) -> usize {
        let small = vertex_count <= usize::from(u16::MAX);
        let (component, size) = if small { (UNSIGNED_SHORT, 2) } else { (UNSIGNED_INT, 4) };
        let view = if compress {
            let encoded = encode_index_buffer(idx, vertex_count).expect("meshoptの索引符号化");
            self.meshopt_view(idx.len() * size, encoded, size, idx.len(), "TRIANGLES")
        } else {
            let bytes: Vec<u8> = if small {
                idx.iter().flat_map(|&i| (i as u16).to_le_bytes()).collect()
            } else {
                idx.iter().flat_map(|i| i.to_le_bytes()).collect()
            };
            let v = self.plain_view(bytes);
            self.views[v]["target"] = ELEMENT_ARRAY_BUFFER.into();
            v
        };
        self.accessor(view, component, idx.len(), "SCALAR", json!({}))
    }

    fn float(&mut self, g: &Group) -> Map<String, Value> {
        let verts: Vec<FloatVertex> = (0..g.positions.len())
            .map(|i| FloatVertex { p: g.positions[i], n: g.normals[i], f: g.features[i] })
            .collect();
        let (v, idx) = weld(&verts, &g.indices);
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for x in &v {
            for k in 0..3 {
                min[k] = min[k].min(x.p[k]);
                max[k] = max[k].max(x.p[k]);
            }
        }
        let pos = self.vertex_view(v.iter().flat_map(|x| x.p.iter().flat_map(|c| c.to_le_bytes())).collect(), 12);
        let nor = self.vertex_view(v.iter().flat_map(|x| x.n.iter().flat_map(|c| c.to_le_bytes())).collect(), 12);
        let fid = self.vertex_view(v.iter().flat_map(|x| x.f.to_le_bytes()).collect(), 4);
        let n = v.len();
        let attributes = json!({
            "POSITION": self.accessor(pos, FLOAT, n, "VEC3", json!({ "min": min, "max": max })),
            "NORMAL": self.accessor(nor, FLOAT, n, "VEC3", json!({})),
            "_FEATURE_ID_0": self.accessor(fid, FLOAT, n, "SCALAR", json!({})),
        });
        let indices = self.indices(&idx, n, false);
        primitive(attributes, indices)
    }

    fn quantized(&mut self, g: &Group, min: [f64; 3], step: f64) -> Map<String, Value> {
        let q = |p: [f32; 3]| -> [u16; 4] {
            let c = |k: usize| ((f64::from(p[k]) - min[k]) / step).round().clamp(0.0, 65535.0) as u16;
            [c(0), c(1), c(2), 0]
        };
        let qn = |n: [f32; 3]| -> [i8; 4] {
            let c = |v: f32| (v * 127.0).round().clamp(-127.0, 127.0) as i8;
            [c(n[0]), c(n[1]), c(n[2]), 0]
        };
        let verts: Vec<QuantVertex> = (0..g.positions.len())
            .map(|i| QuantVertex { p: q(g.positions[i]), n: qn(g.normals[i]), f: g.features[i] })
            .collect();
        let (v, idx) = weld(&verts, &g.indices);
        let (mut qmin, mut qmax) = ([u16::MAX; 3], [0u16; 3]);
        for x in &v {
            for k in 0..3 {
                qmin[k] = qmin[k].min(x.p[k]);
                qmax[k] = qmax[k].max(x.p[k]);
            }
        }
        let n = v.len();
        let pos_raw: Vec<[u16; 4]> = v.iter().map(|x| x.p).collect();
        let nor_raw: Vec<[i8; 4]> = v.iter().map(|x| x.n).collect();
        let fid_raw: Vec<f32> = v.iter().map(|x| x.f).collect();
        let enc = |r: Result<Vec<u8>, meshopt::Error>| r.expect("meshoptの頂点符号化");
        let pos = self.meshopt_view(n * 8, enc(encode_vertex_buffer(&pos_raw)), 8, n, "ATTRIBUTES");
        let nor = self.meshopt_view(n * 4, enc(encode_vertex_buffer(&nor_raw)), 4, n, "ATTRIBUTES");
        let fid = self.meshopt_view(n * 4, enc(encode_vertex_buffer(&fid_raw)), 4, n, "ATTRIBUTES");
        let attributes = json!({
            "POSITION": self.accessor(pos, UNSIGNED_SHORT, n, "VEC3", json!({ "min": qmin, "max": qmax })),
            "NORMAL": self.accessor(nor, BYTE, n, "VEC3", json!({ "normalized": true })),
            "_FEATURE_ID_0": self.accessor(fid, FLOAT, n, "SCALAR", json!({})),
        });
        let indices = self.indices(&idx, n, true);
        primitive(attributes, indices)
    }
}

fn primitive(attributes: Value, indices: usize) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("attributes".into(), attributes);
    p.insert("indices".into(), indices.into());
    p.insert("mode".into(), 4.into());
    p
}

/// JSONとバイナリをGLBコンテナに詰める。
fn pack(gltf: &Value, bin: &[u8]) -> Vec<u8> {
    let mut js = serde_json::to_vec(gltf).expect("glTFのJSON化");
    while !js.len().is_multiple_of(4) {
        js.push(b' ');
    }
    let mut bin = bin.to_vec();
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    let total = 12 + 8 + js.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    let u32le = |n: usize| u32::try_from(n).expect("GLBが4 GiBを超えない").to_le_bytes();
    out.extend(b"glTF");
    out.extend(2u32.to_le_bytes());
    out.extend(u32le(total));
    out.extend(u32le(js.len()));
    out.extend(b"JSON");
    out.extend(js);
    out.extend(u32le(bin.len()));
    out.extend(b"BIN\0");
    out.extend(bin);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::metadata::ElementRecord;
    use crate::units::UnitScales;

    fn cube() -> (Vec<[f64; 3]>, Vec<[f32; 3]>, Vec<u32>) {
        // 1面2三角形×6面、面ごとに頂点を持つ（ifc-liteと同じフラットシェーディング）
        let mut p = Vec::new();
        let mut n = Vec::new();
        let mut idx = Vec::new();
        let faces: [([f64; 3], [f64; 3], [f64; 3]); 6] = [
            ([0., 0., 1.], [1., 0., 0.], [0., 1., 0.]),
            ([0., 0., -1.], [0., 1., 0.], [1., 0., 0.]),
            ([1., 0., 0.], [0., 1., 0.], [0., 0., 1.]),
            ([-1., 0., 0.], [0., 0., 1.], [0., 1., 0.]),
            ([0., 1., 0.], [0., 0., 1.], [1., 0., 0.]),
            ([0., -1., 0.], [1., 0., 0.], [0., 0., 1.]),
        ];
        for (nn, u, v) in faces {
            let o: [f64; 3] = std::array::from_fn(|k| 0.5 + 0.5 * nn[k] - 0.5 * u[k] - 0.5 * v[k]);
            let b = p.len() as u32;
            for (a, c) in [(0., 0.), (1., 0.), (1., 1.), (0., 1.)] {
                p.push(std::array::from_fn(|k| 10.0 + o[k] + a * u[k] + c * v[k]));
                n.push(nn.map(|x| x as f32));
            }
            idx.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        (p, n, idx)
    }

    fn read(glb: &[u8]) -> Value {
        assert_eq!(&glb[0..4], b"glTF");
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        serde_json::from_slice(&glb[20..20 + jlen]).unwrap()
    }

    fn table() -> Table {
        let e = |id| ElementRecord { express_id: id, ifc_class: "IfcWall".into(), ..Default::default() };
        Table::build(&[e(1), e(2)], &UnitScales::default(), true)
    }

    fn build(compress: bool) -> (Vec<u8>, Value) {
        let (p, n, i) = cube();
        let meshes = [
            TileMesh { feature: 0, color: [1.0, 0.0, 0.0, 1.0], positions: &p, normals: &n, indices: &i },
            TileMesh { feature: 1, color: [0.5, 0.5, 0.5, 0.5], positions: &p, normals: &n, indices: &i },
        ];
        let t = table();
        let glb = write(&meshes, &TileMetadata { table: &t, rows: &[0, 1], schema_id: "t" }, compress);
        let js = read(&glb);
        (glb, js)
    }

    #[test]
    fn uncompressed_structure() {
        let (glb, js) = build(false);
        assert_eq!(glb.len() % 4, 0);
        assert_eq!(js["meshes"][0]["primitives"].as_array().unwrap().len(), 2);
        assert_eq!(js["materials"][1]["alphaMode"], "BLEND");
        assert!(js.get("extensionsRequired").is_none());
        // 位置はY上。z上の(10..11)がglTFのyに、−y（−11..−10）がzに来る
        let a = &js["accessors"][0];
        assert_eq!(a["min"], json!([10.0, 10.0, -11.0]));
        // フラットシェーディングなので溶接しても24頂点
        assert_eq!(a["count"], 24);
        for v in js["bufferViews"].as_array().unwrap() {
            assert_eq!(v["byteOffset"].as_u64().unwrap() % 8, 0);
        }
        let fid = &js["meshes"][0]["primitives"][0]["extensions"]["EXT_mesh_features"]["featureIds"][0];
        assert_eq!(fid["featureCount"], 2);
    }

    #[test]
    fn compressed_round_trip() {
        let (glb, js) = build(true);
        assert_eq!(js["extensionsRequired"], json!(["EXT_meshopt_compression", "KHR_mesh_quantization"]));
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        let bin = &glb[20 + jlen + 8..];
        let m = js["nodes"][0]["matrix"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect::<Vec<_>>();
        let prim = &js["meshes"][0]["primitives"][0];
        let acc = &js["accessors"][prim["attributes"]["POSITION"].as_u64().unwrap() as usize];
        let view =
            &js["bufferViews"][acc["bufferView"].as_u64().unwrap() as usize]["extensions"]["EXT_meshopt_compression"];
        let (off, len, count) = (
            view["byteOffset"].as_u64().unwrap() as usize,
            view["byteLength"].as_u64().unwrap() as usize,
            view["count"].as_u64().unwrap() as usize,
        );
        let pos: Vec<[u16; 4]> = meshopt::decode_vertex_buffer(&bin[off..off + len], count).unwrap();
        // 復元した座標が元の立方体の範囲（x: 10〜11）に0.1 mm以内で収まる
        let xs: Vec<f64> = pos.iter().map(|p| m[0] * f64::from(p[0]) + m[12]).collect();
        let (lo, hi) = xs.iter().fold((f64::MAX, f64::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        assert!((lo - 10.0).abs() < 1e-4 && (hi - 11.0).abs() < 1e-4, "{lo} {hi}");
        assert_eq!(js["buffers"][1]["extensions"]["EXT_meshopt_compression"]["fallback"], true);
    }
}
