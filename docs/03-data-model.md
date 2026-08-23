# 03. データモデル

## 1. 基本方針

AutoCAD の Database モデル（シンボルテーブル + 辞書 + エンティティ、全エンティティは BlockTableRecord 所有）を**骨格として踏襲**する。理由は互換性のためだけではない。あのモデルは「ブロック定義＝入れ子図面」「モデル空間とレイアウトの統一」「拡張辞書による業種特化」という 3 つを最小の概念数で成立させており、40 年の実運用に耐えた構造だからである。

その上で、DXF/DWG の制約に由来する部分（グループコード、ハンドルの 16 進文字列、固定的な型システム）は引き継がない。

---

## 2. コアモデル

```rust
/// 図面 1 つ = 1 Database
pub struct Database {
    pub header: HeaderVars,              // 単位, 精度, 現在レイヤ, 挿入基点, ...
    objects: SlotMap<ObjectId, Object>,  // 全オブジェクトの実体
    tables: SymbolTables,                // 名前 → ObjectId の索引
    pub named_dict: DictionaryId,        // 名前付きオブジェクト辞書 (NOD)
    spatial: SpatialIndex,               // R 木（モデル空間）
    unknown: Vec<PreservedBlob>,         // 読めなかった要素の原形保持
}

pub struct SymbolTables {
    pub layers:      Table<Layer>,
    pub linetypes:   Table<LineType>,
    pub text_styles: Table<TextStyle>,
    pub dim_styles:  Table<DimStyle>,
    pub blocks:      Table<BlockRecord>,  // ModelSpace / PaperSpace もここ
    pub views:       Table<View>,
    pub ucs:         Table<Ucs>,
    pub viewports:   Table<ViewportTableRecord>,
    pub app_ids:     Table<AppId>,
    pub materials:   Table<Material>,     // 3D 用に追加
    pub levels:      Table<Level>,        // 建築の「階」。国内実務で必須
    pub grids:       Table<GridAxis>,     // 通り芯。同上
}
```

`Level`（階）と `GridAxis`（通り芯）を**シンボルテーブルの第一級市民**に昇格させているのが AutoCAD との明確な差分。建築・設備では階と通り芯があらゆる図形の位置決めの基準になるため、拡張属性で後付けすると全ドメインが同じものを別々に再発明する。

### オブジェクト

```rust
pub struct Object {
    pub id: ObjectId,
    pub owner: ObjectId,              // 所有者（BlockRecord / Dictionary）
    pub kind: ObjectKind,
    pub xdata: XDataMap,              // AppId → 型付きレコード
    pub ext_dict: Option<DictionaryId>,
    pub reactors: SmallVec<[ObjectId; 2]>,  // 依存通知先
}

pub enum ObjectKind {
    Entity(Entity),
    Dictionary(Dictionary),
    XRecord(XRecord),
    Layout(Layout),
    Group(Group),
    /// ドメイン層が定義するカスタムオブジェクト（MEP のルート等）
    Custom { type_id: TypeId, data: CustomData },
    /// 読めなかったが保存時に書き戻す
    Preserved(PreservedBlob),
}
```

### エンティティ

```rust
pub struct Entity {
    pub layer: LayerId,
    pub geom: Geometry,
    pub style: GraphicStyle,   // 色/線種/線幅/透過/マテリアル（ByLayer/ByBlock 含む）
    pub visible: bool,
    pub space: SpaceRef,       // 所属する BlockRecord
}

pub enum Geometry {
    Point(Point3),
    Line(Line3),
    Circle(Circle3),
    Arc(Arc3),
    Ellipse(EllipticalArc3),
    Polyline(Polyline),        // 2D/3D、バルジ、幅、閉じ
    Spline(Nurbs3),
    Hatch(Hatch),              // 境界ループ + パターン/グラデーション
    Text(TextEntity),          // 縦書きフラグ、和文組版パラメータを含む
    MText(MTextEntity),
    Dimension(Dimension),      // 長さ/角度/半径/直径/座標/並列/累進
    Leader(Leader),
    BlockRef(BlockReference),  // 挿入・属性値・パラメトリック値
    Region(Region2),
    Solid3d(SolidHandle),      // od-geom3d のハンドル（遅延評価）
    Mesh(MeshHandle),
    Image(RasterRef),
    PointCloud(PointCloudRef),
    Wipeout(Wipeout),
    Viewport(ViewportEntity),  // ペーパー空間からモデル空間への窓
}
```

### 幾何の数値表現

- 座標は **f64、単位はミリメートル固定**（表示単位は `HeaderVars` で変換）
- **ワールド原点からの絶対座標を持たない**設計にする。測地座標のような巨大値は `Database.geo_origin` にオフセットとして持ち、エンティティは相対座標。f64 の有効桁を実用域に集中させる（N-05 の達成手段）
- トレランスは 3 段階: `POINT_EPS = 1e-7mm`（同一点判定）、`ANGLE_EPS = 1e-9rad`、`AREA_EPS`。**関数ごとに違うイプシロンを書かない**。誤差方針の不統一は CAD 最大のバグ源

