//! Converts IFC into 3D Tiles 1.1 with element metadata.
//!
//! Pipeline (`convert`):
//! `source` (read the IFC) → `semantics` (decide which elements become features) → `georef` (decide the placement)
//! → `geodesy` (ECEF → root ENU) → `tiling` (tile tree) → `metadata` / `glb` / `tileset` (output).

mod geodesy;
mod georef;
mod glb;
mod metadata;
mod proj;
mod semantics;
mod source;
mod tileset;
mod tiling;
mod units;

pub use georef::GeorefOptions;

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::Path;

use crate::geodesy::{Frame, Projector, rotate};
use crate::georef::Resolved;
use crate::glb::{TileMesh, TileMetadata};
use crate::metadata::Table;
use crate::semantics::Semantics;
use crate::source::SourceModel;
use crate::tiling::{Aabb, Node};

/// Maximum number of elements per tile.
const MAX_FEATURES: usize = 200;

/// Result of a conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct Summary {
    pub elements: usize,
    pub storeys: usize,
    pub tiles: usize,
    /// Total size of the GLB files [bytes].
    pub bytes: usize,
    pub warnings: Vec<String>,
}

#[derive(Debug)]
pub enum Error {
    /// Invalid input or settings (exit code 2).
    Input(String),
    /// No convertible elements (exit code 3).
    NoElements(String),
    /// Output failures and the like (exit code 1).
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
        "map coordinates (E={e:.1}, N={n:.1}) are outside the domain of the CRS; check --crs and the georeferencing"
    ))
}

/// Converts the IFC at `input` and writes tileset.json and tiles/*.glb to `output`.
pub fn convert(input: &Path, output: &Path, opts: &GeorefOptions) -> Result<Summary, Error> {
    let bytes = fs::read(input).map_err(|e| Error::Input(format!("{}: {e}", input.display())))?;
    let model = source::read(&bytes);

    let sem = semantics::build(&model);
    if sem.elements.is_empty() {
        return Err(Error::NoElements(format!("{}: no elements with geometry", input.display())));
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
    fs::write(&ts_path, serde_json::to_vec_pretty(&ts).expect("serializing the tileset to JSON"))
        .map_err(io(&ts_path))?;
    Ok(Summary {
        elements: sem.elements.len(),
        storeys: sem.storeys.len(),
        tiles: uris.len(),
        bytes,
        warnings: placed.warnings,
    })
}

/// Geometry placed on the globe. Vertices and normals are in the root ENU (`frame`) coordinates, indexed like `SourceModel::meshes`.
/// Meshes that do not become features are empty.
struct Placed {
    frame: Frame,
    positions: Vec<Vec<[f64; 3]>>,
    normals: Vec<Vec<[f32; 3]>>,
    /// Bounding boxes in the root ENU, indexed like `Semantics::elements`.
    element_bounds: Vec<Aabb>,
    warnings: Vec<String>,
}

/// Local coordinates → ECEF → root ENU. The root ENU origin is the center of the ECEF bounding box of all vertices.
fn place(model: &SourceModel, sem: &Semantics, resolved: &Resolved) -> Result<Placed, Error> {
    let mut warnings = resolved.warnings.clone();
    let (projector, projector_warnings) = Projector::new(&resolved.placement).map_err(Error::Input)?;
    warnings.extend(projector_warnings);
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

    let frame = Frame::at_ecef(ecef_bounds.center());
    let mut normals: Vec<Vec<[f32; 3]>> = vec![Vec::new(); model.meshes.len()];
    let mut element_bounds = Vec::with_capacity(sem.elements.len());
    for e in &sem.elements {
        // Normals are rotated by the local → root ENU rotation computed at the element's center (not projected per vertex)
        let local = Aabb::from_points(e.meshes.iter().flat_map(|&i| &model.meshes[i].positions).copied());
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

/// (storey index, quadtree path) → content URI.
type TileUris = HashMap<(usize, String), String>;

/// Writes a GLB for every node with content. Returns ((storey, path) → URI, total GLB size).
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
            // The element index within a tile (row of the property table) follows the order of the node's elements
            let meshes: Vec<TileMesh> = n
                .elements
                .iter()
                .enumerate()
                .flat_map(|(f, &ei)| sem.elements[ei].meshes.iter().map(move |&mi| (f, mi)))
                .map(|(f, mi)| TileMesh {
                    feature: u32::try_from(f).expect("the number of elements in a tile fits in u32"),
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
