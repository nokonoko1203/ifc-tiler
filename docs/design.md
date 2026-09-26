# ifc2tiles 第1版 設計書

作成日: 2026-09-26
状態: 承認済み。第1版の実装はこの設計書に従う
根拠の調査資料は作業用リポジトリの`.tmp/research/`にある（このリポジトリには含めない）
根拠: 統合調査（`20260926_ifc3dtiles_00_report.md`）、実現性の判定（`20260926_ifc3dtiles_10_feasibility.md`）、ifc-liteの検証（`20260926_ifc3dtiles_11_ifclite_verification.md`）、ジオリファレンスの検証（`20260926_ifc3dtiles_12_georef_verification.md`）、CesiumJSの負荷（`20260926_ifc3dtiles_13_cesium_metadata_load.md`）、MVPの記録（`20260926_ifc3dtiles_20_mvp_record.md`）、レビューと実験（`20260926_ifc3dtiles_30_review_and_experiments.md`）

## 1. 目的とOutcome

1つのIFCファイル（IFC2x3 / IFC4 / IFC4X3）を、部材ごとに選択・属性参照・属性スタイルができる3D Tiles 1.1へ変換するCLIを提供する。

利用者が観測できる完了状態:

- `ifc2tiles model.ifc -o out/` の1コマンドで`out/tileset.json`と`out/tiles/*.glb`ができ、CesiumJSで地図上の正しい位置・向き・高さに表示される。
- 部材をクリックすると、GlobalId、IFCクラス、名前、型名、所属階、建物名、Pset / Qtoの値が取れる。多層壁のように部品で構成される部材も、親の部材として1つにまとまって選択できる。
- 遠くから見ると大きな部材だけが描かれ、近づくと小さな部材が現れる。
- 変換の経緯（使ったジオリファレンス、CRS、ジオイド、警告、除外した部材）が`out/ifc2tiles-report.json`と`tileset.json`に残る。

## 2. スコープと非目標

**スコープ**: 上記Outcome。日本の平面直角座標系（JGD2011 / JGD2000、I〜XIX系、および鉛直を含む複合CRS）と、IfcSite経緯度・原点指定によるENU配置。

**非目標（第1版では作らない）**:

| 項目 | 理由 |
|---|---|
| 3D Tiles 2.0 / glTF 2.1 | Draftのため |
| 日本以外のCRS（PROJ連携） | 対象外。`--origin`によるENU配置で代替できる |
| インスタンス化（`EXT_mesh_gpu_instancing`） | 実験で頂点削減が5%程度 |
| テクスチャ | 実験データでほぼ使われていない。色（材料の拡散色）だけを出す |
| 遠景用の簡略形状（`REPLACE`） | 寸法によるADD階層で遠景の負荷は十分下がった |
| 輪郭線・AEC描画拡張 | 第2版以降 |
| サイドカー（関係・全属性の別ファイル） | 列の上限内に収まるため |
| 複数IFCの統合、差分更新、ストリーミング処理 | 第1版は1ファイルをメモリ上で処理する |
| 形状が出ない部材の補完（外接箱など） | 誤った形を出すより、件数を報告する |
| IFC4.3の線形配置の形状 | ifc-liteが未対応 |

## 3. 受け入れ条件

| No. | 条件 | 確認方法 |
|---|---|---|
| A1 | 自作IFC 3件の既知点（立方体の角）が、国土地理院の値（`testdata/handmade/expected.json`）と東西・南北・高さとも1 cm以内（`ifc4_map_conversion`、`ifc2x3_site_latlon`、`ifc2x3_plateau_origin --site-coords grid --crs EPSG:6677`） | Rustの結合テスト（出力GLBを復号して検算）と`scripts/acceptance-handmade.sh`（pyprojで独立に検算） |
| A2 | 誤った指定では既知点が外れる（`--geoid gsigeo2011`で高さ−0.0989 m、PLATEAU方式をENUで置くと100 m超）ことと、後者で警告が出ること | 同上 |
| A3 | テストデータ85件のうち、形状を持つ全ファイルが変換でき、3d-tiles-validatorのエラーが0 | `scripts/batch-convert.sh` |
| A4 | 自作IFC4の壁で、型から継承した`Pset_WallCommon.IsExternal = TRUE`と、部材側で上書きした`FireRating = OCC-120`が取れる。開口と室はfeatureにならない。机2台が2つのfeatureになる | 結合テスト |
| A5 | BURKWILの`IfcWall` 540本がすべてfeatureになり、Psetを持つ | 結合テスト（テストデータがある場合だけ実行） |
| A6 | CesiumJSで自作IFCとBURKWILを表示し、ピックで属性が取れ、コンソールにエラーがない | 内蔵ブラウザで確認 |
| A7 | 既定設定のBURKWILの出力が10 MB以下、変換が5秒以下（Apple Silicon） | `scripts/batch-convert.sh`のレポート |
| A8 | `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`が通り、`Cargo.lock`の全crateが公開3日以上 | `scripts/check.sh` |

