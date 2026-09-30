# ifc-tiler

IFC（IFC2x3 / IFC4 / IFC4X3）のBIMモデルを、部材ごとに選択・属性参照・属性スタイルができる3D Tiles 1.1へ変換するRust製のCLIです。

```bash
ifc_tiler model.ifc -o out/model
```

## 目的

BIMモデルをWebの地図上で見たい場面は多い一方で、IFCをそのまま地図に載せるには次の問題があります。

- **位置**: IFCの座標は建物ごとの局所座標で、地図上のどこに置くかはファイルのジオリファレンス（`IfcMapConversion`・`IfcSite`の経緯度など）で決まります。書き方はツールやファイルによってまちまちで、欠けていることも、誤っていることもあります。
- **部材の情報**: 見た目だけのメッシュにすると、壁・柱・扉といった部材の区別や、部材ごとの固有ID、部材に付いた属性（耐火性能や外壁かどうかといった特性、長さ・面積・体積などの数量）が失われます。
- **大きさ**: 実務のモデルは部材数が数千〜数万あり、1つのファイルにまとめるとブラウザで扱いにくくなります。

ifc-tilerは、IFCを次のような3D Tilesにします。

- **正しい位置・向き・高さ**: ファイルのジオリファレンスをもとに、頂点ごとに地図座標から経緯度・高さへ変換し、地球上の正しい位置に置きます。座標変換には[PROJ](https://proj.org/)を使うため、EPSGコードで表せる座標系ならどれでも扱えます。
- **部材を単位にする**: 部材1つが3D Tilesの地物1つになり、クリックすると部材の固有ID、IFCでの種類（壁・柱など）、名前、型の名前、所属する階、属性と数量の値を読めます。属性の値で色分けすることもできます。
- **軽く、段階的に読み込める**: 階ごとに四分木のタイルに分け、大きな部材から読み込みます。形状は量子化とmeshopt圧縮で小さくします。

CesiumJSなど、3D Tiles 1.1（`EXT_mesh_features`・`EXT_structural_metadata`）に対応したビューアで表示できます。

## インストール

必要なもの:

- Rust 1.95（`rust-toolchain.toml`で固定しているため、[rustup](https://rustup.rs/)があれば自動で入ります）
- C++コンパイラ（meshopt crateが同梱のmeshoptimizerをビルドするため。macOSならXcode Command Line Tools）
- PROJ 9.6.2以上（座標変換に使う。macOSなら`brew install proj`、Debian / Ubuntuなら`apt install libproj-dev`）。見つからない場合はソースからビルドするため、CMakeとSQLiteが要ります

```bash
git clone https://github.com/nokonoko1203/ifc-tiler.git
cd ifc-tiler
cargo build --release
```

実行ファイルは`target/release/ifc_tiler`にできます。

ジオイドなどのグリッドは、使うときに[cdn.proj.org](https://cdn.proj.org/)から取得し、キャッシュします（macOSなら`~/Library/Application Support/proj`）。ネットワークに接続できない環境では、`projsync`で必要なグリッドを事前に取得しておきます。

## 使い方

```text
ifc_tiler <INPUT> -o <OUTPUT_DIR> [--crs <CRS>] [--origin <LAT,LON[,H]>] [--map-conversion <E,N[,H[,ROT]]>]
```

| 引数・オプション | 内容 |
|---|---|
| `<INPUT>` | 入力するIFCファイル |
| `-o, --output <DIR>` | 出力先のディレクトリ（必須）。空のディレクトリを指定する（前回のタイルは消さない） |
| `--crs <CRS>` | 座標系（例: `EPSG:32654`、`6677`、高さの基準を含めるなら`EPSG:6677+6695`）。地図座標で置くときは、ファイルの`IfcMapConversion`のTargetCRSを上書きする（`--map-conversion`では必須）。`--origin`・`IfcSite`で置くときは、緯度・経度・高さの座標系になる（省略時は`EPSG:4326`） |
| `--origin <LAT,LON[,H]>` | ファイルのジオリファレンスを使わず、この緯度・経度（度）・標高（m、省略時0）を原点として、局所座標を東・北・高さに置く |
| `--map-conversion <E,N[,H[,ROT]]>` | ファイルのジオリファレンスを使わず、局所原点を地図座標の東E・北N（m）・標高H（m）に置き、局所X軸を東からROT度（反時計回り）回す。`--crs`が必要。`--origin`とは同時に指定できない |

成功すると、部材数・階数・タイル数・サイズ・処理時間を1行で表示します。警告は標準エラーに`warning:`で始まる行として出します。

終了コード: 成功0、入力・設定の誤り2、変換できる部材がない3、その他（書き込みの失敗など）1。

### 位置の決め方

ifc-tilerは、次の順に最初に見つかった情報で置きます。

| 順 | 使う情報 | 置き方 |
|---|---|---|
| 1 | `--origin` | その点を原点とする東・北・高さ（ENU） |
| 2 | `--map-conversion`と`--crs` | 局所座標（m）を地図座標に置き、頂点ごとに経緯度へ変換 |
| 3 | ファイルの`IfcMapConversion` / `IfcMapConversionScaled` | 地図座標へ変換し、頂点ごとに経緯度へ変換 |
| 4 | ファイルの`IfcSite`の経緯度（(0, 0)は未設定とみなす） | その経緯度を原点とするENU（`TrueNorth`で回転） |

どれもなければ、エラー（終了コード2）で止まります。

### 高さの基準

IFCの高さ（標高）は、国や地域ごとの基準で測られています。地球上の正しい高さ（楕円体高）にするには、その基準のジオイドが要るため、高さの基準を含むCRS（複合CRS）を`--crs`で与えます。複合CRSは、水平の座標系と高さの基準を`+`でつなぐか、EPSGに登録されたコードで書きます。

| 例 | 意味 |
|---|---|
| `EPSG:6677+6695`、`EPSG:10170` | JGD2011 / 平面直角座標系IX系 + JGD2011の標高 |
| `EPSG:6697` | JGD2011の経緯度 + JGD2011の標高（`--origin`・`IfcSite`用） |
| `EPSG:25832+7837` | ETRS89 / UTM 32N + ドイツの標高（DHHN2016） |

高さの基準がないCRS（`EPSG:6677`など）では、高さを世界のジオイドモデルEGM2008による標高とみなし、警告します。国の基準とEGM2008の差（数十cm〜1 m程度）が残ります。

グリッドを取得できないときは、グリッドを使わない近似の変換で出力し、警告します。この場合、高さがジオイド高の分（世界で−105〜+85 m）ずれることがあります。

### 例

ジオリファレンスのあるIFCは、そのまま変換できます。

```bash
ifc_tiler building.ifc -o out/building
```

`IfcMapConversion`にCRSの名前がない、または名前からEPSGコードが分からない場合は、`--crs`で指定します。

```bash
ifc_tiler building.ifc -o out/building --crs EPSG:6677+6695
```

局所座標がそのまま地図座標の値になっているIFC（ジオリファレンスを持たない土木のモデルに多い）は、局所原点を地図座標の(0, 0)に置きます。

```bash
ifc_tiler civil.ifc -o out/civil --map-conversion 0,0 --crs EPSG:6677+6695
```

ジオリファレンスがない、または誤っているIFCは、置きたい場所の緯度・経度・標高を与えます。

```bash
ifc_tiler building.ifc -o out/building --origin 35.681236,139.767125,3.5 --crs EPSG:6697
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
  | `expressId` | ファイル内のID |
  | `ifcClass` | IFCクラス（`IfcWall`など） |
  | `globalId` | 部材の固有ID（IFCのGlobalId） |
  | `name`、`description`、`objectType`、`tag`、`predefinedType` | 部材の属性 |
  | `typeName` | 型（`IfcRelDefinesByType`）の名前 |
  | `storeyName`、`storeyGlobalId` | 所属階。所属階がなければ`(unassigned)`の階にまとめる |
  | `buildingName` | 所属する建物 |
  | `<Pset名>__<プロパティ名>` | 属性（IFCのプロパティセット、Pset）と数量（IFCの数量セット、Qto）の値。型に付いた値を引き継ぎ、部材側の値で上書きする |

- 長さ・面積・体積・質量の値はSI単位（m、m²、m³、kg）に換算し、列の`description`に単位を書きます。
- 整数はINT32、実数はFLOAT64、文字列はSTRING、真偽値（`IfcBoolean`・`IfcLogical`）はENUMになります。
- 英数字でないプロパティセット名・プロパティ名（日本語など）の列IDは`p_<ハッシュ>`になります。元の名前は列の`name`にあります。
- 階は`groups`にあり、名前・固有ID・標高を持ちます。

### タイルと形状

- タイルは階ごとの平面四分木（1タイル最大200部材）で、大きい部材ほど上位のタイルに置きます（`refine: ADD`）。遠くからは床・壁などの大きな部材だけが読み込まれ、近づくと小さな部材が足されます。
- 1タイルの形状は不透明・半透明の2つのprimitiveにまとめ、色は頂点色（`COLOR_0`）で持ちます。
- タイル内で同じ形（平行移動だけが違い、形状の最小点からの相対座標を1 mm単位に丸めると一致する）のメッシュが3個以上、かつ200頂点以上なら、テンプレート1つとインスタンスにします。

## 対象外

3D Tiles 2.0、`IfcRigidOperation`（`--origin`か`--map-conversion`で置く）、回転の違う同形メッシュのインスタンス化、テクスチャ、遠景用の簡略形状、輪郭線、複数IFCの統合、IFC4.3の線形配置の形状（ifc-liteが未対応）。
