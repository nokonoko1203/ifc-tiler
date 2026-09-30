//! Shared helpers for the integration tests: reads the output tileset back, including meshopt decoding

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn out_dir(name: &str) -> PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// An element read back from the output, with its properties and ECEF vertices
pub struct Feature {
    pub props: BTreeMap<String, Value>,
    pub ecef: Vec<[f64; 3]>,
}

pub fn read_tileset(out: &Path) -> Vec<Feature> {
    let ts: Value = serde_json::from_slice(&std::fs::read(out.join("tileset.json")).unwrap()).unwrap();
    let m: Vec<f64> = ts["root"]["transform"].as_array().unwrap().iter().map(|v| v.as_f64().unwrap()).collect();
    let mut uris = Vec::new();
    collect(&ts["root"], &mut uris);
    let mut features = Vec::new();
    for uri in uris {
        let glb = std::fs::read(out.join(uri)).unwrap();
        features.extend(read_glb(&glb, &m));
    }
    features
}

fn collect(tile: &Value, uris: &mut Vec<String>) {
    if let Some(u) = tile["content"]["uri"].as_str() {
        uris.push(u.to_string());
    }
    for c in tile["children"].as_array().into_iter().flatten() {
        collect(c, uris);
    }
}

unsafe extern "C" {
    // Filter decoding from the bundled meshoptimizer; the meshopt crate does not expose it
    fn meshopt_decodeFilterOct(buffer: *mut std::ffi::c_void, count: usize, stride: usize);
}

fn u(v: &Value) -> usize {
    v.as_u64().unwrap() as usize
}

/// Bytes of a bufferView, decoded if it uses meshopt
fn view(js: &Value, bin: &[u8], i: usize) -> Vec<u8> {
    let v = &js["bufferViews"][i];
    if let Some(m) = v["extensions"].get("EXT_meshopt_compression") {
        let data = &bin[u(&m["byteOffset"])..u(&m["byteOffset"]) + u(&m["byteLength"])];
        let (count, stride) = (u(&m["count"]), u(&m["byteStride"]));
        return match (m["mode"].as_str().unwrap(), stride) {
            ("ATTRIBUTES", 8) => meshopt::decode_vertex_buffer::<[u16; 4]>(data, count)
                .unwrap()
                .iter()
                .flat_map(|x| x.iter().flat_map(|c| c.to_le_bytes()))
                .collect(),
            ("ATTRIBUTES", 4) => {
                let mut out = meshopt::decode_vertex_buffer::<[u8; 4]>(data, count).unwrap();
                if m["filter"] == "OCTAHEDRAL" {
                    // SAFETY: out is count×4 bytes
                    unsafe { meshopt_decodeFilterOct(out.as_mut_ptr().cast(), count, 4) };
                }
                out.concat()
            }
            ("TRIANGLES", 2) => {
                meshopt::decode_index_buffer::<u16>(data, count).unwrap().iter().flat_map(|x| x.to_le_bytes()).collect()
            }
            ("TRIANGLES", 4) => {
                meshopt::decode_index_buffer::<u32>(data, count).unwrap().iter().flat_map(|x| x.to_le_bytes()).collect()
            }
            other => panic!("{other:?}"),
        };
    }
    let o = u(&v["byteOffset"]);
    bin[o..o + u(&v["byteLength"])].to_vec()
}

