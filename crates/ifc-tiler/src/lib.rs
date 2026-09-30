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
use crate::glb::{Encoding, TileMesh, TileMetadata};
use crate::metadata::Table;
use crate::report::{Report, TileReport};
use crate::semantics::{Semantics, SemanticsOptions};
use crate::source::SourceModel;
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
    /// タイル内の同形メッシュをインスタンス化する。
    pub instancing: bool,
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

fn outside([e, n, _]: [f64; 3]) -> Error {
    Error::Input(format!(
        "地図座標 (E={e:.1}, N={n:.1}) が平面直角座標系の定義域外。--crs やジオリファレンスを確かめる"
    ))
}

/// `input`のIFCを変換し、`output`にtileset.json・tiles/*.glb・ifc-tiler-report.jsonを書く。
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
    let reach = sem
        .elements
        .iter()
        .flat_map(|e| e.meshes.iter().flat_map(|&i| &model.meshes[i].positions))
        .map(|p| p[0].hypot(p[1]))
        .fold(0.0, f64::max);
    let resolved = georef::resolve(&model.georef, &opts.georef, reach).map_err(Error::Input)?;
    let placed = place(&model, &sem, &resolved, opts.geoid)?;

    let records: Vec<_> = sem.elements.iter().map(|e| e.record.clone()).collect();
    let table = Table::build(&records, &model.units, opts.include_properties);
    let trees: Vec<Node> = (0..sem.storeys.len())
        .map(|s| {
            let items: Vec<(usize, Aabb)> = (0..sem.elements.len())
                .filter(|&i| sem.elements[i].storey == s)
                .map(|i| (i, placed.element_bounds[i]))
                .collect();
            tiling::build(&items, opts.max_features)
        })
        .collect();
    let t_convert = t0.elapsed().as_millis();

    let enc = Encoding { compress: opts.compress, instancing: opts.instancing };
    let (uris, tiles) = write_tiles(output, &trees, &model, &sem, &placed, &table, enc)?;
    let conversion = conversion_json(&resolved, opts, &placed.frame, &placed.warnings);
    let bounds = placed.element_bounds.iter().fold(Aabb::EMPTY, |a, b| a.union(b));
    let uri = |s: usize, n: &Node| uris.get(&(s, n.path.clone())).cloned();
    let ts = tileset::build(&placed.frame, &bounds, &trees, &uri, &sem.storeys, &table, conversion.clone());
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
        tiles,
        conversion,
        warnings: placed.warnings,
        timing_ms: Default::default(),
    };
    report.timing_ms.insert("read", t_read);
    report.timing_ms.insert("convert", t_convert - t_read);
    report.timing_ms.insert("write", t0.elapsed().as_millis() - t_convert);
    report.timing_ms.insert("total", t0.elapsed().as_millis());
    let report_path = output.join("ifc-tiler-report.json");
    fs::write(&report_path, serde_json::to_vec_pretty(&report.to_json()).expect("レポートのJSON化"))
        .map_err(io(&report_path))?;
    Ok(report)
}

/// 地球上に置いた形状。頂点と法線は根のENU（`frame`）の座標で、`SourceModel::meshes`と同じ番号。
/// featureにならないメッシュは空。
struct Placed {
    frame: Frame,
    positions: Vec<Vec<[f64; 3]>>,
    normals: Vec<Vec<[f32; 3]>>,
    /// `Semantics::elements`と同じ番号の、根のENUでの外接箱。
    element_bounds: Vec<Aabb>,
    warnings: Vec<String>,
}