## 4. CLI

```text
ifc2tiles <INPUT> -o <OUTPUT_DIR> [OPTIONS]

  --crs <EPSG>               地図座標のCRS（例: EPSG:6677）。IfcMapConversionのTargetCRSを上書きする。
                             --site-coords grid のときは必須
  --site-coords <enu|grid>   IfcSite経路で局所座標をどう解釈するか [既定: enu]
                               enu:  IfcSiteの経緯度を原点とする東・北・上（TrueNorthで回転）
                               grid: 局所座標を地図座標のオフセットとみなす（PLATEAU BIM活用マニュアル第1版の方式）
  --origin <LAT,LON[,H]>     ファイルのジオリファレンスを無視し、この点を原点とするENUで置く（Hは正標高[m]）
  --geoid <jpgeo2024|gsigeo2011|none>   正標高→楕円体高のジオイドモデル [既定: jpgeo2024]
                               jpgeo2024: JPGEO2024＋標高補正パラメータHrefconv2024（離島の標高基準面の差を含む）
  --scale-policy <auto|spec> IfcMapConversion.Scale の解釈 [既定: auto]
  --max-features <N>         1タイルの部材数の上限 [既定: 200]
  --include-spaces           IfcSpaceも出力する
  --keep-parts               集約の部品を親部材にまとめない
  --no-psets                 Pset / Qto を列に含めない
  --no-compress              量子化とmeshopt圧縮をしない（溶接はする）
```

終了コード: 成功0、入力・設定の誤り2、変換できる部材がない3、その他1。エラーメッセージは標準エラーに1行で出し、原因と対処（どのオプションを使うか）を含める。

## 5. 出力

```text
out/
  tileset.json
  tiles/<storey番号>_<四分木パス>.glb      例: 002_r13.glb
  ifc2tiles-report.json
```

### 5.1 tileset.json

- `asset.version = "1.1"`、`asset.generator = "ifc2tiles <版>"`、`asset.extras.ifc2tiles`に変換情報（§8のレポートの`conversion`部分）。
- `schema`: `storey`クラス（`name` STRING・semantic NAME、`globalId` STRING・semantic ID、`elevation` FLOAT64 [m]）と、全タイル共通の`element`クラスの完全な定義（§6）。
- `groups`: 階ごとに1つ（所属階なしは`name = "(unassigned)"`）。標高の昇順。
- 根タイル: `transform`＝根ENU→ECEF（列優先）、contentなし、`refine = "ADD"`、`geometricError`＝全体の外接箱の対角長。
- 各階: 四分木（§7）のノード。contentの`group`は所属階の番号。
- `boundingVolume`はすべて根ENU座標での軸平行`box`（半長さの最小値0.01 m）。

### 5.2 GLB

| 項目 | 内容 |
|---|---|
| 座標 | 根ENU（x東・y北・z上）を glTF の Y-up へ `[x, z, −y]` |
| primitive | 色（RGBA、各8bitに丸めた値）ごとに1つ。材料は`baseColorFactor`、`metallic 0`、`roughness 0.9`、`doubleSided`、α<0.99なら`BLEND` |
| 属性 | `POSITION`、`NORMAL`、`_FEATURE_ID_0`（FLOAT、部材の番号） |
| 既定の符号化 | 位置をタイル外接箱の最小点を基準に一様な刻み（最大辺÷65535）でUINT16に、法線をINT8正規化に量子化 → 同一の量子化頂点（位置・法線・部材番号）を溶接 → 頂点キャッシュ・頂点フェッチ最適化 → `EXT_meshopt_compression`（位置はstride 8、法線・部材番号はstride 4のATTRIBUTES、索引はTRIANGLES。フォールバック用の空バッファ付き）。復元はnodeの`matrix`。刻みを軸で変えないのは、非一様な倍率だと法線がゆがむため |
| `--no-compress` | FLOAT32の位置・法線、非圧縮（溶接と最適化はする） |
| 拡張 | `EXT_mesh_features`（`featureIds[0] = {featureCount: タイルの部材数, attribute: 0, propertyTable: 0, label: "element"}`）、`EXT_structural_metadata`（§6）。圧縮時は`KHR_mesh_quantization`と`EXT_meshopt_compression`を`extensionsRequired`にも入れる |
| バイナリ | bufferViewは8バイト境界。長さ0のbufferViewは作らない |