---

## 3. 拡張機構（ドメイン層の土台）

MEP を含む全ドメインはコアを改造せず、以下の 3 つで自己を表現する。

### 3.1 XData（スキーマ付き拡張属性）

AutoCAD の XDATA/XRecord に相当するが、**スキーマを持つ**点が異なる。属性の意味が失われる（Tfas 実運用の課題）ことを防ぐため。

```rust
pub struct XDataMap(HashMap<AppId, TypedRecord>);

/// アプリごとにスキーマを登録する
pub struct XDataSchema {
    pub app_id: AppId,             // "org.opendraft.mep"
    pub version: SemVer,
    pub fields: Vec<FieldDef>,     // 名前, 型, 単位, 必須, 既定値, 説明
    pub ifc_mapping: Option<PropertySetMapping>,  // IFC Pset への写像
}
```

`ifc_mapping` を**スキーマ定義の時点で要求する**ことで、IFC 書き出し時のマッピング漏れを構造的に防ぐ（→ F-206 往復テスト）。

### 3.2 カスタムエンティティ

ドメインは `CustomEntity` トレイトを実装して、独自の型を登録する。

```rust
pub trait CustomEntity: 'static {
    fn type_id(&self) -> TypeId;
    /// 表示用の派生ジオメトリを生成（単線 / 複線 / 3D）
    fn derive(&self, ctx: &DeriveCtx, view: ViewMode) -> Vec<Geometry>;
    /// このエンティティが依存する他オブジェクト
    fn dependencies(&self) -> Vec<ObjectId>;
    /// 依存先が変わったときの再評価
    fn revalidate(&mut self, ctx: &DeriveCtx) -> Result<()>;
    /// 未対応環境向けのフォールバック（プロキシ図形）
    fn proxy_graphics(&self) -> Vec<Geometry>;
}
```

`proxy_graphics()` は必須。**このドメインプラグインを持たない相手でも図面が正しく見える**ことを保証するため。AutoCAD のプロキシグラフィックスと同じ発想であり、協力会社間の環境差（Tfas 現場の実害）への回答でもある。

### 3.3 派生グラフ（Derivation Graph）

「ルートを引くと 3D になる」「平面を直すと断面が直る」を成立させる仕組み。

```
BaseObject（ソース: MEP ルート、通り芯、断面線）
   │ derive()
   ▼
DerivedGeometry（キャッシュ。ダーティフラグ付き）
   │
   ▼ 描画・干渉判定・集計はここを読む
```

- 依存は DAG として保持し、循環を検出したらエラーにする
- 変更は**トポロジカル順に増分伝播**。全再計算しない
- 派生結果はファイルに保存する（開いた瞬間に見えるため）が、**真実はソース側**。読み込み後にバージョン差があれば再導出する

---

## 4. コマンドとトランザクション

```rust
pub struct Transaction<'db> {
    db: &'db mut Database,
    journal: Vec<Change>,      // 逆操作を含む変更記録
    author: ActorId,
    started_at: Timestamp,
}

pub enum Change {
    Create { id: ObjectId, snapshot: ObjectData },
    Modify { id: ObjectId, before: Patch, after: Patch },
    Delete { id: ObjectId, snapshot: ObjectData },
    TableEdit { table: TableKind, before: ..., after: ... },
}
```

- **コミット時に検証を走らせる**: 参照整合性、派生グラフの循環、拘束の充足、ドメイン規則（例: 接続されていない配管端部の検出）
- 検証失敗はトランザクションごと巻き戻す。**中途半端な状態をドキュメントに残さない**
- Undo は `Change` の逆適用。巨大操作（一括変換など）は N 件を超えたらスナップショットに切り替える

---

## 5. 永続化: `.odc` フォーマット

### 5.1 コンテナ構造

`.odc` は ZIP コンテナ（非圧縮ストア + 個別 zstd 圧縮）。理由: 部分読み出しが可能で、既存ツールで中身を検査でき、差分同期しやすい。

```
document.odc/
├── manifest.json          スキーマ版, 生成アプリ, 必要な拡張の一覧
├── header.cbor            HeaderVars
├── tables/
│   ├── layers.cbor
│   ├── blocks.cbor
│   └── ...
├── objects/
│   ├── 0000.chunk         オブジェクトを空間近傍でまとめたチャンク（CBOR + zstd）
│   ├── 0001.chunk
│   └── ...
├── index/
│   ├── rtree.bin          空間インデックス（先読み対象）
│   └── byid.bin           ObjectId → (chunk, offset)
├── derived/               派生ジオメトリのキャッシュ（欠けても再生成可）
├── history/
│   ├── log-0001.jsonl     操作ログ（コマンド列）
│   └── snapshots/
├── schemas/               この図面が使う XData スキーマの実体（自己記述）
├── resources/             埋め込みフォント, ラスタ, 部品, サムネイル
└── preserved/             読み込み時に理解できなかったデータの原形
```

