//! 結合テストの共通部分: 出力のtilesetを読み戻す（meshoptの復号を含む）。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use ifc2tiles::Options;
use ifc2tiles::geodesy::GeoidModel;
use ifc2tiles::georef::{GeorefOptions, ScalePolicy, SiteCoords};
use serde_json::Value;

pub fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

pub fn options() -> Options {
    Options {
        georef: GeorefOptions {
            crs_epsg: None,
            site_coords: SiteCoords::Enu,
            origin: None,
            scale_policy: ScalePolicy::Auto,
        },
        geoid: GeoidModel::Jpgeo2024,
        max_features: 200,
        include_spaces: false,
        keep_parts: false,
        include_properties: true,
        compress: true,
    }
}

pub fn out_dir(name: &str) -> PathBuf {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&out);
    out
}

/// 出力を読み戻した部材（属性と、ECEFの頂点）。
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

fn u(v: &Value) -> usize {
    v.as_u64().unwrap() as usize
}

/// bufferViewのバイト列（meshoptなら復号する）。
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
            ("ATTRIBUTES", 4) => meshopt::decode_vertex_buffer::<[u8; 4]>(data, count).unwrap().concat(),
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
    let node: Vec<f64> = js["nodes"][0]["matrix"]
        .as_array()
        .map(|a| a.iter().map(|v| v.as_f64().unwrap()).collect())
        .unwrap_or_else(|| vec![1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.]);
    for prim in js["meshes"][0]["primitives"].as_array().unwrap() {
        let pa = &js["accessors"][u(&prim["attributes"]["POSITION"])];
        let fa = &js["accessors"][u(&prim["attributes"]["_FEATURE_ID_0"])];
        let pos = view(&js, bin, u(&pa["bufferView"]));
        let fid = view(&js, bin, u(&fa["bufferView"]));
        let stride = js["bufferViews"][u(&pa["bufferView"])]["byteStride"].as_u64().unwrap() as usize;
        for k in 0..u(&pa["count"]) {
            let q: [f64; 3] = std::array::from_fn(|c| match pa["componentType"].as_u64().unwrap() {
                5123 => {
                    f64::from(u16::from_le_bytes(pos[k * stride + 2 * c..k * stride + 2 * c + 2].try_into().unwrap()))
                }
                _ => f64::from(f32::from_le_bytes(pos[k * stride + 4 * c..k * stride + 4 * c + 4].try_into().unwrap())),
            });
            let g: [f64; 3] =
                std::array::from_fn(|r| node[r] * q[0] + node[4 + r] * q[1] + node[8 + r] * q[2] + node[12 + r]);
            let enu = [g[0], -g[2], g[1]]; // glTFのY上 → ENU
            let e: [f64; 3] =
                std::array::from_fn(|r| m[r] * enu[0] + m[4 + r] * enu[1] + m[8 + r] * enu[2] + m[12 + r]);
            let f = f32::from_le_bytes(fid[4 * k..4 * k + 4].try_into().unwrap()) as usize;
            features[f].ecef.push(e);
        }
    }
    features
}
