//! `ifc2tiles <INPUT> -o <OUTPUT_DIR> [OPTIONS]`

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, ValueEnum};
use ifc2tiles::geodesy::GeoidModel;
use ifc2tiles::georef::{GeorefOptions, ScalePolicy, SiteCoords, parse_epsg};
use ifc2tiles::{Error, Options, convert};

/// IFCを部材情報付きの3D Tiles 1.1へ変換する
#[derive(Parser)]
#[command(version)]
struct Cli {
    /// 入力IFC（IFC2x3 / IFC4 / IFC4X3）
    input: PathBuf,
    /// 出力先のディレクトリ（tileset.json、tiles/、ifc2tiles-report.json）
    #[arg(short, long)]
    output: PathBuf,
    /// 地図座標のCRS（例: EPSG:6677）。IfcMapConversionのTargetCRSを上書きする。--site-coords grid では必須
    #[arg(long, value_parser = epsg)]
    crs: Option<u32>,
    /// IfcSite経路で局所座標をどう解釈するか（enu: 経緯度を原点とする東・北・上、grid: 地図座標のオフセット）
    #[arg(long, value_enum, default_value_t = SiteCoordsArg::Enu)]
    site_coords: SiteCoordsArg,
    /// ファイルのジオリファレンスを使わず、この点を原点とする東・北・上で置く（緯度,経度[,正標高m]）
    #[arg(long, value_parser = origin, allow_hyphen_values = true)]
    origin: Option<[f64; 3]>,
    /// 正標高→楕円体高のジオイドモデル（jpgeo2024はHrefconv2024を含む）
    #[arg(long, value_enum, default_value_t = GeoidArg::Jpgeo2024)]
    geoid: GeoidArg,
    /// IfcMapConversion.Scaleの解釈（auto: 逆数で書かれたファイルを検出して直す）
    #[arg(long, value_enum, default_value_t = ScalePolicyArg::Auto)]
    scale_policy: ScalePolicyArg,
    /// 1タイルの部材数の上限
    #[arg(long, default_value_t = 200, value_parser = clap::value_parser!(u32).range(1..))]
    max_features: u32,
    /// IfcSpaceも出力する
    #[arg(long)]
    include_spaces: bool,
    /// 集約の部品を親部材にまとめない
    #[arg(long)]
    keep_parts: bool,
    /// Pset / Qto を列に含めない
    #[arg(long)]
    no_psets: bool,
    /// 量子化とmeshopt圧縮をしない
    #[arg(long)]
    no_compress: bool,
}

#[derive(Clone, Copy, ValueEnum)]
enum SiteCoordsArg {
    Enu,
    Grid,
}

#[derive(Clone, Copy, ValueEnum)]
enum GeoidArg {
    Jpgeo2024,
    Gsigeo2011,
    None,
}

#[derive(Clone, Copy, ValueEnum)]
enum ScalePolicyArg {
    Auto,
    Spec,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let opts = Options {
        georef: GeorefOptions {
            crs_epsg: cli.crs,
            site_coords: match cli.site_coords {
                SiteCoordsArg::Enu => SiteCoords::Enu,
                SiteCoordsArg::Grid => SiteCoords::Grid,
            },
            origin: cli.origin,
            scale_policy: match cli.scale_policy {
                ScalePolicyArg::Auto => ScalePolicy::Auto,
                ScalePolicyArg::Spec => ScalePolicy::Spec,
            },
        },
        geoid: match cli.geoid {
            GeoidArg::Jpgeo2024 => GeoidModel::Jpgeo2024,
            GeoidArg::Gsigeo2011 => GeoidModel::Gsigeo2011,
            GeoidArg::None => GeoidModel::None,
        },
        max_features: cli.max_features as usize,
        include_spaces: cli.include_spaces,
        keep_parts: cli.keep_parts,
        include_properties: !cli.no_psets,
        compress: !cli.no_compress,
    };
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
                r.tiles.len(),
                r.total_bytes() as f64 / 1e6,
                r.timing_ms["total"],
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
