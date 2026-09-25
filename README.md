# ifc2tiles

IFCを部材情報付きの3D Tiles 1.1へ変換するRust CLI（開発中）。

## 構成

| パス | 内容 |
|---|---|
| `Cargo.toml` | cargo workspace（メンバーはMVP以降に追加） |
| `rust-toolchain.toml` | Rust 1.95.0に固定 |
| `scripts/fetch-testdata.sh` | 外部IFCを commit 固定で `testdata/external/` に取得し、sha256を照合する |
| `scripts/gen_handmade_ifc.py` | 既知点に置いた検証用IFCと期待値を `testdata/handmade/` に生成する |
| `scripts/validate-tiles.sh` | 3d-tiles-validator（`tools/validator`で版固定）でtilesetを検証する |
| `testdata/sources.tsv` / `sources.sha256` | 外部テストデータの取得元とハッシュ |
| `testdata/handmade/` | 自作IFC（IFC4 `IfcMapConversion` / IFC2x3 PLATEAU方式 / IFC2x3 `IfcSite`経緯度）と `expected.json` |
| `tools/validator/` | 3d-tiles-validator 0.6.1。`sharp`をインストールスクリプト不要の0.34.5へ差し替え |
| `viewer/index.html` | 確認用CesiumJSビューア（`?tileset=`で指定、クリックで属性表示） |
| `viewer/bench.html` | メタデータ負荷の計測ページ |

## 使い方

```bash
scripts/fetch-testdata.sh
uv run --no-project --python 3.12 scripts/gen_handmade_ifc.py
scripts/validate-tiles.sh out/xxx/tileset.json
python3 -m http.server 8790 --bind 127.0.0.1   # http://127.0.0.1:8790/viewer/?tileset=/out/xxx/tileset.json
```
