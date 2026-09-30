//! `ifc_tiler <INPUT> -o <OUTPUT_DIR> [OPTIONS]`

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use clap::Parser;
use ifc_tiler::georef::{GeorefOptions, parse_epsg};
use ifc_tiler::{Error, convert};

/// IFCを部材情報付きの3D Tiles 1.1へ変換する
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// 入力IFC（IFC2x3 / IFC4 / IFC4X3）
    input: PathBuf,
    /// 出力先のディレクトリ（tileset.json、tiles/）
    #[arg(short, long)]
    output: PathBuf,
    /// 地図座標のCRS（例: EPSG:6677）。IfcMapConversionのTargetCRSを上書きする。--map-conversion では必須
    #[arg(long, value_parser = epsg)]
    crs: Option<u32>,
    /// ファイルのジオリファレンスを使わず、この点を原点とする東・北・上で置く（緯度,経度[,正標高m]）
    #[arg(long, value_parser = origin, allow_hyphen_values = true)]
    origin: Option<[f64; 3]>,
    /// 局所座標を地図座標で置く（東,北[,正標高[,局所X軸から東への回転°]]、m）。--crsが必要。
    /// ファイルのジオリファレンスの代わりに使う。局所座標が平面直角座標の値なら 0,0
    #[arg(long, value_parser = map_conversion, allow_hyphen_values = true, conflicts_with = "origin")]
    map_conversion: Option<[f64; 4]>,
}

fn epsg(s: &str) -> Result<u32, String> {
    parse_epsg(s).or_else(|| s.parse().ok()).ok_or_else(|| format!("EPSGコードとして読めない: {s}"))
}

fn origin(s: &str) -> Result<[f64; 3], String> {
    let v: Vec<f64> =
        s.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|e| format!("{s}: {e}"))?;
    match v.as_slice() {
        [lat, lon] => Ok([*lat, *lon, 0.0]),
        [lat, lon, h] => Ok([*lat, *lon, *h]),
        _ => Err(format!("緯度,経度[,正標高] の形で指定する: {s}")),
    }
}

fn map_conversion(s: &str) -> Result<[f64; 4], String> {
    let v: Vec<f64> =
        s.split(',').map(|x| x.trim().parse::<f64>()).collect::<Result<_, _>>().map_err(|e| format!("{s}: {e}"))?;
    match v.as_slice() {
        [e, n] => Ok([*e, *n, 0.0, 0.0]),
        [e, n, h] => Ok([*e, *n, *h, 0.0]),
        [e, n, h, r] => Ok([*e, *n, *h, *r]),
        _ => Err(format!("東,北[,正標高[,回転°]] の形で指定する: {s}")),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let opts = GeorefOptions { crs_epsg: cli.crs, origin: cli.origin, map_conversion: cli.map_conversion };
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
