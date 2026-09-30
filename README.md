# ifc-tiler

IFC（IFC2x3 / IFC4 / IFC4X3）のBIMモデルを、部材ごとに選択・属性参照・属性スタイルができる3D Tiles 1.1へ変換するRust製のCLIです。

```bash
ifc_tiler model.ifc -o out/model
```

## 目的

BIMモデルをWebの地図上で見たい場面は多い一方で、IFCをそのまま地図に載せるには次の問題があります。

- **位置**: IFCの座標は建物ごとの局所座標で、地図上のどこに置くかはファイルのジオリファレンス（`IfcMapConversion`・`IfcSite`の経緯度など）で決まります。書き方はツールやファイルによってまちまちで、欠けていることも、誤っていることもあります。
- **部材の情報**: 見た目だけのメッシュにすると、壁・柱・扉といった部材の区別や、GlobalId・Pset・Qtoなどの属性が失われます。
- **大きさ**: 実務のモデルは部材数が数千〜数万あり、1つのファイルにまとめるとブラウザで扱いにくくなります。

ifc-tilerは、IFCを次のような3D Tilesにします。

- **正しい位置・向き・高さ**: 日本の平面直角座標系（JGD2011）を正しく扱い、頂点ごとに経緯度へ逆投影します。正標高は国土地理院のジオイドモデルで楕円体高にします。
- **部材を単位にする**: 部材1つが3D Tilesのfeature 1つになり、クリックするとGlobalId、IFCクラス、名前、型名、所属階、Pset / Qtoの値を読めます。属性の値で色分けすることもできます。
- **軽く、段階的に読み込める**: 階ごとに四分木のタイルに分け、大きな部材から読み込みます。形状は量子化とmeshopt圧縮で小さくします。

CesiumJSなど、3D Tiles 1.1（`EXT_mesh_features`・`EXT_structural_metadata`）に対応したビューアで表示できます。

## インストール

必要なもの:

