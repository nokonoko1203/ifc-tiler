//! 1タイル分のGLB（glTF 2.0バイナリ）を作る。
//!
//! 直接置くメッシュは、不透明・半透明の2つのprimitiveにまとめ、色を`COLOR_0`、部材を`_FEATURE_ID_0`で
//! 区別する。タイル内で同じ形（平行移動だけ違う）のメッシュが多ければ、テンプレート1つと
//! `EXT_mesh_gpu_instancing`のインスタンスにする。既定では、同じ頂点を溶接し、位置をUINT16に、法線を
//! INT8に量子化し（法線はOCTAHEDRALフィルタ）、`EXT_meshopt_compression`で符号化する
//! （量子化の刻みは最大タイルで0.6 mm程度）。

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};

use meshopt::{
    encode_index_buffer, encode_vertex_buffer, generate_vertex_remap, optimize_vertex_cache_in_place,
    optimize_vertex_fetch, remap_index_buffer, remap_vertex_buffer,
};
use serde_json::{Map, Value, json};

use crate::metadata::{LOGICAL_ENUM_ID, Table, logical_enum};
use crate::tiling::Aabb;

// meshopt crateはフィルタの符号化を公開していないため、同梱のmeshoptimizer（vertexfilter.cpp）を直接呼ぶ
unsafe extern "C" {
    fn meshopt_encodeFilterOct(destination: *mut c_void, count: usize, stride: usize, bits: i32, data: *const f32);
}

/// インスタンス化する同形メッシュの最小個数と、テンプレートの最小頂点数。
/// 小さな形状まで分けると、node・accessorのJSONと描画呼び出しが増えて逆効果になる（実データでの計測）。
pub const INSTANCE_MIN_COPIES: usize = 3;
pub const INSTANCE_MIN_VERTICES: usize = 200;

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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Encoding {
    /// 量子化とmeshopt圧縮。
    pub compress: bool,
    /// 同形メッシュのインスタンス化。
    pub instancing: bool,
}

/// 1タイルの集計（レポート用）。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileStats {
    /// primitiveの数（描画呼び出しの目安）。
    pub primitives: usize,
    /// インスタンスとして置いたメッシュの数。
    pub instances: usize,
}

const UNSIGNED_BYTE: u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_INT: u32 = 5125;
const FLOAT: u32 = 5126;
const BYTE: u32 = 5120;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;

/// 8ビットに丸めたαがこれ以下なら半透明（`BLEND`）とする（0.99 × 255）。
const OPAQUE_MIN_ALPHA: u8 = 253;

/// primitive 1つ分の頂点（glTFのY上座標）。
#[derive(Default)]
struct Group {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    colors: Vec<[u8; 4]>,
    features: Vec<f32>,
    indices: Vec<u32>,
}

/// テンプレート1つと、そのインスタンス（部材番号、ENUでの最小点）。
struct Instanced {
    template: usize,
    instances: Vec<(u32, [f64; 3])>,
}

