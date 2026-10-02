# ifc-tiler

IFC（IFC2x3 / IFC4 / IFC4X3）のBIMモデルを、部材ごとに選択・属性参照・属性スタイルができる3D Tiles 1.1へ変換するRust製のCLIです。

[English](./README.md)|**日本語**

```bash
ifc_tiler model.ifc -o out/model
```

## 目的

IFCの座標は建物ごとの局所座標で、地図上のどこに置くかはファイルのジオリファレンス（`IfcMapConversion`や`IfcSite`の経緯度）で決まります。ただ、その書き方はツールやファイルによってばらばらで、抜けていたり間違っていたりもします。また、IFCを見た目だけのメッシュにすると、壁・柱・扉の区別も、部材の固有IDや耐火性能・面積といった属性も失われます。実務のモデルは部材が数千〜数万あり、1つのファイルにまとめるとブラウザでは重くなります。

ifc-tilerは、ジオリファレンスをもとに頂点ごとに地図座標を経緯度と高さへ変換し、モデルを地球上の正しい位置・向き・高さに置きます。座標変換には[PROJ](https://proj.org/)を使うので、EPSGコードで表せる座標系ならどれでも扱えます。

部材1つが3D Tilesの地物1つになり、クリックすると固有ID、IFCでの種類（壁・柱など）、名前、型、所属階、属性と数量の値を読めます。属性の値で色分けもできます。タイルは階ごとの四分木に分けて大きな部材から読み込み、形状は量子化とmeshopt圧縮で小さくしています。

表示には、3D Tiles 1.1（`EXT_mesh_features`・`EXT_structural_metadata`）に対応したCesiumJSなどのビューアを使います。

## インストール

### ビルド済みバイナリ

[Releases](https://github.com/nokonoko1203/ifc-tiler/releases)に、macOS（Apple Silicon / Intel）、Linux（x86_64 / aarch64）、Windows（x86_64）のバイナリを置いています。PROJとSQLiteを静的リンクしているので、RustもPROJも要りません。

```bash
# macOS / Linux
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/nokonoko1203/ifc-tiler/releases/latest/download/ifc-tiler-installer.sh | sh
```

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://github.com/nokonoko1203/ifc-tiler/releases/latest/download/ifc-tiler-installer.ps1 | iex"
```

ビルド済みバイナリは、ジオイドなどのグリッドを使えません（同梱のPROJにグリッドの取得と読み込みの機能がありません）。グリッドが必要な変換ではグリッドを使わない近似になり、`warning:`で知らせます。標高から楕円体高への変換にはジオイド（CRSに高さの基準がなければEGM2008）を使うので、ビルド済みバイナリではほぼすべての入力で、ジオイド高が足されず高さがその分（東京付近で約37 m）ずれます。正確な高さが必要な場合は、次の手順でソースからビルドしてください。

### ソースからビルドする

必要なもの:

- Rust 1.95。`rust-toolchain.toml`で版を固定しているので、[rustup](https://rustup.rs/)があれば自動で入ります
- C++コンパイラ。meshopt crateが同梱のmeshoptimizerをビルドします（macOSならXcode Command Line Tools）
- PROJ 9.6.2以上。macOSなら`brew install proj`、Debian / Ubuntuなら`apt install libproj-dev`で入ります。見つからないとソースからビルドするので、その場合はCMakeとSQLiteも必要です

```bash
git clone https://github.com/nokonoko1203/ifc-tiler.git
cd ifc-tiler
cargo build --release
```

実行ファイルは`target/release/ifc_tiler`にできます。

ジオイドなどのグリッドは、必要になった時点で[cdn.proj.org](https://cdn.proj.org/)から取得してキャッシュします（macOSなら`~/Library/Application Support/proj`）。ネットワークにつながらない環境で使うときは、先に`projsync`でグリッドを取得しておいてください。

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

変換に成功すると、部材数・階数・タイル数・サイズ・処理時間を1行で表示します。警告は`warning:`で始まる行として標準エラーに出ます。

終了コード: 成功0、入力・設定の誤り2、変換できる部材がない3、その他（書き込みの失敗など）1。

### 位置の決め方

次の順に探し、最初に見つかった情報で置きます。

| 順 | 使う情報 | 置き方 |
|---|---|---|
| 1 | `--origin` | その点を原点とする東・北・高さ（ENU） |
| 2 | `--map-conversion`と`--crs` | 局所座標（m）を地図座標に置き、頂点ごとに経緯度へ変換 |
| 3 | ファイルの`IfcMapConversion` / `IfcMapConversionScaled` | 地図座標へ変換し、頂点ごとに経緯度へ変換 |
| 4 | ファイルの`IfcSite`の経緯度（(0, 0)は未設定とみなす） | その経緯度を原点とするENU（`TrueNorth`で回転） |

どれもなければエラー（終了コード2）になります。

### 高さの基準

IFCの高さ（標高）は、国や地域ごとの基準で測られています。これを楕円体高に直すにはその基準のジオイドが必要なので、高さの基準を含むCRS（複合CRS）を`--crs`で指定します。複合CRSは、水平の座標系と高さの基準を`+`でつなぐか、EPSGに登録されたコードで書きます。

| 例 | 意味 |
|---|---|
| `EPSG:6677+6695`、`EPSG:10170` | JGD2011 / 平面直角座標系IX系 + JGD2011の標高 |
| `EPSG:6697` | JGD2011の経緯度 + JGD2011の標高（`--origin`・`IfcSite`用） |
| `EPSG:25832+7837` | ETRS89 / UTM 32N + ドイツの標高（DHHN2016） |

`EPSG:6677`のように高さの基準を含まないCRSでは、高さを世界のジオイドモデルEGM2008による標高とみなし、警告を出します。この場合、国の基準とEGM2008の差（数十cm〜1 m程度）が残ります。

グリッドを取得できなかったときは、グリッドを使わない近似の変換で出力し、警告を出します。このとき高さは、ジオイド高の分（世界で−105〜+85 m）ずれることがあります。

### 例

ジオリファレンスのあるIFCは、そのまま変換できます。

```bash
ifc_tiler building.ifc -o out/building
```

`IfcMapConversion`にCRSの名前がない、または名前からEPSGコードが分からない場合は、`--crs`で指定します。

```bash
ifc_tiler building.ifc -o out/building --crs EPSG:6677+6695
```

局所座標がそのまま地図座標の値になっているIFCは、局所原点を地図座標の(0, 0)に置きます。ジオリファレンスを持たない土木のモデルによくある形です。

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

- `IfcProduct`1つが地物（feature）1つになります。開口（`IfcOpeningElement`）、室（`IfcSpace`）、構造解析の要素、型は出力しません。多層壁の層のような集約の部品は、親の部材にまとめます。
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

- 長さ・面積・体積・質量はSI単位（m、m²、m³、kg）に換算し、単位を列の`description`に書きます。
- 整数はINT32、実数はFLOAT64、文字列はSTRING、真偽値（`IfcBoolean`・`IfcLogical`）はENUMになります。
- プロパティセット名やプロパティ名に英数字以外（日本語など）を含む列は、IDが`p_<ハッシュ>`になります。元の名前は列の`name`に残ります。
- 階の情報（名前・固有ID・標高）は`groups`に入ります。

### タイルと形状

- タイルは階ごとの平面四分木で、1タイルに最大200部材を入れます。大きい部材ほど上位のタイルに置く（`refine: ADD`）ので、遠くからは床や壁などの大きな部材だけが読み込まれ、近づくにつれて小さな部材が加わります。
- 1タイルの形状は、不透明と半透明の2つのprimitiveにまとめます。色は頂点色（`COLOR_0`）で持ちます。
- タイルの中に同じ形のメッシュが3個以上あり、200頂点以上ある場合は、テンプレート1つとそのインスタンスとして出力します。同じ形とみなすのは、平行移動だけが違い、形状の最小点からの相対座標を1 mm単位に丸めたときに一致するものです。

## 対応していないもの

3D Tiles 2.0、`IfcRigidOperation`（`--origin`か`--map-conversion`で置いてください）、回転の違う同形メッシュのインスタンス化、テクスチャ、遠景用の簡略形状、輪郭線、複数IFCの統合、IFC4.3の線形配置の形状（ifc-liteが未対応）には対応していません。