## 6. 部材メタデータ

### 6.1 部材（feature）の単位

- 形状を持つ`IfcProduct`の出現（occurrence）を部材とする。
- 除外: `IfcOpeningElement`、`IfcOpeningStandardCase`、`IfcVirtualElement`、`IfcAnnotation`、`IfcGrid`、`IfcStructural*`、型オブジェクト（`*Type`、`*Style`）、`IfcSpace`（`--include-spaces`で含める）。
- 部品のまとめ（既定）: 部材から`IfcRelAggregates`の親を辿り、親が空間要素（`IfcProject`、`IfcSite`、`IfcBuilding`、`IfcBuildingStorey`、`IfcSpace`、`IfcSpatialZone`、`IfcExternalSpatialElement`、`IfcFacility`とその派生、`IfcFacilityPart`とその派生）でない限り上へ進む。最上位の部材を部材の単位にし、部品のメッシュをすべてその部材の形状とする。

### 6.2 列

tileset全体で1つの`element`クラス。IDは`^[a-zA-Z_][a-zA-Z0-9_]*$`。

| ID | 型 | 内容 |
|---|---|---|
| `expressId` | UINT32、required | STEPインスタンス番号 |
| `ifcClass` | STRING、required | IFCクラス名 |
| `globalId` | STRING、semantic ID | |
| `name` | STRING、semantic NAME | |
| `description` | STRING、semantic DESCRIPTION | |
| `objectType`、`tag`、`predefinedType` | STRING | |
| `typeName` | STRING | `IfcRelDefinesByType`の型の名前 |
| `storeyName`、`storeyGlobalId` | STRING | 空間構造を上へ辿った最初の`IfcBuildingStorey` |
| `buildingName` | STRING | 同じく最初の`IfcBuilding` |
| Pset / Qto | §6.3 | 型から継承し、部材側で上書きした値 |

### 6.3 Pset / Qto の列

- ID: `sanitize(Pset名) + "__" + sanitize(プロパティ名)`。`sanitize`は英数字と`_`以外を`_`に置換する。英数字が文字数の半分未満なら`p_` + FNV-1a 32bit（`"Pset名.プロパティ名"`）の16進8桁。先頭が数字なら`_`を前置。衝突したら`_` + ハッシュを後置。元の`"Pset名.プロパティ名"`は`name`に入れる（IDと同じなら省略）。
- 型: 全部材の値を見て決める。すべて真偽値→ENUM `IfcLogical`（UINT8。FALSE=0、TRUE=1、UNKNOWN=2、NOT_SET=255、noData `NOT_SET`）。すべて整数で、32ビットに収まり`i32::MIN`を含まない→INT32（noData `i32::MIN`）。収まらなければFLOAT64。INT64は使わない（CesiumJSはINT64の値を`BigInt`で返し、JSONの数値で書いたnoDataと一致しないため、値のない部材が`undefined`にならない。BLCJのサンプルで判明）。すべて数値→FLOAT64（noData −9999.0）。それ以外→STRING（noData `""`。空文字列を値なしとする）。固定列は`expressId`と`ifcClass`だけがrequiredで、ほかはSTRINGのnoData `""`。
- 値の型: ifc-liteが文字列にした値を、値の型の名前で戻す。`IFCBOOLEAN`・`IFCLOGICAL`→真偽値、`IFCINTEGER`・`IFCCOUNTMEASURE`→整数、`LABEL`・`TEXT`・`IDENTIFIER`などの文字列型→文字列、それ以外で数値として読めるもの→数値。
- 単位: 値の型（または数量の種類）が長さ・面積・体積・質量の測度なら、プロジェクト単位からSI（m、m²、m³、kg）へ換算する。換算した列の`description`に`unit: m`などを入れる。
- 1タイルのproperty tableには、そのタイルに値が1つでもある列だけを書く。GLBのスキーマは、その列だけを持つ`element`クラス（スキーマIDはタイルごとに`ifc2tiles_<タイル名>`）。列のID・名前・型は全タイルで同じ。