> **実装状況（2026-08 現在）**: `crates/od-io-odc` が上記のうち `manifest.json` /
> `header.cbor` / `tables/` / `objects/` / `schemas/` / `preserved/` を読み書きする。
> `index/`・`derived/`・`history/` はまだ生成しない — 空間インデックスも履歴の
> 永続化もまだ存在しないため。いずれも `optional-ignore` / `optional-preserve`
> として manifest に宣言済みなので、後から追加しても既存ファイルは壊れない。
> オブジェクトのチャンク分割は現状「挿入順に 1000 件ずつ」。設計にある空間近傍で
> のグルーピングは R 木の実装後に行う（先に任意のグルーピングを固定すると、
> 読み手がそれを支え続けることになる）。

### 5.2 互換性の契約（N-04 の実装）

`manifest.json` に**必要拡張の一覧**を持ち、各要素に「未知でも安全に無視できるか」を宣言する。

| 分類 | 挙動 |
|------|------|
| `required` | 対応していないバージョンは**読み込みを拒否**（壊した保存をさせない） |
| `optional-preserve` | 理解できないが `preserved/` に原形保持し、再保存時に書き戻す |
| `optional-ignore` | キャッシュ類。無視してよい |

これにより「新しいバージョンで保存された図面を古いバージョンが開いて、知らない情報を消して保存してしまう」という**最も破壊的な事故**を構造的に防ぐ。既存製品でバージョン差が実害になっている点への直接の対策。

### 5.3 スキーマの自己記述

図面は自分が使う XData スキーマを `schemas/` に同梱する。これにより、そのドメインプラグインを持たない環境でも**属性の意味（名前・単位・型）が読める**。「変換で属性が欠落する」問題への構造的対処。

---

## 6. 履歴とバージョン管理

操作ログ（`history/*.jsonl`）はコマンドの列であり、以下を可能にする。

- **クラッシュ復旧**: 最後のスナップショット + ログ再生
- **監査**: 誰がいつ何を変えたか
- **名前付きバージョン**: 「実施設計提出時」のようなタグ
- **差分表示**: 2 バージョン間で「追加/変更/削除されたエンティティ」を図面上でハイライト
- **ブランチとマージ**: 別案の検討

ログはテキスト（JSON Lines）にする。Git 等での管理・grep・外部ツールでの解析が効く。バイナリ最適化は後で足せるが、可読性は後から足せない。

---

## 7. 協調編集

### 7.1 方針

**オフラインファースト**（F-401）。サーバは任意。ローカルでは常に単独で完結して動く。

### 7.2 同期モデル

CAD の同時編集は、テキストの CRDT をそのまま持ち込むと破綻する（幾何の整合性・拘束・接続関係が絡むため）。以下の階層で扱う。

| 層 | 手法 |
|----|------|
| オブジェクトの追加・削除 | Add-Wins Observed-Remove Set。ID は `(ActorId, Counter)` で衝突なく生成 |
| オブジェクトの属性 | フィールド単位の LWW（Last-Writer-Wins）+ ハイブリッド論理時計 |
| 幾何の同時編集 | **意図保存**: 「頂点を絶対座標 P へ」ではなく「頂点を Δ 移動」として送り、可換にする |
| 接続関係・拘束 | 可換にできない。**楽観ロック**: 編集開始時にオブジェクト群のソフトロックを取得し、他者には「編集中」を表示 |
| 派生ジオメトリ | 同期しない。各クライアントがソースから再導出する |

### 7.3 コンフリクト

自動マージできない場合は**捨てない**。両方を保持して、ユーザーに提示する（差分ビューで選択）。CAD では「自動で片方が消える」ほうが遥かに危険。

### 7.4 現実的な運用単位

Onshape でも同時編集者は 4 名以下が推奨されている。**同時編集を売りにしすぎない**。主眼は「排他ロックなしで待たされない」ことと「今誰がどこを触っているか見える」ことに置く。図面分割（シート・XREF）による分業のほうが実務に合う場面が多い。

---

## 8. 単位・座標系

```rust
pub struct HeaderVars {
    pub insunits: Units,          // 図面単位（mm 既定）
    pub linear_precision: u8,
    pub angle_base: f64,
    pub angle_dir: AngleDir,
    pub current_ucs: UcsId,
    pub geo_origin: Option<GeoRef>,  // 測地座標との対応（EPSG コード + 原点）
    pub project_north: f64,          // 真北と図面上の北のずれ
}
```

- **UCS（ユーザー座標系）はエンティティに保存しない**。作図時の入力変換にのみ使い、格納は常にワールド座標。AutoCAD の OCS（オブジェクト座標系）に相当する概念は、DXF 入出力の変換層に閉じ込める
- 測地参照を持つことで GIS・土木・BIM 座標合わせに対応する
