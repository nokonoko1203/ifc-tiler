//! IFCを部材情報付きの3D Tiles 1.1へ変換する。
//!
//! 処理の流れ（`convert`）:
//! `source`（IFCを読む）→ `semantics`（featureにする部材を決める）→ `georef`（置き方を決める）
//! → `geodesy`（ECEF→根のENUへ）→ `tiling`（タイルの木）→ `metadata` / `glb` / `tileset`（書き出し）。

pub mod geodesy;
pub mod georef;
pub mod glb;
pub mod metadata;
pub mod semantics;
pub mod source;
pub mod tileset;
pub mod tiling;
pub mod units;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::geodesy::{Frame, GEOID_NAME, Projector, rotate};
use crate::georef::{GeorefOptions, Placement, Resolved};
use crate::glb::{TileMesh, TileMetadata};
use crate::metadata::Table;
use crate::semantics::Semantics;
use crate::source::SourceModel;
use crate::tiling::{Aabb, Node};

/// 1タイルの部材数の上限。
const MAX_FEATURES: usize = 200;

/// 変換の結果。
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub elements: usize,
    pub storeys: usize,
    pub tiles: usize,
    /// GLBの合計サイズ [byte]。
    pub bytes: usize,
    pub warnings: Vec<String>,
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

/// `input`のIFCを変換し、`output`にtileset.jsonとtiles/*.glbを書く。
pub fn convert(input: &Path, output: &Path, opts: &GeorefOptions) -> Result<Summary, Error> {
    let bytes = fs::read(input).map_err(|e| Error::Input(format!("{}: {e}", input.display())))?;
    let model = source::read(&bytes);

    let sem = semantics::build(&model);
    if sem.elements.is_empty() {
        return Err(Error::NoElements(format!("{}: 形状を持つ部材がない", input.display())));
    }
    let reach = sem
        .elements
        .iter()
        .flat_map(|e| e.meshes.iter().flat_map(|&i| &model.meshes[i].positions))
        .map(|p| p[0].hypot(p[1]))
        .fold(0.0, f64::max);
    let resolved = georef::resolve(&model.georef, opts, reach).map_err(Error::Input)?;
    let placed = place(&model, &sem, &resolved)?;

    let records: Vec<_> = sem.elements.iter().map(|e| e.record.clone()).collect();
    let table = Table::build(&records, &model.units);
    let trees: Vec<Node> = (0..sem.storeys.len())
        .map(|s| {
            let items: Vec<(usize, Aabb)> = (0..sem.elements.len())
                .filter(|&i| sem.elements[i].storey == s)
                .map(|i| (i, placed.element_bounds[i]))
                .collect();
            tiling::build(&items, MAX_FEATURES)
        })
        .collect();

    let (uris, bytes) = write_tiles(output, &trees, &model, &sem, &placed, &table)?;
    let bounds = placed.element_bounds.iter().fold(Aabb::EMPTY, |a, b| a.union(b));
    let uri = |s: usize, n: &Node| uris.get(&(s, n.path.clone())).cloned();
    let ts = tileset::build(&placed.frame, &bounds, &trees, &uri, &sem.storeys, &table);
    let ts_path = output.join("tileset.json");
    fs::write(&ts_path, serde_json::to_vec_pretty(&ts).expect("tilesetのJSON化")).map_err(io(&ts_path))?;
    Ok(Summary {
        elements: sem.elements.len(),
        storeys: sem.storeys.len(),
        tiles: uris.len(),
        bytes,
        warnings: placed.warnings,
    })
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
fn place(model: &SourceModel, sem: &Semantics, resolved: &Resolved) -> Result<Placed, Error> {
    let mut warnings = resolved.warnings.clone();
    let projector = Projector::new(resolved.placement);
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
        warnings.push(format!("{what}がジオイドモデル（{}）の範囲外で、ジオイド高を0とした", GEOID_NAME));
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

/// contentを持つノードごとにGLBを書く。返り値は（(階, パス)→URI、GLBの合計サイズ）。
fn write_tiles(
    output: &Path,
    trees: &[Node],
    model: &SourceModel,
    sem: &Semantics,
    placed: &Placed,
    table: &Table,
) -> Result<(TileUris, usize), Error> {
    let tiles_dir = output.join("tiles");
    fs::create_dir_all(&tiles_dir).map_err(io(&tiles_dir))?;
    let mut uris = HashMap::new();
    let mut total = 0;
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
            let bytes = glb::write(&meshes, &TileMetadata { table, rows: &n.elements, schema_id: &schema_id });
            let uri = format!("tiles/{name}.glb");
            let path = output.join(&uri);
            fs::write(&path, &bytes).map_err(io(&path))?;
            total += bytes.len();
            uris.insert((s, n.path.clone()), uri);
        }
    }
    Ok((uris, total))
}
