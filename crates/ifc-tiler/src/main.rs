//! `ifc_tiler <INPUT> -o <OUTPUT_DIR> [OPTIONS]`

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use ifc_tiler::{Error, GeorefOptions, convert};

/// Converts IFC into 3D Tiles 1.1 with element metadata
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Input IFC file (IFC2x3 / IFC4 / IFC4X3)
    input: PathBuf,
    /// Output directory (tileset.json, tiles/)
    #[arg(short, long)]
    output: PathBuf,
    /// CRS (e.g. EPSG:6677, or EPSG:6677+6695 to include a height reference). For map coordinates it overrides
    /// the TargetCRS of IfcMapConversion (required with --map-conversion); for --origin and IfcSite it is the CRS of latitude, longitude and height
    #[arg(long, value_parser = crs)]
    crs: Option<String>,
    /// Ignore the file's georeferencing and place the model as east, north and height around this point (latitude,longitude[,elevation in m])
    #[arg(long, value_parser = origin, allow_hyphen_values = true)]
    origin: Option<[f64; 3]>,
    /// Place the local coordinates in map coordinates (easting,northing[,elevation[,rotation of the local X axis from east in degrees]], in m). Requires --crs.
    /// Used instead of the file's georeferencing. Use 0,0 if the local coordinates are already map coordinates
    #[arg(long, value_parser = map_conversion, allow_hyphen_values = true, conflicts_with = "origin")]
    map_conversion: Option<[f64; 4]>,
}

/// A bare number is treated as an EPSG code. Anything else is passed to PROJ as is.
fn crs(s: &str) -> Result<String, String> {
    let s = s.trim();
    Ok(if s.parse::<u32>().is_ok() { format!("EPSG:{s}") } else { s.to_string() })
}

fn origin(s: &str) -> Result<[f64; 3], String> {
    let v: Vec<f64> =
        s.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|e| format!("{s}: {e}"))?;
    match v.as_slice() {
        [lat, lon] => Ok([*lat, *lon, 0.0]),
        [lat, lon, h] => Ok([*lat, *lon, *h]),
        _ => Err(format!("specify it as latitude,longitude[,elevation]: {s}")),
    }
}

fn map_conversion(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> =
        s.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|e| format!("{s}: {e}"))?;
    match v.as_slice() {
        [e, n] => Ok([*e, *n, 0.0, 0.0]),
        [e, n, h] => Ok([*e, *n, *h, 0.0]),
        [e, n, h, r] => Ok([*e, *n, *h, *r]),
        _ => Err(format!("specify it as easting,northing[,elevation[,rotation°]]: {s}")),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let opts = GeorefOptions { crs: cli.crs, origin: cli.origin, map_conversion: cli.map_conversion };
    let t0 = Instant::now();
    match convert(&cli.input, &cli.output, &opts) {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("warning: {w}");
            }
            println!(
                "{}: {} elements, {} storeys, {} tiles, {:.2} MB, {} ms -> {}",
                cli.input.display(),
                r.elements,
                r.storeys,
                r.tiles,
                r.bytes as f64 / 1e6,
                t0.elapsed().as_millis(),
                cli.output.join("tileset.json").display()
            );
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(match e {
                Error::Input(_) => 2,
                Error::NoElements(_) => 3,
                Error::Io(_) => 1,
            })
        }
    }
}