- Rust 1.95（`rust-toolchain.toml`で固定しているため、[rustup](https://rustup.rs/)があれば自動で入ります）
- C++コンパイラ（meshopt crateが同梱のmeshoptimizerをビルドするため。macOSならXcode Command Line Tools）

```bash
git clone https://github.com/nokonoko1203/ifc-tiler.git
cd ifc-tiler
cargo build --release
```

実行ファイルは`target/release/ifc_tiler`にできます。ジオイドモデルを埋め込んでいるため、実行時にネットワークや追加のファイルは要りません。

## 使い方

```text
ifc_tiler <INPUT> -o <OUTPUT_DIR> [--crs <EPSG>] [--origin <LAT,LON[,H]>] [--map-conversion <E,N[,H[,ROT]]>]
```

| 引数・オプション | 内容 |
|---|---|
| `<INPUT>` | 入力するIFCファイル |
| `-o, --output <DIR>` | 出力先のディレクトリ（必須）。空のディレクトリを指定する（前回のタイルは消さない） |
| `--crs <EPSG>` | 地図座標のCRS（例: `EPSG:6677`、`6677`）。ファイルの`IfcMapConversion`のTargetCRSを上書きする。`--map-conversion`では必須 |
| `--origin <LAT,LON[,H]>` | ファイルのジオリファレンスを使わず、この緯度・経度（度）・正標高（m、省略時0）を原点として、局所座標を東・北・上に置く |
| `--map-conversion <E,N[,H[,ROT]]>` | ファイルのジオリファレンスを使わず、局所原点を地図座標の東E・北N（m）・正標高H（m）に置き、局所X軸を東からROT度（反時計回り）回す。`--crs`が必要。`--origin`とは同時に指定できない |

成功すると、部材数・階数・タイル数・サイズ・処理時間を1行で表示します。警告は標準エラーに`warning:`で始まる行として出します。

終了コード: 成功0、入力・設定の誤り2、変換できる部材がない3、その他（書き込みの失敗など）1。

### 位置の決め方

ifc-tilerは、次の順に最初に見つかった情報で置きます。

| 順 | 使う情報 | 置き方 |
|---|---|---|
| 1 | `--origin` | その点を原点とする東・北・上（ENU） |
| 2 | `--map-conversion`と`--crs` | 局所座標（m）を平面直角座標に置き、頂点ごとに逆投影 |
| 3 | ファイルの`IfcMapConversion` / `IfcMapConversionScaled` | 平面直角座標へ変換し、頂点ごとに逆投影 |
| 4 | ファイルの`IfcSite`の経緯度（(0, 0)は未設定とみなす） | その経緯度を原点とするENU（`TrueNorth`で回転） |

どれもなければ、エラー（終了コード2）で止まります。

### 例

ジオリファレンスのあるIFCは、そのまま変換できます。

```bash
ifc_tiler building.ifc -o out/building
```

`IfcMapConversion`にCRSの名前がない、または名前からEPSGコードが分からない場合は、`--crs`で指定します。

```bash
ifc_tiler building.ifc -o out/building --crs EPSG:6677
```

局所座標がそのまま平面直角座標の値になっているIFC（ジオリファレンスを持たない土木のモデルに多い）は、局所原点を地図座標の(0, 0)に置きます。

```bash
ifc_tiler civil.ifc -o out/civil --map-conversion 0,0 --crs EPSG:6677
```

ジオリファレンスがない、または誤っているIFCは、置きたい場所の緯度・経度・正標高を与えます。

```bash
ifc_tiler building.ifc -o out/building --origin 35.681236,139.767125,3.5
```

## 出力

```text
out/building/
  tileset.json             3D Tiles 1.1。schema（storey・element）、groups（階）
  tiles/<階>_<四分木>.glb   glTF 2.0。EXT_mesh_features、EXT_structural_metadata、KHR_mesh_quantization、
                           EXT_meshopt_compression、（同形メッシュがあるタイルは）EXT_mesh_gpu_instancing、EXT_instance_features
```

### 部材と属性

- featureは`IfcProduct`の出現1つです。開口（`IfcOpeningElement`）、室（`IfcSpace`）、構造解析、型は出力しません。集約の部品（多層壁の層など）は親の部材にまとめます。
- 各部材は次の列を持ちます。

  | 列 | 内容 |
  |---|---|
  | `expressId` | STEPファイル内のエンティティ番号 |
  | `ifcClass` | IFCクラス（`IfcWall`など） |
  | `globalId` | GlobalId |
  | `name`、`description`、`objectType`、`tag`、`predefinedType` | 部材の属性 |
  | `typeName` | 型（`IfcRelDefinesByType`）の名前 |
  | `storeyName`、`storeyGlobalId` | 所属階。所属階がなければ`(unassigned)`の階にまとめる |
  | `buildingName` | 所属する建物 |
  | `<Pset名>__<プロパティ名>` | Pset / Qtoの値。型のPsetを継承し、部材側の値で上書きする |

- 長さ・面積・体積・質量の値はSI単位（m、m²、m³、kg）に換算し、列の`description`に単位を書きます。
- 整数はINT32、実数はFLOAT64、文字列はSTRING、真偽値（`IfcBoolean`・`IfcLogical`）はENUMになります。
- 英数字でないPset名・プロパティ名（日本語など）の列IDは`p_<ハッシュ>`になります。元の名前は列の`name`にあります。
- 階は`groups`にあり、名前・GlobalId・標高を持ちます。

### タイルと形状

- タイルは階ごとの平面四分木（1タイル最大200部材）で、大きい部材ほど上位のタイルに置きます（`refine: ADD`）。遠くからは床・壁などの大きな部材だけが読み込まれ、近づくと小さな部材が足されます。
- 1タイルの形状は不透明・半透明の2つのprimitiveにまとめ、色は頂点色（`COLOR_0`）で持ちます。
- タイル内で同じ形（平行移動だけが違い、形状の最小点からの相対座標を1 mm単位に丸めると一致する）のメッシュが3個以上、かつ200頂点以上なら、テンプレート1つとインスタンスにします。

## CesiumJSで表示する

出力ディレクトリをHTTPで配信し、`tileset.json`を読み込みます。

```js
const viewer = new Cesium.Viewer("viewer");
const tileset = await Cesium.Cesium3DTileset.fromUrl("/out/building/tileset.json");
viewer.scene.primitives.add(tileset);
await viewer.zoomTo(tileset);

// クリックした部材の属性を表示する
const handler = new Cesium.ScreenSpaceEventHandler(viewer.scene.canvas);
handler.setInputAction((ev) => {
  const picked = viewer.scene.pick(ev.position);
  if (picked instanceof Cesium.Cesium3DTileFeature) {
    for (const id of picked.getPropertyIds()) {
      console.log(id, picked.getProperty(id));
    }
  }
}, Cesium.ScreenSpaceEventType.LEFT_CLICK);
```

高さは楕円体高で書いてあるため、地形と重ねるときは楕円体高の地形を使います。日本国内なら、トークン不要のPLATEAU-Terrainが使えます。

```js
viewer.terrainProvider = await Cesium.CesiumTerrainProvider.fromUrl("https://tile.plateauview.mlit.go.jp/terrain");
viewer.scene.globe.depthTestAgainstTerrain = true;
```

### スタイル式の注意

値のない部材では、プロパティが`undefined`になります。`undefined`と数値を比較すると例外になり、**描画が止まる**ため、必ず確かめてから比較します。

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

- 真偽値は`'TRUE'`・`'FALSE'`・`'UNKNOWN'`の文字列として返ります。
- `Number(${x}) > 10`でも比較できます（`undefined`は`NaN`になり、比較は偽）。

### 配信

サーバーで`.glb`と`.json`を`Content-Encoding: gzip`（またはbrotli）で返すと、転送量がさらに3〜4分の1程度になります。

## 座標変換の詳細

- 対応するCRSは日本の平面直角座標系（JGD2011・JGD2000、I〜XIX系、鉛直を含む複合CRS）です。JGD2000はJGD2011と同じとみなし、警告します。旧日本測地系（Tokyo Datum）は対応していません。日本以外の場所は`--origin`で置きます。
- 正標高には、国土地理院の「JPGEO2024＋Hrefconv2024」（離島の標高基準面の補正を含む）のジオイド高を足して楕円体高にします。ジオイドモデルの範囲外（海外など）ではジオイド高を0とし、警告します。
- 平面直角座標で置く場合は頂点ごとに逆投影するため、子午線収差（真北と座標北のずれ）や縮尺係数も反映されます。
- 地球上の座標（ECEF）は、全頂点の外接箱の中心を原点とする東・北・上の座標系に変換して書き、`tileset.json`の`root.transform`でECEFに戻します。

## 警告とその対処

| 警告 | 意味 | 対処 |
|---|---|---|
| 形状が局所原点から最大○ m離れている | ENUで置いたが、形状が原点から1 km以上離れている。局所座標が平面直角座標の値である疑いがある | `--map-conversion 0,0 --crs EPSG:xxxx` |
| IfcSiteの経緯度が、Revitの既定の場所・東京と一致する | オーサリングツールの既定値（Revitの既定の場所〔北緯42.4149°・西経71.2581°〕、Revitの都市リストの東京〔北緯35.6850°・東経139.7510°〕）のままで、実際の位置ではない可能性が高い | `--origin`か`--map-conversion`で置き直す |
| IfcSiteの経緯度が(0, 0)のため、未設定とみなした | 経緯度が入っていない | ほかにジオリファレンスがなければエラーになるので、`--origin`か`--map-conversion`で置く |
| IfcMapConversion.Scaleの実効倍率が○で、逆数で書かれているとみなし○を使う | `Scale`が逆数で書かれたファイル（公式サンプルにもある）を検出して直した | なし（結果の位置を確かめる） |
| IfcMapConversion.Scaleの実効倍率が○で、1から1%以上ずれている | 縮尺が不自然 | ファイルのジオリファレンスを確かめる |
| EPSG:○（JGD2000）をJGD2011と同じとみなした | JGD2000の座標をJGD2011として扱った（東日本を中心に、場所により最大数mの差がある） | 精度が要るならJGD2011の座標にする |
| ○がジオイドモデルの範囲外で、ジオイド高を0とした | 日本国外、またはジオイドモデルのない海域 | 高さが正標高のままでよいか確かめる |
| ファイルのIfcMapConversionは使わず、--map-conversionで置いた | 指定どおり | なし |

## 対象外

3D Tiles 2.0、日本以外のCRS（`--origin`で置く）、`IfcRigidOperation`（`--origin`か`--map-conversion`で置く）、回転の違う同形メッシュのインスタンス化、テクスチャ、遠景用の簡略形状、輪郭線、複数IFCの統合、IFC4.3の線形配置の形状（ifc-liteが未対応）。

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

依存crateは、公開から3日以上たった版を完全一致で固定しています。IFCの読み込みと形状の生成には[ifc-lite](https://crates.io/crates/ifc-lite-core)を使っています。