/// GLBのバイト列を作る。
pub fn write(meshes: &[TileMesh], meta: &TileMetadata, enc: Encoding) -> (Vec<u8>, TileStats) {
    let (instanced, direct) =
        if enc.instancing { find_instances(meshes) } else { (Vec::new(), (0..meshes.len()).collect()) };
    let groups = group_by_opacity(meshes, &direct);
    let mut w = Writer::default();
    let mut materials = Materials::default();
    let mut gltf_meshes = Vec::new();
    let mut nodes = Vec::new();
    let mut stats = TileStats::default();

    if !groups.is_empty() {
        // 位置の量子化は、タイル全体で1つの一様な倍率にする（非一様だと法線がゆがむ）
        let bounds = groups.iter().flat_map(|(_, g)| &g.positions).fold(Aabb::EMPTY, |mut b, p| {
            b.add(p.map(f64::from));
            b
        });
        let dequant = enc.compress.then(|| quantization(&bounds));
        let feature_ids = json!({ "EXT_mesh_features": { "featureIds": [{
            "featureCount": meta.rows.len(), "attribute": 0, "propertyTable": 0, "label": "element"
        }] } });
        let mut primitives = Vec::new();
        for (translucent, g) in &groups {
            let mut p = match dequant {
                Some((min, step)) => w.quantized(g, min, step),
                None => w.float(g),
            };
            p.insert("material".into(), materials.get(MaterialKey::VertexColor { translucent: *translucent }).into());
            p.insert("extensions".into(), feature_ids.clone());
            primitives.push(Value::Object(p));
        }
        let mut node = json!({ "mesh": 0 });
        if let Some((min, step)) = dequant {
            node["matrix"] = json!([step, 0, 0, 0, 0, step, 0, 0, 0, 0, step, 0, min[0], min[1], min[2], 1]);
        }
        stats.primitives += primitives.len();
        gltf_meshes.push(json!({ "primitives": primitives }));
        nodes.push(node);
    }

    for inst in &instanced {
        let m = &meshes[inst.template];
        let origin = inst.instances[0].1;
        let g = template_group(m, origin);
        let (mut p, offset, scale) = if enc.compress {
            let bounds = g.positions.iter().fold(Aabb::EMPTY, |mut b, p| {
                b.add(p.map(f64::from));
                b
            });
            let (min, step) = quantization(&bounds);
            (w.quantized(&g, min, step), min, Some(step))
        } else {
            (w.float(&g), [0.0; 3], None)
        };
        p.insert("material".into(), materials.get(MaterialKey::Color(color_key(m.color))).into());
        // インスタンスの平行移動（Y上）＝最小点＋テンプレートの量子化の原点。圧縮時は刻みを倍率にする
        let translation: Vec<f32> = inst
            .instances
            .iter()
            .flat_map(|&(_, q)| [q[0] + offset[0], q[2] + offset[1], -q[1] + offset[2]].map(|v| v as f32))
            .collect();
        let features: Vec<f32> = inst.instances.iter().map(|&(f, _)| f as f32).collect();
        let n = inst.instances.len();
        let mut attributes = Map::new();
        attributes.insert("TRANSLATION".into(), w.instance_accessor(&translation, n, "VEC3").into());
        if let Some(step) = scale {
            let s = vec![step as f32; 3 * n];
            attributes.insert("SCALE".into(), w.instance_accessor(&s, n, "VEC3").into());
        }
        attributes.insert("_FEATURE_ID_0".into(), w.instance_accessor(&features, n, "SCALAR").into());
        nodes.push(json!({
            "mesh": gltf_meshes.len(),
            "extensions": {
                "EXT_mesh_gpu_instancing": { "attributes": attributes },
                "EXT_instance_features": { "featureIds": [{
                    "featureCount": meta.rows.len(), "attribute": 0, "propertyTable": 0, "label": "element"
                }] }
            }
        }));
        gltf_meshes.push(json!({ "primitives": [Value::Object(p)] }));
        stats.primitives += 1;
        stats.instances += n;
    }

    // メタデータ（非圧縮のままbuffer 0へ）
    let (class, property_table) = meta.table.encode(meta.rows, &mut |bytes| w.plain_view(bytes));
    let mut schema = json!({ "id": meta.schema_id, "classes": { "element": class } });
    if meta.table.uses_logical() {
        schema["enums"] = json!({ LOGICAL_ENUM_ID: logical_enum() });
    }

    let mut used = vec!["EXT_mesh_features", "EXT_structural_metadata"];
    let mut required = Vec::new();
    if enc.compress {
        used.extend(["EXT_meshopt_compression", "KHR_mesh_quantization"]);
        required.extend(["EXT_meshopt_compression", "KHR_mesh_quantization"]);
    }
    if !instanced.is_empty() {
        used.extend(["EXT_mesh_gpu_instancing", "EXT_instance_features"]);
        required.push("EXT_mesh_gpu_instancing");
    }
    let mut buffers = vec![json!({ "byteLength": w.bin.len() })];
    if enc.compress {
        buffers.push(
            json!({ "byteLength": w.fallback_len, "extensions": { "EXT_meshopt_compression": { "fallback": true } } }),
        );
    }
    let mut gltf = json!({
        "asset": { "version": "2.0", "generator": concat!("ifc-tiler ", env!("CARGO_PKG_VERSION")) },
        "extensionsUsed": used,
        "scene": 0,
        "scenes": [{ "nodes": (0..nodes.len()).collect::<Vec<_>>() }],
        "nodes": nodes,
        "meshes": gltf_meshes,
        "materials": materials.list,
        "accessors": w.accessors,
        "bufferViews": w.views,
        "buffers": buffers,
        "extensions": { "EXT_structural_metadata": { "schema": schema, "propertyTables": [property_table] } },
    });
    if !required.is_empty() {
        gltf["extensionsRequired"] = json!(required);
    }
    (pack(&gltf, &w.bin), stats)
}

