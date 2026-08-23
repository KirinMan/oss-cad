# 00. 調査サマリ

調査日: 2026-08-15

---

## 1. 参考プロダクト

### 1.1 AutoCAD（Autodesk）

汎用 CAD のデファクト。設計上で参考にすべき本質は「機能一覧」ではなく **データモデルとカスタマイズ機構**。

- **データベース中心設計**: 図面 = 1 つの `Database` オブジェクト。ヘッダ変数群、9 種のシンボルテーブル（Layer / Block / LType / TextStyle / DimStyle / View / UCS / Viewport / AppId）、名前付きオブジェクト辞書（NOD）、エンティティから構成される。
- **全エンティティは BlockTableRecord に所有される**。モデル空間・ペーパー空間も BlockTableRecord の一種にすぎない。この統一が「ブロック定義 = 図面の入れ子」を成立させている。
- **拡張辞書 / XRecord / XDATA**: 任意アプリが任意データをエンティティにぶら下げられる。サードパーティ製の業種特化 CAD（設備 CAD 含む）はここに属性を載せて成立している。
- **ObjectARX**: C++ で AutoCAD プロセス内に DLL をロードするネイティブ拡張 API。加えて AutoLISP / VBA / .NET。
- 参考: [The Database Object (.NET)](https://help.autodesk.com/cloudhelp/2015/ENU/AutoCAD-NET/files/GUID-7313ECA1-4875-4946-82E3-C06A4074F807.htm), [AcDb Classes](https://help.autodesk.com/cloudhelp/2018/ENU/OARX-DevGuide/files/GUID-DC57940B-B5E3-4D9E-83EC-0CA28ABACCFC.htm), [ObjectARX (Wikipedia)](https://en.wikipedia.org/wiki/ObjectARX), [ezdxf Data Model](https://ezdxf.readthedocs.io/en/stable/dxfinternals/datamodel.html)

**設計への含意**: 「エンティティ + 拡張属性 + 辞書」という汎用土台を先に作れば、MEP は上に載る。逆順にすると破綻する。

### 1.2 CADWe'll Tfas（ダイテック）

国産の建築設備専用 CAD。空調・衛生・電気の 3 領域を 1 本で扱うのが最大の特徴で、空調衛生・電気設備の現場で広く採用されている。

調査で確認できた機能特性:

- **設備専用ライブラリ**: 業界標準シンボル・部品を配置。最新の Tfas 15 では部品数を大幅拡張。
- **自動 3D 化**: ダクト・配管の**ルート（芯線）を引くと自動で立体化**され、そのまま干渉確認につながる。2D 作図の操作感を保ったまま 3D モデルが育つ。
- **機器ポートへの自動接続**: ダクト・配管が機器の接続口に自動でつながる。部材移動時の追従も。
- **電気設備**: 個別・範囲・円弧配置などの複数配置手法、自動配線・接続シミュレーション、3D プレビュー。
- **風量からのダンパー等の自動サイズ決定**。
- **シート機能**: 建築図と設備図を分離して保持し、建築側変更時の手戻りを最小化（＝外部参照の一種）。
- **外部リンク**: 複数図面にまたがる変更を集約反映。
- **高速表示**: 大規模モデルでも快適に動く 3D 表示性能が競合（Rebro / CADEWA）比の強み。
- **相互運用**: DWG/DXF、Revit 連携（Tfas 11 以降で属性付き設備 BIM モデルの相互編集）、Tfas 15 で IFC 入出力を強化。クラウド（Box）連携。
- 参考: [Tfas 使い方ガイド](https://www.paradygm.co.jp/portal/cad-bim/articles/tfas-guide-mep), [Tfas 基本操作と活用方法](https://www.at-cad.com/column/cad-cadwelltfas/), [Tfas 15 リリースと IFC 連携](https://zenn.dev/arupakaimokenpi/articles/202605180354-4bd752), [ダイテック公式](https://www.daitec.jp/download/index_manual.html)

**現場で報告されている課題**（＝ OSS 側の勝ち筋）:

- 協力会社間で**バージョンが揃わない**、アップグレード費用が下請けの負担になる
- **IFC 書き出し品質が担当者のスキルに依存**する
- 形式変換で**属性が欠落**する
- 買い切りライセンスが数十万円台〜で、教育・部品整備を含めた総額が重い

### 1.3 競合（国産設備 CAD）

Rebro（NYK システムズ）、CADEWA（四電工）と合わせて国産設備 CAD 三強。Rebro は平面と断面が常に連動し、干渉チェックと属性管理に強い。CADEWA も同領域。
参考: [Rebro と Tfas の違い](https://bimcim-kenkyujo.com/bim-cim/rebro/tfas-chigai/), [建築設備 CAD 主要 5 製品](https://www.abkss.jp/blog/143)

---

## 2. 既存 OSS の到達点と限界

| プロジェクト | ライセンス | 到達点 | 限界 |
|---|---|---|---|
| [LibreCAD 2.x](https://github.com/LibreCAD/LibreCAD) | GPL-2.0 | 実用的な 2D 作図。DXF/DWG 読み、DXF/DWG/PDF/SVG 書き。安定版 2.2.1.4（2026-03） | 2D のみ。拘束なし。アーキテクチャが GUI と密結合 |
| [LibreCAD_3](https://github.com/LibreCAD/LibreCAD_3) | GPL-2.0 | GUI 非依存のモジュラーコア、Lua スクリプティングという**正しい設計方針** | 長年プレビュー段階のまま。実運用に至っていない |
| [QCAD](https://github.com/qcad/qcad) | GPL-3.0（DWG プラグインは proprietary） | 完成度の高い 2D CAD。ECMAScript による広範なスクリプト拡張 | DWG はクローズド。3D なし |
| [FreeCAD](https://github.com/FreeCAD/FreeCAD) | LGPL-2.1 | OCCT ベースの本格 3D パラメトリック。**PlaneGCS**（2D 拘束ソルバ）、BIM ワークベンチ | 2D 製図の実務品質が AutoCAD 水準に届かない。UI 学習コスト |
| [SolveSpace](https://solvespace.com/) | GPL-3.0 | 自前の拘束ソルバ内蔵、軽量 | 大規模図面・製図機能は非対象 |
| [BlenderBIM / IfcOpenShell](https://ifcopenshell.org/) | LGPL-3.0 | IFC の読み書き・ジオメトリ生成の事実上の標準実装 | CAD 作図 UI ではない |

**結論**: 「2D 製図の実務品質」と「3D + 拘束 + 業種ドメイン」を**両立**した OSS は存在しない。ここが空白地帯。

### ジオメトリカーネル / ソルバの選択肢

- **OCCT (Open CASCADE Technology)**: LGPL-2.1 + 例外。数少ない実用 OSS B-rep カーネル。FreeCAD / IfcOpenShell が依存。C++。巨大で癖がある。
- **PlaneGCS**: FreeCAD 由来の 2D 幾何拘束ソルバ（LGPL）。DogLeg / Levenberg-Marquardt / BFGS / SQP による数値最適化。ヤコビアンの階数と残差から**過拘束・冗長・矛盾**を検出。[WASM ポート](https://github.com/Salusoft89/planegcs)も存在。
- **truck**（Rust, Apache-2.0/MIT）: NURBS B-rep を Rust で再実装。実用度は OCCT に及ばないが、依存の軽さは魅力。
- **Fornjot**（Rust）: **開発終了**。ゼロから B-rep を書く難度の証拠として参照する。
- 参考: [PlaneGCS 解説](https://deepwiki.com/FreeCAD/FreeCAD/3.1.2-constraint-system-and-gcs-solver), [truck](https://github.com/ricosjp/truck), [Fornjot](https://github.com/hannobraun/fornjot)

**結論**: B-rep カーネルは自作しない。OCCT を境界を切って使う。2D 拘束は PlaneGCS のアルゴリズムを Rust で再実装するのが現実解。

---

## 3. ファイル形式

### 3.1 DWG

- **[GNU LibreDWG](https://www.gnu.org/software/libredwg/) 0.13.4（2026-03-19）**: 読み取りは r1.2〜r2018 を約 99% カバー。**書き込みは r1.1〜r2000 まで**、r2004 以降は開発中で r2007 は未実装。ASCII DXF は r11〜r2021 書き込み可（バイナリは未対応）。ライセンスは **GPL-3.0**。
- **[ODA Drawings SDK](https://www.opendesign.com/products/drawings)**: 商用。2026-01-01 から Controlling レベルが新規締切、Standard $25,000/年（バイナリのみ）、Premium $50,000/年（ソース付き）。年額サブスクで、解約すると再頒布権を失う。

**含意**: OSS プロジェクトのコアが ODA に依存することはできない。DWG は「LibreDWG による読み込み中心 + DWG 書き出しは r2000 互換に限定」から始め、実務の主戦場は DXF に置く。

### 3.2 DXF

仕様が公開されており、[ezdxf](https://ezdxf.mozman.at/)（Python, MIT, 1.4.4 / 2026-05）が R12〜R2018 の読み書きをカバー。C++ には [libdxfrw](https://github.com/codelibs/libdxfrw)。**参照実装として ezdxf の DXF 内部ドキュメントが最良の一次資料**。

### 3.3 IFC

buildingSMART の BIM 標準。IfcOpenShell（LGPL-3.0）が読み書き＋ジオメトリ生成を提供し、内部で OCCT を用いてパラメトリック定義を BRep に展開する。MEP は `IfcDuctSegment` / `IfcPipeSegment` / `IfcFlowFitting` / `IfcDistributionPort` などが定義済み。

### 3.4 SXF（国内・公共案件で必須）

- 国土交通省主導の SCADEC が 1999 年に策定した CAD データ交換標準。仕様は公開。
- 物理形式が 2 つ: **P21**（ISO 10303/AP202 準拠、国際標準対応）と **SFC**（国内独自、P21 の 1/3〜1/8 のサイズ）。電子納品の実運用では SFC が主流。
- C 言語の共通ライブラリが存在する。
- 参考: [SXF (Wikipedia)](https://ja.wikipedia.org/wiki/SXF), [国総研資料 第403号](https://www.nilim.go.jp/lab/bcg/siryou/tnn/tnn0403pdf/ks0403009.pdf), [SFC と P21 の違い](https://ijcad.jp/column/sxf-extension-attention)

**含意**: 国内で採用されるには SXF 入出力が事実上の必須条件。かつ仕様が公開されているため OSS 実装が可能。**ここは商用 CAD に対する明確な差別化ポイントではないが、無いと土俵に上がれない**。

---

## 4. レンダリング / 実行環境

大規模図面の描画で効くのは以下（Web CAD ビューアの実装知見より）:

- CPU→GPU のドローコールがボトルネック。**多数の小ジオメトリを 1 つの大きな頂点バッファにバッチ**し、メタデータ配列でエンティティ位置を追跡して 1 ドローコールに畳む。
- 大規模アセンブリには **Octree / BVH の階層空間インデックス**を CPU 側に持ち、フラスタムを上位ノードとテストして可視な枝だけを辿る。
- **WebGPU** はコンピュートシェーダを持ち、カリングや線分展開を GPU 側に寄せられる。WASM に C++/Rust エンジンを載せてブラウザ内で DWG/DXF を直接パースする実装例も既にある。
- 参考: [Building a High-Performance Web-Based CAD Viewer](https://medium.com/@mlightcad/building-a-high-performance-web-based-cad-viewer-with-batched-geometry-system-a8859bbb0a3a), [WebGL/Three.js CAD Rendering Optimization](https://rapidmade.com/webgl-three-js-cad-rendering-optimization/)

---

## 5. 調査から導いた設計上の決定事項

| # | 決定 | 根拠 |
|---|------|------|
| D-1 | 汎用 CAD コア（AutoCAD 相当）と MEP ドメイン（Tfas 相当）を**層として分離**し、後者は前者の拡張属性機構の上に載せる | AutoCAD の拡張辞書モデルで実際に業種 CAD が成立している |
| D-2 | B-rep カーネルは**自作しない**。OCCT を別クレート・別プロセス境界に隔離 | Fornjot の開発終了。OCCT は LGPL で商用利用可 |
| D-3 | DWG は読み中心（LibreDWG, GPL）で、**プラグインとしてプロセス分離**。コアのライセンスを汚染させない | LibreDWG が GPL-3.0、ODA が年 $25k〜 |
| D-4 | 主戦場は DXF + 独自形式 + IFC。国内向けに SXF(SFC/P21) を第一級サポート | 電子納品要件。仕様公開済み |
| D-5 | MEP は「**芯線ルート + 断面仕様 + 自動継手生成**」モデル。2D 作図が即 3D になる | Tfas の自動 3D 化がユーザー体験の核 |
| D-6 | 2D 拘束ソルバは PlaneGCS のアルゴリズム（数値最適化 + ヤコビアン階数解析）を踏襲 | 実績のある唯一の OSS 系譜 |
| D-7 | 属性を落とさない相互運用を**契約としてテスト**する（往復変換テスト） | Tfas 実運用の最大の不満が「変換で属性が欠落」 |
| D-8 | バージョン差でファイルが開けない事態を作らない（前方・後方互換を仕様化） | 協力会社間のバージョン不一致が現場の実害 |
