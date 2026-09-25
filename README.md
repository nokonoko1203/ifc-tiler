# ifc2tiles

IFC（IFC2x3 / IFC4 / IFC4X3）を、部材ごとに選択・属性参照・属性スタイルができる3D Tiles 1.1へ変換するRust CLI。

```bash
cargo build --release
target/release/ifc2tiles model.ifc -o out/model
```

`out/model/tileset.json`をCesiumJSなどで読み込むと、地図上の正しい位置・向き・高さに表示され、部材をクリックするとGlobalId、IFCクラス、名前、型名、所属階、Pset / Qtoの値が取れる。

## 配置（ジオリファレンス）

次の順に、最初に見つかったもので置く。

| 順 | 情報 | 置き方 |
|---|---|---|
| 1 | `--origin LAT,LON[,H]` | その点を原点とする東・北・上（ENU） |
| 2 | `IfcMapConversion` / `IfcMapConversionScaled` | 平面直角座標へ変換し、頂点ごとに逆投影 |
| 3 | `IfcRigidOperation`（IFC4.3） | 投影CRSなら2と同じ。地理CRSならその経緯度を原点とするENU |
| 4 | `IfcSite`の経緯度（(0,0)は未設定とみなす） | `--site-coords enu`（既定）: ENU（`TrueNorth`で回転）。`--site-coords grid --crs EPSG:xxxx`: 局所座標を地図座標のオフセットとみなす（PLATEAU BIM活用マニュアル第1版の方式） |

- 対応するCRSは日本の平面直角座標系（JGD2011・JGD2000、I〜XIX系、鉛直を含む複合CRS）。それ以外は`--origin`で置く。
- 正標高は`--geoid`のジオイド高を足して楕円体高にする。既定の`jpgeo2024`は、国土地理院の「JPGEO2024＋Hrefconv2024」（離島の標高基準面の補正を含む）。
- `IfcMapConversion.Scale`が逆数で書かれたファイル（公式サンプルにもある）は、`--scale-policy auto`（既定）で検出して直し、警告する。
- ENUで置いたのに形状が原点から1 km以上離れている場合は、局所座標が平面直角座標の値である疑いがあるため警告する。

## オプション

| オプション | 既定 | 内容 |
|---|---|---|
| `-o, --output <DIR>` | 必須 | 出力先。空のディレクトリを指定する（前回のタイルは消さない） |
| `--crs <EPSG>` | | 地図座標のCRS。`IfcMapConversion`のTargetCRSを上書きする |
| `--site-coords <enu\|grid>` | `enu` | `IfcSite`経路での局所座標の解釈 |
| `--origin <LAT,LON[,H]>` | | ファイルのジオリファレンスを使わず、この点に置く |
| `--geoid <jpgeo2024\|gsigeo2011\|none>` | `jpgeo2024` | ジオイドモデル |
| `--scale-policy <auto\|spec>` | `auto` | `Scale`の解釈 |
| `--max-features <N>` | 200 | 1タイルの部材数の上限 |
| `--include-spaces` | | `IfcSpace`も出力する |
| `--keep-parts` | | 集約の部品（多層壁の層など）を親部材にまとめない |
| `--no-psets` | | Pset / Qto を列に含めない |
| `--no-compress` | | 量子化とmeshopt圧縮をしない |

終了コード: 成功0、入力・設定の誤り2、変換できる部材がない3、その他1。

## 出力

```text
out/
  tileset.json             3D Tiles 1.1。schema（storey・element）、groups（階）、変換の経緯（asset.extras.ifc2tiles）
  tiles/<階>_<四分木>.glb   glTF 2.0。EXT_mesh_features、EXT_structural_metadata、KHR_mesh_quantization、EXT_meshopt_compression
  ifc2tiles-report.json    部材数、除外した部材、形状が出なかった部材、警告、タイルごとのサイズ、処理時間
```

- タイルは階ごとの平面四分木で、大きい部材ほど上位のタイルに置く（`refine: ADD`）。遠くからは床・壁などの大きな部材だけが読み込まれる。
- featureは`IfcProduct`の出現1つ。開口、室（既定）、構造解析、型は除く。集約の部品は親の部材にまとめる。
- 列は`expressId`、`ifcClass`、`globalId`、`name`、`description`、`objectType`、`tag`、`predefinedType`、`typeName`、`storeyName`、`storeyGlobalId`、`buildingName`と、Pset / Qtoごとの`<Pset名>__<プロパティ名>`。型のPsetを継承し、部材側の値で上書きする。
- 長さ・面積・体積・質量の値はSI単位（m、m²、m³、kg）に換算し、列の`description`に単位を書く。
- 英数字でないPset名・プロパティ名（日本語など）の列IDは`p_<ハッシュ>`になる。元の名前は列の`name`にある。

## スタイル式の約束事（CesiumJS）

値のない部材では、プロパティが`undefined`になる。数値と比較すると例外になり、**描画が止まる**ため、必ず確かめてから比較する。

```js
tileset.style = new Cesium.Cesium3DTileStyle({
  color: {
    conditions: [
      ["${BaseQuantities__Height} !== undefined && ${BaseQuantities__Height} > 2.5", "color('red')"],
      ["${Pset_WallCommon__IsExternal} === 'TRUE'", "color('blue')"],   // 真偽値はENUMの名前で比べる
      ["true", "color('white')"],
    ],
  },
});
```

- 真偽値（`IfcBoolean`・`IfcLogical`）は`'TRUE'`・`'FALSE'`・`'UNKNOWN'`の文字列として返る。
- `Number(${x}) > 10`でも比較できる（`undefined`は`NaN`になり偽）。

## 開発

```bash
scripts/fetch-testdata.sh                               # 外部テストデータ（commit固定、sha256照合）
scripts/check.sh                                        # fmt・clippy・テスト・依存の公開経過日数
scripts/acceptance-handmade.sh                          # 自作IFCの既知点をpyprojで検算し、validatorにかける
scripts/batch-convert.sh > out/batch.jsonl              # テストデータ全件の変換とvalidator
python3 -m http.server 8790 --bind 127.0.0.1            # http://127.0.0.1:8790/viewer/?tileset=/out/xxx/tileset.json
```

| パス | 内容 |
|---|---|
| `crates/ifc2tiles/src/` | `source`（ifc-liteでの読み込み。ifc-liteを使うのはここだけ）、`semantics`（featureにする部材）、`georef`（置き方の決定）、`geodesy`（ECEFと根のENU）、`tiling`（タイルの木）、`metadata`（列）、`glb`、`tileset`、`report`、`units` |
| `crates/ifc2tiles/tests/` | 自作IFCとBURKWILの結合テスト（出力をmeshoptで復号して検算） |
| `docs/design.md` | 設計書（受け入れ条件、座標変換の式、列とタイル分割の規則） |
| `testdata/handmade/` | 既知点に立方体を置いた検証用IFCと、国土地理院の計算による期待値（`expected.json`） |
| `tools/validator/` | 3d-tiles-validator 0.6.1（版固定） |
| `viewer/` | 確認用のCesiumJSビューア（`index.html`）と計測ページ |

依存は公開から3日以上たった版を完全一致で固定する（`scripts/check-crate-age.py`）。meshopt crateはC++のmeshoptimizerをビルドするため、C++コンパイラが必要。

## 対象外

3D Tiles 2.0、日本以外のCRS（`--origin`で置く）、インスタンス化、テクスチャ、遠景用の簡略形状、輪郭線、複数IFCの統合、IFC4.3の線形配置の形状（ifc-liteが未対応）。