### 6.4 利用者向けの約束事（READMEに書く）

- 値のない部材は、CesiumJSでは`undefined`になる。数値比較は `${x} !== undefined && ${x} > 10` または `Number(${x}) > 10` と書く（そのまま比較すると例外で描画が止まる）。
- ENUMは値名で比較する（`${Pset_WallCommon__IsExternal} === 'TRUE'`）。

## 7. タイル分割

1. 部材の根ENU座標での外接箱を求める。
2. 所属階（§6.2）ごとに分ける。
3. 各階を次の規則で四分木にする（`build(部材集合, パス)`）。
   - 部材数が`--max-features`以下、または深さ10、または全部材が1つの象限に入り上位に置く部材もない場合 → 葉（全部材をcontentに、geometricError 0）。
   - それ以外: ノードの平面の広がり`L = max(幅x, 幅y)`。外接箱の対角長が`L/4`以上の部材を対角長の降順に最大`--max-features`個までノードのcontentに置く。残りを重心のx・yがノード中心より大きいかで4象限に分けて再帰する。ノードのgeometricErrorは、残りの部材の対角長の最大値。
4. 空の子は作らない。contentのない中間ノードは許す。

## 8. ジオリファレンスと座標変換

### 8.1 解決の優先順位

1. `--origin`があれば、それを原点とするENU（TrueNorthは使わない）。
2. `IfcMapConversionScaled` / `IfcMapConversion`。CRSは`--crs`、なければTargetCRSの名前。
   ジオリファレンスのエンティティはifc-liteの抽出関数を使わず`source`で直接読む（抽出関数は`IfcRigidOperation`を扱わず、(0,0)の`IfcSite`も有効とみなし、`RefElevation`の単位を換算しないため）。
3. `IfcRigidOperation`（自前で読む）: TargetCRSが`IfcProjectedCRS`なら、長さの平行移動として2と同じ経路（回転0、倍率はプロジェクト単位→地図単位）。`IfcGeographicCRS`なら、経緯度（角度単位は`AngleUnit`、なければプロジェクトの平面角の単位）と高さを原点とするENU。
4. `IfcSite`の経緯度（(0,0)は未設定とみなす）。`RefElevation`はプロジェクト単位→m。`--site-coords`でENUか地図座標かを決める。
5. どれもなければ終了コード2で、`--origin`を案内する。

### 8.2 IfcMapConversionの数値

- 地図単位（`MapUnit`、なければプロジェクト単位）をmへ換算した値で E、N、H を持つ。
- メートル化した局所座標に掛かる実効倍率 `s = Scale × (地図単位[m]) ÷ (プロジェクト単位[m])`。`--scale-policy auto`のとき、`|log10 s| > 2`かつ逆数`(1/Scale)`で計算した値が1に近い（`|log10| < 0.01`）なら逆数を採用して警告する。それ以外で`|s − 1| > 0.01`なら警告する。
- 軸別係数（Scaled）を x、y、z に掛けてから回転する。回転角 θ = atan2(XAxisOrdinate, XAxisAbscissa)。
- 地図座標: `E = E0 + s·(Fx·x·cosθ − Fy·y·sinθ)`、`N = N0 + s·(Fx·x·sinθ + Fy·y·cosθ)`、正標高 `h = H0 + s·Fz·z`。

### 8.3 ENU経路

- 原点の楕円体高 = 正標高 + ジオイド高（範囲外は0とし警告）。
- `jpgeo2024`は、国土地理院が楕円体高と標高の変換に使う「JPGEO2024＋Hrefconv2024」の合成モデルを使う。本土ではHrefconv2024が0なのでJPGEO2024と同じ値になり、離島では標高基準面の差（那覇で+0.684 m）が加わる。
- IfcSiteのとき、モデル3Dコンテキストの`TrueNorth`（局所XY平面での真北方向 (tx, ty)）があれば、局所座標を α = 90° − atan2(ty, tx) だけ回転してからENUへ置く。

### 8.4 頂点の変換

