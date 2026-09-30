# ifc-tiler

IFC（IFC2x3 / IFC4 / IFC4X3）を、部材ごとに選択・属性参照・属性スタイルができる3D Tiles 1.1へ変換するRust CLI。

```bash
cargo build --release
target/release/ifc_tiler model.ifc -o out/model
```

`out/model/tileset.json`をCesiumJSなどで読み込むと、地図上の正しい位置・向き・高さに表示され、部材をクリックするとGlobalId、IFCクラス、名前、型名、所属階、Pset / Qtoの値が取れる。

## 配置（ジオリファレンス）

次の順に、最初に見つかったもので置く。

| 順 | 情報 | 置き方 |
|---|---|---|
| 1 | `--origin LAT,LON[,H]` | その点を原点とする東・北・上（ENU） |
| 2 | `--map-conversion E,N[,H[,ROT]]`（`--crs`が必要） | 局所座標（m）を、局所原点の地図座標(E, N, 正標高H)と回転ROT°（局所X軸から東へ反時計回り）で平面直角座標へ置き、頂点ごとに逆投影 |
| 3 | `IfcMapConversion` / `IfcMapConversionScaled` | 平面直角座標へ変換し、頂点ごとに逆投影 |
| 4 | `IfcSite`の経緯度（(0,0)は未設定とみなす） | その経緯度を原点とするENU（`TrueNorth`で回転） |

- 対応するCRSは日本の平面直角座標系（JGD2011・JGD2000、I〜XIX系、鉛直を含む複合CRS）。それ以外は`--origin`で置く。
- 正標高は、国土地理院の「JPGEO2024＋Hrefconv2024」（離島の標高基準面の補正を含む）のジオイド高を足して楕円体高にする。
- `IfcMapConversion.Scale`が逆数で書かれたファイル（公式サンプルにもある）は、検出して直し、警告する。
- ENUで置いたのに形状が原点から1 km以上離れている場合は、局所座標が平面直角座標の値である疑いがあるため警告する。局所座標がそのまま平面直角座標の値なら`--map-conversion 0,0 --crs EPSG:xxxx`で置ける（ジオリファレンスを持たない土木のIFCに多い）。
- `IfcSite`の経緯度が、オーサリングツールの既定値とみられる値（Revitの既定の場所〔北緯42.4149°・西経71.2581°〕、Revitの都市リストの東京〔北緯35.6850°・東経139.7510°〕）と一致すると警告する。そのまま置くが、実際の位置ではない可能性が高いため、`--origin`か`--map-conversion`で置き直す。

## オプション

| オプション | 内容 |
|---|---|
| `-o, --output <DIR>` | 出力先（必須）。空のディレクトリを指定する（前回のタイルは消さない） |
| `--crs <EPSG>` | 地図座標のCRS。`IfcMapConversion`のTargetCRSを上書きする |
| `--origin <LAT,LON[,H]>` | ファイルのジオリファレンスを使わず、この点に置く |
| `--map-conversion <E,N[,H[,ROT]]>` | ファイルのジオリファレンスを使わず、局所原点を地図座標のこの点に置く。`--crs`が必要。`--origin`とは同時に指定できない |

警告は標準エラーに出す。終了コード: 成功0、入力・設定の誤り2、変換できる部材がない3、その他1。

## 出力

```text
out/
  tileset.json             3D Tiles 1.1。schema（storey・element）、groups（階）
  tiles/<階>_<四分木>.glb   glTF 2.0。EXT_mesh_features、EXT_structural_metadata、KHR_mesh_quantization、EXT_meshopt_compression、
                           （同形メッシュがあるタイルは）EXT_mesh_gpu_instancing、EXT_instance_features
```

- タイルは階ごとの平面四分木（1タイル最大200部材）で、大きい部材ほど上位のタイルに置く（`refine: ADD`）。遠くからは床・壁などの大きな部材だけが読み込まれる。
- 1タイルの形状は、不透明・半透明の2つのprimitiveにまとめ、色は頂点色（`COLOR_0`）で持つ。タイル内で同じ形（平行移動だけ違い、差が0.5 mm以内）のメッシュが3個以上、かつ200頂点以上なら、テンプレート1つとインスタンスにする。
- 配信するときは、サーバーで`.glb`と`.json`を`Content-Encoding: gzip`（またはbrotli）で返す。meshoptの出力はgzipでさらに3〜4分の1程度になる。
- featureは`IfcProduct`の出現1つ。開口、室（`IfcSpace`）、構造解析、型は除く。集約の部品は親の部材にまとめる。
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
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --release --locked
```

| パス | 内容 |
|---|---|
| `crates/ifc-tiler/src/` | `source`（ifc-liteでの読み込み。ifc-liteを使うのはここだけ）、`semantics`（featureにする部材）、`georef`（置き方の決定）、`geodesy`（ECEFと根のENU）、`tiling`（タイルの木）、`metadata`（列）、`glb`、`tileset`、`units` |
| `crates/ifc-tiler/tests/` | 自作IFCの結合テスト（出力をmeshoptで復号し、既知点の位置と属性を検算） |
| `testdata/handmade/` | 既知点に立方体を置いた検証用IFCと、国土地理院の計算による期待値（`expected.json`） |

依存は公開から3日以上たった版を完全一致で固定する。meshopt crateはC++のmeshoptimizerをビルドするため、C++コンパイラが必要。

## 対象外

3D Tiles 2.0、日本以外のCRS（`--origin`で置く）、`IfcRigidOperation`（`--origin`か`--map-conversion`で置く）、回転の違う同形メッシュのインスタンス化、テクスチャ、遠景用の簡略形状、輪郭線、複数IFCの統合、IFC4.3の線形配置の形状（ifc-liteが未対応）。
