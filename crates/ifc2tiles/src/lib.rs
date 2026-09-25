//! IFCを部材情報付きの3D Tiles 1.1へ変換する。
//!
//! 処理の流れ（`convert`）:
//! `source`（IFCを読む）→ `semantics`（featureにする部材を決める）→ `georef`（置き方を決める）
//! → `geodesy`（ECEF→根のENUへ）→ `tiling`（タイルの木）→ `metadata` / `glb` / `tileset`（書き出し）。

pub mod geodesy;
pub mod georef;
pub mod glb;
pub mod metadata;
pub mod report;
pub mod semantics;
pub mod source;
pub mod tileset;
pub mod tiling;
pub mod units;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;
use std::time::Instant;

use serde_json::{Value, json};

use crate::geodesy::{Frame, GeoidModel, Projector, rotate};
use crate::georef::{GeorefOptions, Placement, Resolved};
use crate::glb::{TileMesh, TileMetadata};
use crate::metadata::Table;
use crate::report::{Report, TileReport};
use crate::semantics::SemanticsOptions;
use crate::tiling::{Aabb, Node};

#[derive(Clone, Debug, PartialEq)]
pub struct Options {
    pub georef: GeorefOptions,
    pub geoid: GeoidModel,
    pub max_features: usize,
    pub include_spaces: bool,
    pub keep_parts: bool,
    pub include_properties: bool,
    pub compress: bool,
}