/// 局所座標→ECEF→根のENU。根のENUの原点は、全頂点のECEF外接箱の中心。
fn place(model: &SourceModel, sem: &Semantics, resolved: &Resolved, geoid: GeoidModel) -> Result<Placed, Error> {
    let mut warnings = resolved.warnings.clone();
    let projector = Projector::new(resolved.placement, geoid);
    let mut positions: Vec<Vec<[f64; 3]>> = vec![Vec::new(); model.meshes.len()];
    let mut ecef_bounds = Aabb::EMPTY;
    for &i in sem.elements.iter().flat_map(|e| &e.meshes) {
        positions[i] = model.meshes[i]
            .positions
            .iter()
            .map(|&p| projector.to_ecef(p))
            .collect::<Result<_, _>>()
            .map_err(outside)?;
        positions[i].iter().for_each(|&p| ecef_bounds.add(p));
    }
    if projector.geoid_misses() > 0 {
        let what = match resolved.placement {
            Placement::Enu(_) => "原点".to_string(),
            Placement::Grid(_) => format!("{}頂点", projector.geoid_misses()),
        };
        warnings.push(format!("{what}がジオイドモデル（{}）の範囲外で、ジオイド高を0とした", geoid.name()));
    }

    let frame = Frame::at_ecef(ecef_bounds.center());
    let mut normals: Vec<Vec<[f32; 3]>> = vec![Vec::new(); model.meshes.len()];
    let mut element_bounds = Vec::with_capacity(sem.elements.len());
    for e in &sem.elements {
        // 法線は、部材の中心で求めた局所→根のENUの回転で向きを変える（頂点ごとには投影しない）
        let local = e.meshes.iter().flat_map(|&i| &model.meshes[i].positions).fold(Aabb::EMPTY, |mut b, &p| {
            b.add(p);
            b
        });
        let r = projector.rotation_at(local.center(), &frame).map_err(outside)?;
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
    Ok(Placed { frame, positions, normals, element_bounds, warnings })
}

/// (階の番号, 四分木のパス) → contentのURI。
type TileUris = HashMap<(usize, String), String>;

/// contentを持つノードごとにGLBを書く。返り値は（(階, パス)→URI、タイルごとの報告）。
fn write_tiles(
    output: &Path,
    trees: &[Node],
    model: &SourceModel,
    sem: &Semantics,
    placed: &Placed,
    table: &Table,
    enc: Encoding,
) -> Result<(TileUris, Vec<TileReport>), Error> {
    let tiles_dir = output.join("tiles");
    fs::create_dir_all(&tiles_dir).map_err(io(&tiles_dir))?;
    let mut uris = HashMap::new();
    let mut reports = Vec::new();
    for (s, tree) in trees.iter().enumerate() {
        let mut stack = vec![tree];
        while let Some(n) = stack.pop() {
            stack.extend(&n.children);
            if n.elements.is_empty() {
                continue;
            }
            let name = format!("{s:03}_{}", n.path);
            // タイル内の部材番号（property tableの行）は、ノードの部材の並び順
            let meshes: Vec<TileMesh> = n
                .elements
                .iter()
                .enumerate()
                .flat_map(|(f, &ei)| sem.elements[ei].meshes.iter().map(move |&mi| (f, mi)))
                .map(|(f, mi)| TileMesh {
                    feature: u32::try_from(f).expect("1タイルの部材数がu32に収まる"),
                    color: model.meshes[mi].color,
                    positions: &placed.positions[mi],
                    normals: &placed.normals[mi],
                    indices: &model.meshes[mi].indices,
                })
                .collect();
            let schema_id = format!("ifc_tiler_{name}");
            let (bytes, stats) =
                glb::write(&meshes, &TileMetadata { table, rows: &n.elements, schema_id: &schema_id }, enc);
            let uri = format!("tiles/{name}.glb");
            let path = output.join(&uri);
            fs::write(&path, &bytes).map_err(io(&path))?;
            reports.push(TileReport {
                uri: uri.clone(),
                features: n.elements.len(),
                bytes: bytes.len(),
                primitives: stats.primitives,
                instances: stats.instances,
            });
            uris.insert((s, n.path.clone()), uri);
        }
    }
    reports.sort_by(|a, b| a.uri.cmp(&b.uri));
    Ok((uris, reports))
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
        "instancing": opts.instancing,
        "maxFeatures": opts.max_features,
        "warnings": warnings,
    })
}