/// 外接箱から、一様な量子化の原点と刻み（最大辺 ÷ 65535）を決める。
fn quantization(bounds: &Aabb) -> ([f64; 3], f64) {
    let extent = bounds.size().into_iter().fold(0.001, f64::max);
    (bounds.min, extent / 65535.0)
}

fn color_key(c: [f32; 4]) -> [u8; 4] {
    c.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// 同じ形か比べるための量子化（最小点からの相対座標を1 mm、法線を0.01の刻みに丸める）。
struct Shape {
    color: [u8; 4],
    min: [f64; 3],
    positions: Vec<[i64; 3]>,
    normals: Vec<[i32; 3]>,
}

impl Shape {
    fn of(m: &TileMesh) -> Self {
        let min = m.positions.iter().fold([f64::INFINITY; 3], |a, p| std::array::from_fn(|k| a[k].min(p[k])));
        Self {
            color: color_key(m.color),
            min,
            positions: m
                .positions
                .iter()
                .map(|p| std::array::from_fn(|k| ((p[k] - min[k]) * 1000.0).round() as i64))
                .collect(),
            normals: m.normals.iter().map(|n| n.map(|v| (v * 100.0).round() as i32)).collect(),
        }
    }

    fn hash(&self, indices: &[u32]) -> u64 {
        let mut h = DefaultHasher::new();
        self.color.hash(&mut h);
        self.positions.hash(&mut h);
        self.normals.hash(&mut h);
        indices.hash(&mut h);
        h.finish()
    }

    fn same(&self, a: &TileMesh, o: &Self, b: &TileMesh) -> bool {
        self.color == o.color && a.indices == b.indices && self.positions == o.positions && self.normals == o.normals
    }
}

/// インスタンス化するメッシュの組と、直接置くメッシュの番号に分ける。
fn find_instances(meshes: &[TileMesh]) -> (Vec<Instanced>, Vec<usize>) {
    let shapes: Vec<Shape> = meshes.iter().map(Shape::of).collect();
    // ハッシュで候補を集め、量子化した値を比べて同値類に分ける（出現順を保つ）
    let mut buckets: HashMap<u64, Vec<usize>> = HashMap::new();
    let mut classes: Vec<Vec<usize>> = Vec::new();
    for (i, s) in shapes.iter().enumerate() {
        let bucket = buckets.entry(s.hash(meshes[i].indices)).or_default();
        match bucket.iter().find(|&&c| {
            let r = classes[c][0];
            shapes[r].same(&meshes[r], s, &meshes[i])
        }) {
            Some(&c) => classes[c].push(i),
            None => {
                bucket.push(classes.len());
                classes.push(vec![i]);
            }
        }
    }
    let mut instanced = Vec::new();
    let mut direct = Vec::new();
    for class in classes {
        let first = class[0];
        if class.len() >= INSTANCE_MIN_COPIES && meshes[first].positions.len() >= INSTANCE_MIN_VERTICES {
            let instances = class.iter().map(|&i| (meshes[i].feature, shapes[i].min)).collect();
            instanced.push(Instanced { template: first, instances });
        } else {
            direct.extend(class);
        }
    }
    direct.sort_unstable();
    (instanced, direct)
}

/// 直接置くメッシュを、不透明・半透明の2つにまとめる（この順）。
fn group_by_opacity(meshes: &[TileMesh], direct: &[usize]) -> Vec<(bool, Group)> {
    let mut opaque = Group::default();
    let mut translucent = Group::default();
    for &i in direct {
        let m = &meshes[i];
        let color = color_key(m.color);
        let g = if color[3] < OPAQUE_MIN_ALPHA { &mut translucent } else { &mut opaque };
        let base = u32::try_from(g.positions.len()).expect("1タイルの頂点数がu32に収まる");
        // z上（ENU）→ glTFのY上: (x, y, z) → (x, z, −y)
        g.positions.extend(m.positions.iter().map(|p| [p[0] as f32, p[2] as f32, -p[1] as f32]));
        g.normals.extend(m.normals.iter().map(|n| normalize([n[0], n[2], -n[1]])));
        g.colors.extend(std::iter::repeat_n(color, m.positions.len()));
        g.features.extend(std::iter::repeat_n(m.feature as f32, m.positions.len()));
        g.indices.extend(m.indices.iter().map(|i| base + i));
    }
    [(false, opaque), (true, translucent)].into_iter().filter(|(_, g)| !g.positions.is_empty()).collect()
}

/// テンプレートの頂点（`origin`を原点にしたY上座標）。色は材料で与えるため持たない。
fn template_group(m: &TileMesh, origin: [f64; 3]) -> Group {
    let rel = |p: &[f64; 3]| [(p[0] - origin[0]) as f32, (p[2] - origin[2]) as f32, (origin[1] - p[1]) as f32];
    Group {
        positions: m.positions.iter().map(rel).collect(),
        normals: m.normals.iter().map(|n| normalize([n[0], n[2], -n[1]])).collect(),
        colors: Vec::new(),
        features: Vec::new(),
        indices: m.indices.to_vec(),
    }
}

fn normalize(n: [f32; 3]) -> [f32; 3] {
    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    if l > 1e-12 { n.map(|v| v / l) } else { [0.0, 1.0, 0.0] }
}

/// 材料の種類。直接置くメッシュは係数が白で色は`COLOR_0`、テンプレートは係数がその色。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum MaterialKey {
    VertexColor { translucent: bool },
    Color([u8; 4]),
}