fn read_glb(glb: &[u8], m: &[f64]) -> Vec<Feature> {
    let jlen = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    let js: Value = serde_json::from_slice(&glb[20..20 + jlen]).unwrap();
    let bin = &glb[20 + jlen + 8..];
    let meta = &js["extensions"]["EXT_structural_metadata"];
    let class = &meta["schema"]["classes"]["element"]["properties"];
    let table = &meta["propertyTables"][0];
    let count = u(&table["count"]);
    let mut features: Vec<Feature> = (0..count).map(|_| Feature { props: BTreeMap::new(), ecef: Vec::new() }).collect();
    for (id, p) in table["properties"].as_object().unwrap() {
        let values = view(&js, bin, u(&p["values"]));
        let def = &class[id];
        for (i, f) in features.iter_mut().enumerate() {
            let v = match def["type"].as_str().unwrap() {
                "STRING" => {
                    let off = view(&js, bin, u(&p["stringOffsets"]));
                    let at = |k: usize| u32::from_le_bytes(off[4 * k..4 * k + 4].try_into().unwrap()) as usize;
                    let s = std::str::from_utf8(&values[at(i)..at(i + 1)]).unwrap();
                    if s.is_empty() { continue } else { Value::from(s) }
                }
                "ENUM" => {
                    Value::from(["FALSE", "TRUE", "UNKNOWN"].get(usize::from(values[i])).copied().unwrap_or("NOT_SET"))
                }
                _ => match def["componentType"].as_str().unwrap() {
                    "UINT32" => Value::from(u32::from_le_bytes(values[4 * i..4 * i + 4].try_into().unwrap())),
                    "FLOAT64" => Value::from(f64::from_le_bytes(values[8 * i..8 * i + 8].try_into().unwrap())),
                    "INT32" => Value::from(i32::from_le_bytes(values[4 * i..4 * i + 4].try_into().unwrap())),
                    other => panic!("{other}"),
                },
            };
            f.props.insert(id.clone(), v);
        }
    }
    // Visit every node. An instanced node applies translation + scale per instance, and its element index is an instance attribute
    for node in js["nodes"].as_array().unwrap() {
        let matrix: Vec<f64> = node["matrix"]
            .as_array()
            .map(|a| a.iter().map(|v| v.as_f64().unwrap()).collect())
            .unwrap_or_else(|| vec![1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]);
        let floats = |a: &Value| -> Vec<f64> {
            view(&js, bin, u(&js["accessors"][u(a)]["bufferView"]))
                .chunks_exact(4)
                .map(|c| f64::from(f32::from_le_bytes(c.try_into().unwrap())))
                .collect()
        };
        let inst = &node["extensions"]["EXT_mesh_gpu_instancing"]["attributes"];
        // (translation, scale, element index); a single entry if not instanced, where the element index is a vertex attribute
        let placements: Vec<([f64; 3], [f64; 3], Option<usize>)> = if inst.is_object() {
            let t = floats(&inst["TRANSLATION"]);
            let s = floats(&inst["SCALE"]);
            let f = floats(&inst["_FEATURE_ID_0"]);
            (0..f.len())
                .map(|i| {
                    (
                        [t[3 * i], t[3 * i + 1], t[3 * i + 2]],
                        [s[3 * i], s[3 * i + 1], s[3 * i + 2]],
                        Some(f[i] as usize),
                    )
                })
                .collect()
        } else {
            vec![([0.0; 3], [1.0; 3], None)]
        };
        for prim in js["meshes"][u(&node["mesh"])]["primitives"].as_array().unwrap() {
            let pa = &js["accessors"][u(&prim["attributes"]["POSITION"])];
            let pos = view(&js, bin, u(&pa["bufferView"]));
            let fid = prim["attributes"]
                .get("_FEATURE_ID_0")
                .map(|a| view(&js, bin, u(&js["accessors"][u(a)]["bufferView"])));
            let stride = js["bufferViews"][u(&pa["bufferView"])]["byteStride"].as_u64().unwrap() as usize;
            for k in 0..u(&pa["count"]) {
                // Positions are quantized to UINT16 (KHR_mesh_quantization)
                let q: [f64; 3] = std::array::from_fn(|c| {
                    let o = k * stride + 2 * c;
                    f64::from(u16::from_le_bytes(pos[o..o + 2].try_into().unwrap()))
                });
                let g: [f64; 3] = std::array::from_fn(|r| {
                    matrix[r] * q[0] + matrix[4 + r] * q[1] + matrix[8 + r] * q[2] + matrix[12 + r]
                });
                for (t, s, f) in &placements {
                    let p: [f64; 3] = std::array::from_fn(|c| t[c] + s[c] * g[c]);
                    let enu = [p[0], -p[2], p[1]]; // glTF Y-up → ENU
                    let e: [f64; 3] =
                        std::array::from_fn(|r| m[r] * enu[0] + m[4 + r] * enu[1] + m[8 + r] * enu[2] + m[12 + r]);
                    let f = f.unwrap_or_else(|| {
                        let b = fid.as_ref().expect("a primitive that is not instanced has an element index");
                        f32::from_le_bytes(b[4 * k..4 * k + 4].try_into().unwrap()) as usize
                    });
                    features[f].ecef.push(e);
                }
            }
        }
    }
    features
}