#[derive(Debug)]
pub enum Error {
    /// 入力・設定の誤り（終了コード2）。
    Input(String),
    /// 変換できる部材がない（終了コード3）。
    NoElements(String),
    /// 書き出しの失敗など（終了コード1）。
    Io(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(m) | Self::NoElements(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for Error {}

fn io(path: &Path) -> impl Fn(std::io::Error) -> Error + '_ {
    move |e| Error::Io(format!("{}: {e}", path.display()))
}

/// `input`のIFCを変換し、`output`にtileset.json・tiles/*.glb・ifc2tiles-report.jsonを書く。
pub fn convert(input: &Path, output: &Path, opts: &Options) -> Result<Report, Error> {
    let t0 = Instant::now();
    let bytes = fs::read(input).map_err(|e| Error::Input(format!("{}: {e}", input.display())))?;
    let model = source::read(&bytes);
    let t_read = t0.elapsed().as_millis();

    let sem =
        semantics::build(&model, SemanticsOptions { include_spaces: opts.include_spaces, keep_parts: opts.keep_parts });
    if sem.elements.is_empty() {
        return Err(Error::NoElements(format!("{}: 形状を持つ部材がない", input.display())));
    }
    let element_meshes = || sem.elements.iter().flat_map(|e| e.meshes.iter().map(|&i| &model.meshes[i]));
    let reach = element_meshes().flat_map(|m| &m.positions).map(|p| p[0].hypot(p[1])).fold(0.0, f64::max);
    let resolved = georef::resolve(&model.georef, &opts.georef, reach).map_err(Error::Input)?;
    let mut warnings = resolved.warnings.clone();

    // 局所座標→ECEF→根のENU
    let projector = Projector::new(resolved.placement, opts.geoid);
    let mut positions: Vec<Vec<[f64; 3]>> = vec![Vec::new(); model.meshes.len()];
    let mut ecef_bounds = Aabb::EMPTY;
    for e in &sem.elements {
        for &i in &e.meshes {
            positions[i] = model.meshes[i].positions.iter().map(|&p| projector.to_ecef(p)).collect();
            positions[i].iter().for_each(|&p| ecef_bounds.add(p));
        }
    }
    if projector.geoid_misses() > 0 {
        let what = match resolved.placement {
            Placement::Enu(_) => "原点".to_string(),
            Placement::Grid(_) => format!("{}頂点", projector.geoid_misses()),
        };
        warnings.push(format!("{what}がジオイドモデル（{}）の範囲外で、ジオイド高を0とした", opts.geoid.name()));
    }
    let frame = Frame::at_ecef(ecef_bounds.center());
    let mut normals: Vec<Vec<[f32; 3]>> = vec![Vec::new(); model.meshes.len()];
    let mut element_bounds = Vec::with_capacity(sem.elements.len());
    for e in &sem.elements {
        let local = e.meshes.iter().flat_map(|&i| &model.meshes[i].positions).fold(Aabb::EMPTY, |mut b, &p| {
            b.add(p);
            b
        });
        let r = projector.rotation_at(local.center(), &frame);
        let mut b = Aabb::EMPTY;
        for &i in &e.meshes {
            for p in &mut positions[i] {
                *p = frame.to_local(*p);
                b.add(*p);
            }
            normals[i] =
                model.meshes[i].normals.iter().map(|n| rotate(&r, n.map(f64::from)).map(|v| v as f32)).collect();
        }
        element_bounds.push(b);
    }
    let bounds = element_bounds.iter().fold(Aabb::EMPTY, |a, b| a.union(b));

    let records: Vec<_> = sem.elements.iter().map(|e| e.record.clone()).collect();
    let table = Table::build(&records, &model.units, opts.include_properties);
    let trees: Vec<Node> = (0..sem.storeys.len())
        .map(|s| {
            let items: Vec<(usize, Aabb)> = (0..sem.elements.len())
                .filter(|&i| sem.elements[i].storey == s)
                .map(|i| (i, element_bounds[i]))
                .collect();
            tiling::build(&items, opts.max_features)
        })
        .collect();
    let t_convert = t0.elapsed().as_millis();

    // 書き出し
    let tiles_dir = output.join("tiles");
    fs::create_dir_all(&tiles_dir).map_err(io(&tiles_dir))?;
    let mut uris: HashMap<(usize, String), String> = HashMap::new();
    let mut tile_reports = Vec::new();
    for (s, tree) in trees.iter().enumerate() {
        let mut stack = vec![tree];
        while let Some(n) = stack.pop() {
            stack.extend(&n.children);
            if n.elements.is_empty() {
                continue;
            }
            let name = format!("{s:03}_{}", n.path);
            let meshes: Vec<TileMesh> = n
                .elements
                .iter()
                .enumerate()
                .flat_map(|(f, &ei)| sem.elements[ei].meshes.iter().map(move |&mi| (f, mi)))
                .map(|(f, mi)| TileMesh {
                    feature: u32::try_from(f).expect("1タイルの部材数がu32に収まる"),
                    color: model.meshes[mi].color,
                    positions: &positions[mi],
                    normals: &normals[mi],
                    indices: &model.meshes[mi].indices,
                })
                .collect();
            let schema_id = format!("ifc2tiles_{name}");
            let meta = TileMetadata { table: &table, rows: &n.elements, schema_id: &schema_id };
            let bytes = glb::write(&meshes, &meta, opts.compress);
            let uri = format!("tiles/{name}.glb");
            let path = output.join(&uri);
            fs::write(&path, &bytes).map_err(io(&path))?;
            tile_reports.push(TileReport { uri: uri.clone(), features: n.elements.len(), bytes: bytes.len() });
            uris.insert((s, n.path.clone()), uri);
        }
    }
    tile_reports.sort_by(|a, b| a.uri.cmp(&b.uri));

    let conversion = conversion_json(&resolved, opts, &frame, &warnings);
    let uri = |s: usize, n: &Node| uris.get(&(s, n.path.clone())).cloned();
    let ts = tileset::build(&frame, &bounds, &trees, &uri, &sem.storeys, &table, conversion.clone());
    let ts_path = output.join("tileset.json");
    fs::write(&ts_path, serde_json::to_vec_pretty(&ts).expect("tilesetのJSON化")).map_err(io(&ts_path))?;

    let mut report = Report {
        input: input.display().to_string(),
        schema: model.schema.clone(),
        elements: sem.elements.len(),
        storeys: sem.storeys.len(),
        columns: table.columns.len(),
        excluded: sem.excluded.clone(),
        without_mesh: sem.without_mesh.clone(),
        tiles: tile_reports,
        conversion,
        warnings,
        timing_ms: Default::default(),
    };
    report.timing_ms.insert("read", t_read);
    report.timing_ms.insert("convert", t_convert - t_read);
    report.timing_ms.insert("write", t0.elapsed().as_millis() - t_convert);
    report.timing_ms.insert("total", t0.elapsed().as_millis());
    let report_path = output.join("ifc2tiles-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report.to_json()).expect("レポートのJSON化"))
        .map_err(io(&report_path))?;
    Ok(report)
}

fn conversion_json(resolved: &Resolved, opts: &Options, frame: &Frame, warnings: &[String]) -> Value {
    let placement = match resolved.placement {
        Placement::Grid(g) => json!({
            "kind": "grid", "crs": format!("EPSG:{}", g.epsg), "originMap": g.origin,
            "rotationDeg": g.rotation.to_degrees(), "scale": g.scale, "factors": g.factors,
        }),
        Placement::Enu(e) => json!({
            "kind": "enu", "latitude": e.latitude_deg, "longitude": e.longitude_deg,
            "orthometricHeight": e.orthometric_height, "rotationDeg": e.rotation.to_degrees(),
        }),
    };
    let [lat, lon, h] = geodesy::to_geodetic(frame.origin);
    json!({
        "source": resolved.source,
        "placement": placement,
        "geoid": opts.geoid.name(),
        "rootOrigin": { "latitude": lat, "longitude": lon, "ellipsoidalHeight": h },
        "compress": opts.compress,
        "maxFeatures": opts.max_features,
        "warnings": warnings,
    })
}