#[derive(Default)]
struct Materials {
    list: Vec<Value>,
    index: HashMap<MaterialKey, usize>,
}

impl Materials {
    fn get(&mut self, key: MaterialKey) -> usize {
        *self.index.entry(key).or_insert_with(|| {
            let mut m = json!({
                "pbrMetallicRoughness": { "metallicFactor": 0.0, "roughnessFactor": 0.9 },
                "doubleSided": true,
            });
            let blend = match key {
                MaterialKey::VertexColor { translucent } => translucent,
                MaterialKey::Color(c) => {
                    m["pbrMetallicRoughness"]["baseColorFactor"] = json!(c.map(|v| f32::from(v) / 255.0));
                    c[3] < OPAQUE_MIN_ALPHA
                }
            };
            if blend {
                m["alphaMode"] = "BLEND".into();
            }
            self.list.push(m);
            self.list.len() - 1
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FloatVertex {
    p: [f32; 3],
    n: [f32; 3],
    c: [u8; 4],
    f: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct QuantVertex {
    p: [u16; 4],
    n: [i8; 4],
    c: [u8; 4],
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

/// INT8の法線（xyzと0）を、OCTAHEDRALフィルタの8ビット表現にする。
fn encode_oct(normals: &[[i8; 4]]) -> Vec<[i8; 4]> {
    let data: Vec<f32> = normals
        .iter()
        .flat_map(|n| [n[0], n[1], n[2]].map(|v| f32::from(v) / 127.0).into_iter().chain([0.0]))
        .collect();
    let mut out = vec![[0i8; 4]; normals.len()];
    // SAFETY: outは count×4 バイト、dataは count×4 個のf32。stride 4・8ビットはmeshoptimizerの許す組み合わせ
    unsafe { meshopt_encodeFilterOct(out.as_mut_ptr().cast(), out.len(), 4, 8, data.as_ptr()) };
    out
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

    /// 4バイト幅の頂点属性をmeshoptで符号化する。
    fn meshopt_attribute<T: Copy + Default>(&mut self, data: &[T]) -> usize {
        let encoded = encode_vertex_buffer(data).expect("meshoptの頂点符号化");
        self.meshopt_view(std::mem::size_of_val(data), encoded, size_of::<T>(), data.len(), "ATTRIBUTES")
    }

    fn accessor(&mut self, view: usize, component: u32, count: usize, ty: &str, extra: Value) -> usize {
        let mut a = json!({ "bufferView": view, "componentType": component, "count": count, "type": ty });
        if let (Some(a), Value::Object(e)) = (a.as_object_mut(), extra) {
            a.extend(e);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    /// インスタンスの属性（FLOAT）。件数が少ないため圧縮しない。
    fn instance_accessor(&mut self, values: &[f32], count: usize, ty: &str) -> usize {
        let view = self.plain_view(values.iter().flat_map(|v| v.to_le_bytes()).collect());
        self.accessor(view, FLOAT, count, ty, json!({}))
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

    /// 非圧縮のprimitive。`g`に色・部材番号があれば`COLOR_0`・`_FEATURE_ID_0`を書く。
    fn float(&mut self, g: &Group) -> Map<String, Value> {
        let verts: Vec<FloatVertex> = (0..g.positions.len())
            .map(|i| FloatVertex {
                p: g.positions[i],
                n: g.normals[i],
                c: g.colors.get(i).copied().unwrap_or_default(),
                f: g.features.get(i).copied().unwrap_or_default(),
            })
            .collect();
        let (v, idx) = weld(&verts, &g.indices);
        let (mut min, mut max) = ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]);
        for x in &v {
            for k in 0..3 {
                min[k] = min[k].min(x.p[k]);
                max[k] = max[k].max(x.p[k]);
            }
        }
        let n = v.len();
        let pos = self.vertex_view(v.iter().flat_map(|x| x.p.iter().flat_map(|c| c.to_le_bytes())).collect(), 12);
        let nor = self.vertex_view(v.iter().flat_map(|x| x.n.iter().flat_map(|c| c.to_le_bytes())).collect(), 12);
        let mut attributes = Map::new();
        attributes
            .insert("POSITION".into(), self.accessor(pos, FLOAT, n, "VEC3", json!({ "min": min, "max": max })).into());
        attributes.insert("NORMAL".into(), self.accessor(nor, FLOAT, n, "VEC3", json!({})).into());
        if !g.colors.is_empty() {
            let col = self.vertex_view(v.iter().flat_map(|x| x.c).collect(), 4);
            attributes.insert(
                "COLOR_0".into(),
                self.accessor(col, UNSIGNED_BYTE, n, "VEC4", json!({ "normalized": true })).into(),
            );
        }
        if !g.features.is_empty() {
            let fid = self.vertex_view(v.iter().flat_map(|x| x.f.to_le_bytes()).collect(), 4);
            attributes.insert("_FEATURE_ID_0".into(), self.accessor(fid, FLOAT, n, "SCALAR", json!({})).into());
        }
        let indices = self.indices(&idx, n, false);
        primitive(attributes, indices)
    }

    /// 量子化・meshoptのprimitive。位置は`min + step·q`で復元する（nodeの行列かインスタンスの倍率）。
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
            .map(|i| QuantVertex {
                p: q(g.positions[i]),
                n: qn(g.normals[i]),
                c: g.colors.get(i).copied().unwrap_or_default(),
                f: g.features.get(i).copied().unwrap_or_default(),
            })
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
        let positions: Vec<[u16; 4]> = v.iter().map(|x| x.p).collect();
        let pos = self.meshopt_attribute(&positions);
        let normals = encode_oct(&v.iter().map(|x| x.n).collect::<Vec<_>>());
        let nor = self.meshopt_attribute(&normals);
        self.views[nor]["extensions"]["EXT_meshopt_compression"]["filter"] = "OCTAHEDRAL".into();
        let mut attributes = Map::new();
        attributes.insert(
            "POSITION".into(),
            self.accessor(pos, UNSIGNED_SHORT, n, "VEC3", json!({ "min": qmin, "max": qmax })).into(),
        );
        attributes.insert("NORMAL".into(), self.accessor(nor, BYTE, n, "VEC3", json!({ "normalized": true })).into());
        if !g.colors.is_empty() {
            let col = self.meshopt_attribute(&v.iter().map(|x| x.c).collect::<Vec<_>>());
            attributes.insert(
                "COLOR_0".into(),
                self.accessor(col, UNSIGNED_BYTE, n, "VEC4", json!({ "normalized": true })).into(),
            );
        }
        if !g.features.is_empty() {
            let fid = self.meshopt_attribute(&v.iter().map(|x| x.f).collect::<Vec<_>>());
            attributes.insert("_FEATURE_ID_0".into(), self.accessor(fid, FLOAT, n, "SCALAR", json!({})).into());
        }
        let indices = self.indices(&idx, n, true);
        primitive(attributes, indices)
    }
}

fn primitive(attributes: Map<String, Value>, indices: usize) -> Map<String, Value> {
    let mut p = Map::new();
    p.insert("attributes".into(), Value::Object(attributes));
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

    unsafe extern "C" {
        fn meshopt_decodeFilterOct(buffer: *mut c_void, count: usize, stride: usize);
    }

    type Mesh = (Vec<[f64; 3]>, Vec<[f32; 3]>, Vec<u32>);

    /// 1辺1 mの立方体（面ごとに頂点を持つ。ifc-liteと同じフラットシェーディング）を`o`に置く。
    fn cube(o: [f64; 3]) -> Mesh {
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
            let c: [f64; 3] = std::array::from_fn(|k| 0.5 + 0.5 * nn[k] - 0.5 * u[k] - 0.5 * v[k]);
            let b = p.len() as u32;
            for (a, s) in [(0., 0.), (1., 0.), (1., 1.), (0., 1.)] {
                p.push(std::array::from_fn(|k| o[k] + c[k] + a * u[k] + s * v[k]));
                n.push(nn.map(|x| x as f32));
            }
            idx.extend([b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        (p, n, idx)
    }

    /// 15×15頂点（225頂点、インスタンス化の最小頂点数以上）の、z上向きの格子を`o`に置く。
    fn grid(o: [f64; 3]) -> Mesh {
        let k = 15u32;
        let p = (0..k * k).map(|i| [o[0] + f64::from(i % k) * 0.1, o[1] + f64::from(i / k) * 0.1, o[2]]).collect();
        let n = vec![[0.0, 0.0, 1.0]; (k * k) as usize];
        let mut idx = Vec::new();
        for y in 0..k - 1 {
            for x in 0..k - 1 {
                let a = y * k + x;
                idx.extend([a, a + 1, a + k + 1, a, a + k + 1, a + k]);
            }
        }
        (p, n, idx)
    }

    fn read(glb: &[u8]) -> (Value, &[u8]) {
        assert_eq!(&glb[0..4], b"glTF");
        let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
        (serde_json::from_slice(&glb[20..20 + jlen]).unwrap(), &glb[20 + jlen + 8..])
    }

    fn table(n: u32) -> Table {
        let e = |id| ElementRecord { express_id: id, ifc_class: "IfcWall".into(), ..Default::default() };
        Table::build(&(1..=n).map(e).collect::<Vec<_>>(), &UnitScales::default(), true)
    }

    fn build(meshes: &[(Mesh, [f32; 4])], enc: Encoding) -> (Vec<u8>, TileStats) {
        let tm: Vec<TileMesh> = meshes
            .iter()
            .enumerate()
            .map(|(i, ((p, n, idx), c))| TileMesh {
                feature: i as u32,
                color: *c,
                positions: p,
                normals: n,
                indices: idx,
            })
            .collect();
        let t = table(meshes.len() as u32);
        let rows: Vec<usize> = (0..meshes.len()).collect();
        write(&tm, &TileMetadata { table: &t, rows: &rows, schema_id: "t" }, enc)
    }

    const RED: [f32; 4] = [1.0, 0.0, 0.0, 1.0];
    const GREY: [f32; 4] = [0.5, 0.5, 0.5, 1.0];
    const GLASS: [f32; 4] = [0.5, 0.5, 0.5, 0.5];
    const PLAIN: Encoding = Encoding { compress: false, instancing: true };
    const PACKED: Encoding = Encoding { compress: true, instancing: true };

    fn u(v: &Value) -> usize {
        v.as_u64().unwrap() as usize
    }

    /// bufferViewのバイト列（meshoptなら復号し、OCTAHEDRALフィルタも戻す）。
    fn view(js: &Value, bin: &[u8], i: usize) -> Vec<u8> {
        let v = &js["bufferViews"][i];
        let Some(m) = v["extensions"].get("EXT_meshopt_compression") else {
            let o = u(&v["byteOffset"]);
            return bin[o..o + u(&v["byteLength"])].to_vec();
        };
        let data = &bin[u(&m["byteOffset"])..u(&m["byteOffset"]) + u(&m["byteLength"])];
        let count = u(&m["count"]);
        match (m["mode"].as_str().unwrap(), u(&m["byteStride"])) {
            ("ATTRIBUTES", 8) => meshopt::decode_vertex_buffer::<[u8; 8]>(data, count).unwrap().concat(),
            ("ATTRIBUTES", 4) => {
                let mut out = meshopt::decode_vertex_buffer::<[u8; 4]>(data, count).unwrap();
                if m["filter"] == "OCTAHEDRAL" {
                    // SAFETY: outは count×4 バイト
                    unsafe { meshopt_decodeFilterOct(out.as_mut_ptr().cast(), count, 4) };
                }
                out.concat()
            }
            ("TRIANGLES", 2) => {
                meshopt::decode_index_buffer::<u16>(data, count).unwrap().iter().flat_map(|x| x.to_le_bytes()).collect()
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn colors_become_vertex_colors_in_two_primitives() {
        let meshes = [(cube([0., 0., 0.]), RED), (cube([2., 0., 0.]), GREY), (cube([4., 0., 0.]), GLASS)];
        let (glb, stats) = build(&meshes, PLAIN);
        let (js, _) = read(&glb);
        assert_eq!(glb.len() % 4, 0);
        let prims = js["meshes"][0]["primitives"].as_array().unwrap();
        assert_eq!(prims.len(), 2);
        assert_eq!(stats, TileStats { primitives: 2, instances: 0 });
        assert!(prims.iter().all(|p| p["attributes"].get("COLOR_0").is_some()));
        // 不透明（赤・灰）が先、半透明が後。材料は白で、半透明はBLEND
        assert_eq!(js["accessors"][u(&prims[0]["attributes"]["POSITION"])]["count"], 48);
        assert!(js["materials"][0]["pbrMetallicRoughness"].get("baseColorFactor").is_none());
        assert_eq!(js["materials"][u(&prims[1]["material"])]["alphaMode"], "BLEND");
        assert!(js.get("extensionsRequired").is_none());
        // 位置はY上。z上の(0..1)がglTFのyに、−y（−1..0）がzに来る
        assert_eq!(js["accessors"][u(&prims[0]["attributes"]["POSITION"])]["min"], json!([0.0, 0.0, -1.0]));
        for v in js["bufferViews"].as_array().unwrap() {
            assert_eq!(u(&v["byteOffset"]) % 8, 0);
        }
        assert_eq!(prims[0]["extensions"]["EXT_mesh_features"]["featureIds"][0]["featureCount"], 3);
    }

    #[test]
    fn compressed_positions_and_octahedral_normals_round_trip() {
        let (glb, _) = build(&[(cube([10., 10., 10.]), RED)], PACKED);
        let (js, bin) = read(&glb);
        assert_eq!(js["extensionsRequired"], json!(["EXT_meshopt_compression", "KHR_mesh_quantization"]));
        let m: Vec<f64> = js["nodes"][0]["matrix"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
        let prim = &js["meshes"][0]["primitives"][0];
        let pos = view(&js, bin, u(&js["accessors"][u(&prim["attributes"]["POSITION"])]["bufferView"]));
        let nor_view = u(&js["accessors"][u(&prim["attributes"]["NORMAL"])]["bufferView"]);
        assert_eq!(js["bufferViews"][nor_view]["extensions"]["EXT_meshopt_compression"]["filter"], "OCTAHEDRAL");
        let nor = view(&js, bin, nor_view);
        for k in 0..24 {
            let q = |c: usize| f64::from(u16::from_le_bytes(pos[8 * k + 2 * c..8 * k + 2 * c + 2].try_into().unwrap()));
            // 復元した座標（Y上）が元の立方体の範囲（x: 10〜11、z: −11〜−10）に0.1 mm以内で収まる
            let x = m[0] * q(0) + m[12];
            let z = m[10] * q(2) + m[14];
            assert!((9.9999..=11.0001).contains(&x) && (-11.0001..=-9.9999).contains(&z), "{x} {z}");
            // 法線は軸方向の単位ベクトル（INT8で±127）
            let n: Vec<i32> = (0..3).map(|c| i32::from(nor[4 * k + c] as i8)).collect();
            assert_eq!(n.iter().map(|v| v.abs()).max(), Some(127), "{n:?}");
            assert_eq!(n.iter().filter(|v| **v == 0).count(), 2, "{n:?}");
        }
        assert_eq!(js["buffers"][1]["extensions"]["EXT_meshopt_compression"]["fallback"], true);
    }

    #[test]
    fn three_copies_of_a_large_shape_are_instanced() {
        let origins = [[0., 0., 0.], [5., 0., 0.], [0., 5., 3.]];
        let mut meshes: Vec<(Mesh, [f32; 4])> = origins.iter().map(|&o| (grid(o), GREY)).collect();
        meshes.push((cube([9., 9., 9.]), RED));
        for enc in [PLAIN, PACKED] {
            let (glb, stats) = build(&meshes, enc);
            let (js, bin) = read(&glb);
            assert_eq!(stats, TileStats { primitives: 2, instances: 3 }, "{enc:?}");
            let node = &js["nodes"][1];
            let ext = &node["extensions"]["EXT_mesh_gpu_instancing"]["attributes"];
            assert_eq!(js["extensionsRequired"].as_array().unwrap().last().unwrap(), "EXT_mesh_gpu_instancing");
            assert_eq!(node["extensions"]["EXT_instance_features"]["featureIds"][0]["featureCount"], 4);
            let f32s = |a: &Value| -> Vec<f32> {
                view(&js, bin, u(&js["accessors"][u(a)]["bufferView"]))
                    .chunks_exact(4)
                    .map(|c| f32::from_le_bytes(c.try_into().unwrap()))
                    .collect()
            };
            assert_eq!(f32s(&ext["_FEATURE_ID_0"]), vec![0.0, 1.0, 2.0]);
            let t = f32s(&ext["TRANSLATION"]);
            let s = if enc.compress { f64::from(f32s(&ext["SCALE"])[0]) } else { 1.0 };
            // テンプレートの全頂点をインスタンスの変換で戻した外接箱が、元の各格子（Y上）の外接箱と一致する
            let prim = &js["meshes"][u(&node["mesh"])]["primitives"][0];
            let pa = &js["accessors"][u(&prim["attributes"]["POSITION"])];
            let pos = view(&js, bin, u(&pa["bufferView"]));
            let stride = if enc.compress { 8 } else { 12 };
            let local: Vec<[f64; 3]> = pos
                .chunks_exact(stride)
                .map(|v| {
                    std::array::from_fn(|c| {
                        if enc.compress {
                            f64::from(u16::from_le_bytes(v[2 * c..2 * c + 2].try_into().unwrap()))
                        } else {
                            f64::from(f32::from_le_bytes(v[4 * c..4 * c + 4].try_into().unwrap()))
                        }
                    })
                })
                .collect();
            for (i, o) in origins.iter().enumerate() {
                let placed: Vec<[f64; 3]> =
                    local.iter().map(|q| std::array::from_fn(|c| f64::from(t[3 * i + c]) + s * q[c])).collect();
                let (want, _, _) = grid(*o);
                let want: Vec<[f64; 3]> = want.iter().map(|q| [q[0], q[2], -q[1]]).collect();
                let bbox = |v: &[[f64; 3]]| {
                    v.iter().fold(([f64::MAX; 3], [f64::MIN; 3]), |(a, b), p| {
                        (std::array::from_fn(|c| a[c].min(p[c])), std::array::from_fn(|c| b[c].max(p[c])))
                    })
                };
                let (got, exp) = (bbox(&placed), bbox(&want));
                for c in 0..3 {
                    assert!(
                        (got.0[c] - exp.0[c]).abs() < 1e-3 && (got.1[c] - exp.1[c]).abs() < 1e-3,
                        "{enc:?} {got:?} {exp:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn two_copies_or_small_shapes_are_not_instanced() {
        let two: Vec<(Mesh, [f32; 4])> = [[0., 0., 0.], [5., 0., 0.]].iter().map(|&o| (grid(o), GREY)).collect();
        let small: Vec<(Mesh, [f32; 4])> = (0..5).map(|i| (cube([2.0 * f64::from(i), 0., 0.]), GREY)).collect();
        let recolored = vec![(grid([0., 0., 0.]), GREY), (grid([5., 0., 0.]), GREY), (grid([9., 0., 0.]), RED)];
        for meshes in [two, small, recolored] {
            let (glb, stats) = build(&meshes, PACKED);
            assert_eq!(stats.instances, 0);
            let (js, _) = read(&glb);
            assert!(!js["extensionsUsed"].as_array().unwrap().contains(&json!("EXT_mesh_gpu_instancing")));
        }
        let off = Encoding { instancing: false, ..PACKED };
        let three: Vec<(Mesh, [f32; 4])> = (0..3).map(|i| (grid([5.0 * f64::from(i), 0., 0.]), GREY)).collect();
        assert_eq!(build(&three, off).1.instances, 0);
    }
}