- 地図経路: 頂点ごとに、地図座標→`jprect`で逆投影→楕円体高 = 正標高 + ジオイド高（`japan-geoid`）→GRS80楕円体で地心直交座標（ECEF、`geocentric`）。CesiumJSはWGS84として読むが、両楕円体の差は0.1 mm程度で無視できる。
- ENU経路: 原点のECEF + 基底の線形結合。
- 法線: 部材の（IFC座標での）外接箱の中心で、局所→根ENUのヤコビアン（x、y、z方向に1 m動かした差）を1回求めてグラム・シュミットで直交化し、その回転で変換する。
- 逆投影が定義域外になった場合は、終了コード2で`--crs`とジオリファレンスの確認を促す。
- 根ENU: 全頂点のECEF外接箱の中心を原点とする。頂点は根ENUでf64のまま持ち、GLBを書くときにf32（または量子化）にする。

## 9. モジュール構成

単一crate（lib＋bin）。ifc-liteに依存するのは`source`だけとする。

| モジュール | 責務 | 副作用 |
|---|---|---|
| `main.rs` | CLIの解釈、終了コード | 標準出力・標準エラー |
| `lib.rs` | `convert(input: &Path, output: &Path, opts: &Options) -> Result<Report, Error>`。各段をつなぐ。`Error`は`Input`（終了コード2）・`NoElements`（3）・`Io`（1） | ファイル書き出し（出力先の既存ファイルは消さない） |
| `source` | ifc-liteを呼び、`SourceModel`（部材の形状〔IFC世界座標・m・f64〕、属性、関係、単位、ジオリファレンスの生データ、TrueNorth、RigidOperation）を作る | なし（入力はバイト列） |
| `semantics` | 除外、部品のまとめ、所属階・建物・型の解決 | なし |
| `units` | 測度の型→SI換算係数 | なし |
| `georef` | 生データ→`Placement`（地図経路かENU経路か）と警告 | なし |
| `geodesy` | `Placement`→頂点・法線の変換器、根ENUフレーム | なし（ジオイドの格子を読むだけ） |
| `metadata` | 列の決定（ID・型）、property tableの符号化 | なし |
| `tiling` | 部材の外接箱→タイルの木 | なし |
| `glb` | 1タイル分のメッシュ集約・溶接・量子化・meshopt・GLBのバイト列 | なし |
| `tileset` | tileset.jsonの組み立て | なし |
| `report` | レポートの型 | なし |

## 10. 依存

| crate | 版 | 用途 |
|---|---|---|
| ifc-lite-core / -processing / -export | =17.3.0（公開3日を満たす最新。16.2.0から上げ、85件で退行なしを確認） | IFCの読み込み |
| jprect | =0.1.0 | 平面直角座標 |
| japan-geoid | =0.6.0（features: gsigeo2011, jpgeo2024_hrefconv2024） | ジオイド |
| geocentric | =0.1.3 | 測地↔地心 |
| meshopt | =0.6.2 | 最適化・圧縮（C++コンパイラが必要） |
| clap | =4.6.7 | CLI |
| serde_json | =1.0.151 | JSON（derive不要のため`serde`は直接使わない） |

`scripts/check-crate-age.py`で`Cargo.lock`の全crateが公開3日以上であることを確認する。

## 11. テスト方針

| 種類 | 対象 |
|---|---|
| 単体 | `metadata`（ID規則、型推定、SI換算、列の省略、noData、符号化の長さ）、`georef`（Scaleの判定、(0,0)、RefElevation、TrueNorth、RigidOperation、JGD2000・旧日本測地系）、`geodesy`（既知点、子午線収差、ジオイド範囲外）、`tiling`（上位配置、葉の条件、geometricErrorの単調性）、`glb`（構造、量子化の復元誤差）、`semantics`（部品のまとめ、除外、階の並び）、`source`（値の型、度分秒）、`tileset`（構造） |
| 結合 | 自作IFC 3件を`convert`し、出力GLB（meshoptを復号）から既知点と属性を検算（A1、A2、A4）。BURKWILがあればA5 |
| 外部 | `scripts/acceptance-handmade.sh`（pyproj）、`scripts/batch-convert.sh`（validator）、CesiumJS目視（A6） |

## 12. 未確定事項

なし。着手時の未確定事項だったifc-liteの版は17.3.0に決めた。16.2.0と比べ、85件の属性・関係・ジオリファレンスの抽出結果は同じで、形状はIfcOpenShellとの体積の差が1%を超える部材が203件→193件に減った（外接箱の1 cm超の差は4件のまま）。
