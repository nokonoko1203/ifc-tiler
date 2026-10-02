# ifc-tiler

A Rust CLI that converts IFC (IFC2x3 / IFC4 / IFC4X3) BIM models into 3D Tiles 1.1, where each building element can be picked, queried for its properties, and styled by them.

**English**|[日本語](./README.ja.md)

```bash
ifc_tiler model.ifc -o out/model
```

## Why

IFC coordinates are local to each building, and where the model sits on the map is determined by the file's georeferencing (`IfcMapConversion` or the latitude/longitude of `IfcSite`). How that is written varies between tools and files, and it is often missing or wrong. Converting IFC into plain meshes also loses the distinction between walls, columns and doors, along with each element's unique ID and properties such as fire rating or area. Real-world models have thousands to tens of thousands of elements, and packing them into a single file makes them heavy in the browser.

ifc-tiler uses the georeferencing to convert map coordinates to latitude, longitude and height for every vertex, and places the model at the correct position, orientation and height on the globe. Coordinate conversion is done by [PROJ](https://proj.org/), so any coordinate system that can be expressed as an EPSG code is supported.

Each element becomes one feature in 3D Tiles. Clicking it shows its unique ID, IFC class (wall, column, …), name, type, storey, and property and quantity values. Features can also be colored by property values. Tiles are split into a quadtree per storey and load from the largest elements down, and geometry is kept small with quantization and meshopt compression.

To view the output, use a viewer that supports 3D Tiles 1.1 (`EXT_mesh_features` and `EXT_structural_metadata`), such as CesiumJS.

## Installation

### Prebuilt binaries

[Releases](https://github.com/nokonoko1203/ifc-tiler/releases) has binaries for macOS (Apple Silicon / Intel), Linux (x86_64 / aarch64) and Windows (x86_64). PROJ and SQLite are statically linked, so neither Rust nor PROJ is needed.

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/nokonoko1203/ifc-tiler/releases/latest/download/ifc-tiler-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/nokonoko1203/ifc-tiler/releases/latest/download/ifc-tiler-installer.ps1 | iex"
```

The prebuilt binaries cannot use grids such as geoid models (the bundled PROJ can neither download nor read them). Transformations that need grids fall back to an approximation without grids, reported with `warning:`. Elevations are converted to ellipsoidal heights with a geoid model (EGM2008 when the CRS has no height reference), so in the prebuilt binaries this applies to almost every input: the geoid height is not added and heights are off by that amount (about 37 m around Tokyo). If you need accurate heights, build from source as described below.

### Building from source

Requirements:

- Rust 1.95. The version is pinned in `rust-toolchain.toml`, so [rustup](https://rustup.rs/) installs it automatically
- A C++ compiler. The meshopt crate builds its bundled meshoptimizer (Xcode Command Line Tools on macOS)
- PROJ 9.6.2 or later. Install it with `brew install proj` on macOS or `apt install libproj-dev` on Debian / Ubuntu. If it is not found, it is built from source, which additionally requires CMake and SQLite

```bash
git clone https://github.com/nokonoko1203/ifc-tiler.git
cd ifc-tiler
cargo build --release
```

The executable is created at `target/release/ifc_tiler`.

Grids such as geoid models are downloaded from [cdn.proj.org](https://cdn.proj.org/) when needed and cached (`~/Library/Application Support/proj` on macOS). To use the tool without network access, fetch the grids in advance with `projsync`.

## Usage

```text
ifc_tiler <INPUT> -o <OUTPUT_DIR> [--crs <CRS>] [--origin <LAT,LON[,H]>] [--map-conversion <E,N[,H[,ROT]]>]
```

| Argument / option | Description |
|---|---|
| `<INPUT>` | Input IFC file |
| `-o, --output <DIR>` | Output directory (required). Specify an empty directory (tiles from a previous run are not removed) |
| `--crs <CRS>` | Coordinate reference system (e.g. `EPSG:32654`, `6677`, or `EPSG:6677+6695` to include a height reference). When placing by map coordinates, it overrides the TargetCRS of the file's `IfcMapConversion` (required with `--map-conversion`). When placing by `--origin` or `IfcSite`, it is the CRS of the latitude, longitude and height (default `EPSG:4326`) |
| `--origin <LAT,LON[,H]>` | Ignore the file's georeferencing and place the local coordinates as east, north and height around this latitude, longitude (degrees) and elevation (m, default 0) |
| `--map-conversion <E,N[,H[,ROT]]>` | Ignore the file's georeferencing, place the local origin at map easting E, northing N (m) and elevation H (m), and rotate the local X axis ROT degrees counterclockwise from east. Requires `--crs`. Cannot be combined with `--origin` |

On success, it prints one line with the number of elements, storeys and tiles, the size and the processing time. Warnings are written to standard error as lines starting with `warning:`.

Exit codes: 0 on success, 2 for input or configuration errors, 3 when there are no convertible elements, 1 otherwise (e.g. write failures).

### How the position is determined

The following sources are checked in order, and the first one found is used.

| Order | Source | Placement |
|---|---|---|
| 1 | `--origin` | East, north and height (ENU) around that point |
| 2 | `--map-conversion` and `--crs` | Local coordinates (m) are placed in map coordinates and each vertex is converted to latitude/longitude |
| 3 | The file's `IfcMapConversion` / `IfcMapConversionScaled` | Converted to map coordinates, then each vertex is converted to latitude/longitude |
| 4 | The latitude/longitude of the file's `IfcSite` ((0, 0) is treated as unset) | ENU around that point (rotated by `TrueNorth`) |

If none is available, it fails with an error (exit code 2).

### Height reference

Heights (elevations) in IFC are measured against a national or regional height reference. Converting them to ellipsoidal heights requires the geoid for that reference, so specify a CRS that includes the height reference (a compound CRS) with `--crs`. A compound CRS is written either by joining a horizontal CRS and a height reference with `+`, or with a code registered in EPSG.

| Example | Meaning |
|---|---|
| `EPSG:6677+6695`, `EPSG:10170` | JGD2011 / Japan Plane Rectangular CS IX + JGD2011 height |
| `EPSG:6697` | JGD2011 latitude/longitude + JGD2011 height (for `--origin` and `IfcSite`) |
| `EPSG:25832+7837` | ETRS89 / UTM 32N + German height (DHHN2016) |

For a CRS without a height reference, such as `EPSG:6677`, heights are treated as elevations based on the global geoid model EGM2008, and a warning is shown. In that case, the difference between the national reference and EGM2008 (tens of centimeters to about 1 m) remains.

If a grid cannot be downloaded, the output is produced with an approximate transformation that does not use grids, and a warning is shown. Heights may then be off by the geoid height (−105 to +85 m worldwide).

### Examples

An IFC with georeferencing can be converted as is.

```bash
ifc_tiler building.ifc -o out/building
```

If the `IfcMapConversion` has no CRS name, or the EPSG code cannot be read from the name, specify it with `--crs`.

```bash
ifc_tiler building.ifc -o out/building --crs EPSG:6677+6695
```

For an IFC whose local coordinates are already map coordinates, place the local origin at map coordinates (0, 0). This is common in civil engineering models without georeferencing.

```bash
ifc_tiler civil.ifc -o out/civil --map-conversion 0,0 --crs EPSG:6677+6695
```

For an IFC with missing or wrong georeferencing, give the latitude, longitude and elevation where it should be placed.

```bash
ifc_tiler building.ifc -o out/building --origin 35.681236,139.767125,3.5 --crs EPSG:6697
```

## Output

```text
out/building/
  tileset.json                    3D Tiles 1.1. schema (storey, element), groups (storeys)
  tiles/<storey>_<quadtree>.glb   glTF 2.0. EXT_mesh_features, EXT_structural_metadata, KHR_mesh_quantization,
                                  EXT_meshopt_compression, and (for tiles with identical meshes)
                                  EXT_mesh_gpu_instancing, EXT_instance_features
```

### Elements and properties

- Each `IfcProduct` becomes one feature. Openings (`IfcOpeningElement`), spaces (`IfcSpace`), structural analysis elements and types are not output. Aggregated parts, such as the layers of a multi-layer wall, are merged into their parent element.
- Each element has the following columns.

  | Column | Description |
  |---|---|
  | `expressId` | ID within the file |
  | `ifcClass` | IFC class (e.g. `IfcWall`) |
  | `globalId` | Unique ID of the element (IFC GlobalId) |
  | `name`, `description`, `objectType`, `tag`, `predefinedType` | Element attributes |
  | `typeName` | Name of the type (`IfcRelDefinesByType`) |
  | `storeyName`, `storeyGlobalId` | Storey. Elements without a storey are grouped into an `(unassigned)` storey |
  | `buildingName` | Building the element belongs to |
  | `<Pset name>__<property name>` | Property (IFC property set, Pset) and quantity (IFC quantity set, Qto) values. Values from the type are inherited and overridden by the element's own values |

- Lengths, areas, volumes and masses are converted to SI units (m, m², m³, kg), and the unit is written in the column's `description`.
- Integers become INT32, reals FLOAT64, strings STRING, and booleans (`IfcBoolean`, `IfcLogical`) ENUM.
- Columns whose property set or property name contains non-alphanumeric characters (e.g. Japanese) get an ID of the form `p_<hash>`. The original name is kept in the column's `name`.
- Storey information (name, unique ID, elevation) is stored in `groups`.

### Tiles and geometry

- Tiles form a planar quadtree per storey, with up to 200 elements per tile. Larger elements are placed in higher tiles (`refine: ADD`), so from far away only large elements such as floors and walls are loaded, and smaller ones are added as you get closer.
- The geometry of a tile is merged into two primitives, opaque and translucent. Colors are stored as vertex colors (`COLOR_0`).
- When a tile contains three or more identical meshes with 200 or more vertices, they are output as one template and its instances. Meshes are considered identical when they differ only by translation and their coordinates relative to the minimum point match after rounding to 1 mm.

## Not supported

3D Tiles 2.0, `IfcRigidOperation` (place such models with `--origin` or `--map-conversion`), instancing of identical meshes with different rotations, textures, simplified geometry for distant views, outlines, merging multiple IFC files, and IFC4.3 linear placement geometry (not supported by ifc-lite).
