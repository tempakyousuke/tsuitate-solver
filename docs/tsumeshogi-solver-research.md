# 詰将棋ソルバー調査レポート

## 1. 合駒（合駒/aigoma）の解決方法

### 1.1 合駒問題の背景

詰将棋において、飛車・角行・香車などの長距離利き駒が離れた位置から王手をかけた場合、玉方はチェッカーと玉の間に駒を打つ（合駒）ことで王手を回避できる。これがソルバーにとって組合せ的な課題を生む：

- 最大7種類の駒を合駒に使用可能（歩・桂・香・銀・金・角・飛）
- 合駒マスが複数ある場合、選択肢がさらに増加
- 一部の合駒は「無駄合い」（結果を変えない合駒）であり、詰将棋のルール上省略される

### 1.2 無駄合い（futile interposition）の判定

#### 柿木アルゴリズム（最も広く参照される手法）

柿木義一氏（柿木将棋の作者）が考案し、1990年の書籍『コンピュータ将棋―あなたも挑戦してみませんか』（サイエンス社）で発表された。

**基本的な定義**: 攻め方が合駒を取り返して再び王手をかけ、**取った合駒を使わずに** N手以内で詰ませられるなら、その合駒は無駄合いである。

具体的には：
1. 合駒がない状態でN手詰みであることが既知
2. 合駒の後、攻め方がそれを取り返して再王手
3. 結果の局面が、取った駒を使わずにN手以内の詰みであれば、無駄合い

このアルゴリズムは**再帰的**：取り返し後に玉方がさらに合駒した場合、それも無駄合いかテストが必要。

**参考**:
- [将棋プログラムK1.5の無駄合判定アルゴリズムについて | やねうら王](https://yaneuraou.yaneu.com/2023/02/03/algorithm-for-determining-the-wasted-pin-of-the-shogi-program-k1-5/)
- [詰将棋の無駄合の定義について(長文) | やねうら王](https://yaneuraou.yaneu.com/2023/01/28/the-definition-of-shogi-problems-wasted-pin/)

#### 無駄合いの3つの定義

やねうら王のブログ記事による整理：

1. **基本定義**: 攻め駒が合駒を取り返し、その取った駒を使わずに詰みが成立する場合
2. **改良定義**: 合駒→取り返しの手順を除外しても同手数以内で詰みが成立する場合
3. **公式定義（全日本詰将棋連盟、1999年）**: 「駒を取って以降の手順内容に変化がない合駒」

**エッジケース**:
- 特定の取り方（不成など）でのみ詰みを回避できる場合の分類が曖昧
- 合駒により手数が伸びるが、別の短手順が発見される場合
- 2マス以上離れた王手にのみ適用（両王手は除外）

#### 実装アプローチ

| アプローチ | 説明 | 採用例 |
|-----------|------|--------|
| **A: 探索中の事前フィルタリング**（柿木方式） | 応手生成時に各合駒候補の無駄合いをテスト。取り返し→再王手→取った駒未使用で詰みかを再帰的に確認し、無駄合いを探索木から枝刈り | tsuitate-solver（深さ3制限）|
| **B: 探索後のバリデーション** | 全合駒候補を探索に含め、証明木を得た後に無駄合いを検証。無駄合いが見つかれば該当分岐を破棄 | sugyan/tsumeshogi-solver |
| **C: 無駄合い判定なし** | 無駄合いを含む最短解を返す | KomoringHeights v1.0.0 |

**参考**:
- [Rustでつくる詰将棋Solver - すぎゃーんメモ](https://memo.sugyan.com/entry/2021/11/11/005132)
- [安定性が向上した詰将棋エンジン KomoringHeights v1.0.0](https://komorinfo.com/blog/komoring-heights-v1/)

### 1.3 df-pnにおける合駒の扱い

#### AND/ORツリー構造での影響

df-pnフレームワークにおいて：
- **ORノード**（攻め方の手番）: pn = min(子のpn), dn = Σ(子のdn)
- **ANDノード**（玉方の手番）: pn = Σ(子のpn), dn = min(子のdn)

合駒はANDノードで分岐を生む。長距離王手に対する玉方の応手：
- 玉の移動（最大8方向）
- 王手駒の取り
- 合駒（最大7駒種 × 合駒マス数）
- 移動合い

→ pn = Σ(子のpn)なので、ANDノードのpnが非常に大きくなりうる。

#### 二重カウント問題

df-pnでは、合駒が**収束問題**（transposition convergence）の主要な原因となる。異なる駒種の合駒が取り返された後、同一局面に合流するためである。

**KomoringHeights v0.4.1の解決策**: 収束しやすい手の種類（合駒、持ち駒での離し王手、不成王手）を識別し、これらの手についてANDノードでの**Σの代わりにmax演算**を適用。テストで**探索局面数88.1%削減**を達成。

**参考**: [詰将棋探索における簡易的な二重カウント対策 | コウモリのちょーおんぱ](https://komorinfo.com/blog/proof-number-double-count/)

#### 証明駒・反証駒（proof pieces / disproof pieces）

攻め方の持ち駒が多いほど詰みやすい、という支配関係に基づく枝刈り手法。

合駒処理における特殊ロジック：離し王手で合駒可能な場合：
```
proof_pieces(node) = ∪(子のproof_pieces) ∪ A_n
```
`A_n`は「攻め方が独占している駒種」（玉方が持っていない駒種）。玉方がその駒で合駒できないことが詰みの条件の一部である場合に必要。

**参考**: [詰将棋探索における証明駒／反証駒の活用方法 | コウモリのちょーおんぱ](https://komorinfo.com/blog/proof-piece-and-disproof-piece/)

### 1.4 合駒に対する最適化手法

#### 遅延合駒展開（KomoringHeights v0.5.0）

全駒種を同時に展開する代わりに、安い駒から順にテスト：**歩→桂→香→銀→角→飛→金**

理由：歩の合駒で攻めが破綻する（不詰が証明される）なら、桂・香・銀・角・飛・金のテストは不要。

**効果**: テスト局面で探索局面数が ~22,000,000 → ~3,600 に劇的改善。

**参考**: [KomoringHeights v0.5.0を公開した | コウモリのちょーおんぱ](https://komorinfo.com/blog/komoring-heights-v050/)

#### 無駄合いキャッシュ（マスベース）

マスSへの駒Xの打ち込みが無駄合いと判定された場合、同じマスSへの駒Yの打ち込みも無駄合い（打ち駒に限る）。tsuitate-solverの`futile_drop_squares: HashSet<Square>`で実装。

#### 合駒マス数制限

探索深さに応じて考慮する合駒マス数を制限：
```rust
let max_interposition: u8 = if remaining_depth >= 7 { 4 } else { 2 };
```

#### N-2再探索

N手詰みを発見後、N-2手以内の解を再探索。短い解が見つかれば、無駄合いの手順を暗黙的に除外できる。df-pnの主探索を複雑にせずに解の質を改善。

**参考**: [詰将棋アルゴリズムdf-pnのすべて | やねうら王](https://yaneuraou.yaneu.com/2024/05/08/all-about-df-pn/)

#### 観測分岐マージ（tsuitate-solver固有）

衝立詰将棋では、異なる合駒種が同じ観測タイプ（NoCapture等）を生むが先手持ち駒状態が異なる場合がある。`merge_observation_branches`で3分岐以上を1分岐にマージし、指数爆発（7^k → 1）を防止。

### 1.5 関連学術論文

1. **Nagai & Imai (2002)**: "df-pnアルゴリズムの詰将棋を解くプログラムへの応用" - [IPSJ](https://www.ipsj.or.jp/award/H14/14-01.html)
2. **Kishimoto & Muller (2002)**: "The PN*-search algorithm: Application to tsume-shogi" - [ScienceDirect](https://www.sciencedirect.com/science/article/pii/S0004370201000844)
3. **Kaneko (2004)**: "詰将棋におけるdf-pn探索のための展開後の証明数と反証数" - [PDF](https://www.graco.c.u-tokyo.ac.jp/~kaneko/papers/gpw04.pdf)
4. **JSAI講座 (2011)**: "詰将棋を解くための探索技術について" - [J-STAGE](https://www.jstage.jst.go.jp/article/jjsai/26/4/26_392/_pdf)

---

## 2. 詰将棋ソルバー Gitリポジトリ一覧

### 2.1 専用詰将棋ソルバー

#### KomoringHeights
- **URL**: https://github.com/komori-n/KomoringHeights
- **言語**: C++ (94%)
- **アルゴリズム**: df-pn+（強化版df-pn）
- **特徴**:
  - YaneuraOuのコードベース上に構築
  - 優等/劣等局面枝刈り（証明駒/反証駒）
  - 千日手（連続王手の千日手）の厳密判定
  - 局面合流検出・二重カウント回避
  - USIプロトコル対応
  - ミクロコスモス（1525手）クラスの超長手数問題に対応
- **最終更新**: 2024年6月、38スター
- **備考**: 最も高機能なオープンソース詰将棋ソルバー。著者ブログ: [komorinfo.com](https://komorinfo.com/blog/komoring-heights/)

#### sugyan/tsumeshogi-solver
- **URL**: https://github.com/sugyan/tsumeshogi-solver
- **言語**: Rust (100%)
- **アルゴリズム**: df-pn
- **特徴**:
  - CLIツール（v0.6.0）
  - SFEN/CSA/KIF入力対応、USI/CSA/KIFU出力対応
  - タイムアウト設定、ベンチマーク機能
  - 高速指し手生成ライブラリ"yasai"使用
- **最終更新**: 2022年9月、21スター
- **参考ブログ**: [Rustでつくる詰将棋Solver](https://memo.sugyan.com/entry/2021/11/11/005132)

#### sugyan/tsumeshogi-solver-wasm
- **URL**: https://github.com/sugyan/tsumeshogi-solver-wasm
- **言語**: JavaScript (47.8%), Rust (30.9%), HTML (13.6%)
- **アルゴリズム**: df-pn（Rust→WASM）
- **特徴**: ブラウザ上で動作、サーバ不要
- **最終更新**: 2022年5月、3スター

#### hkijin/shtsume
- **URL**: https://github.com/hkijin/shtsume
- **言語**: C (99.7%)
- **アルゴリズム**: 証明数探索（df-pn変種）
- **特徴**:
  - USIプロトコル対応
  - 低メモリ・高速
  - ミクロコスモス（1525手）を約1分で解ける
  - macOS (Intel/M1)、Windows対応
- **最終更新**: 2025年5月（v1.2.6）、20スター
- **参考**: [ミクロコスモスが1分で解ける謎のソフト「shtsume」](https://mycube.blog/shtsume/)

#### semiexp/tsumeshogi
- **URL**: https://github.com/semiexp/tsumeshogi
- **言語**: Rust (100%)
- **アルゴリズム**: 不明（証明数ベースと推測）
- **最終更新**: 小規模プロジェクト、8コミット、0スター

#### koba-e964/tsumeshogi-web-solver
- **URL**: https://github.com/koba-e964/tsumeshogi-web-solver
- **言語**: TypeScript (62.3%), JavaScript (25.2%), Rust (10.1%)
- **特徴**: Webベースソルバー、WASM使用
- **最終更新**: 2024年12月、1スター

#### snipsnipsnip/narazu
- **URL**: https://github.com/snipsnipsnip/narazu
- **言語**: Haskell (88.2%), Ruby (11.8%)
- **アルゴリズム**: 全探索（brute-force）
- **備考**: 教育的実装、Haskellによる珍しいアプローチ
- **最終更新**: 2013年、1スター

#### qhapaq-49/checkmate-gps-osl
- **URL**: https://github.com/qhapaq-49/checkmate-gps-osl
- **言語**: C++ (100%)
- **アルゴリズム**: df-pn（GPSshogi/OpenShogiLib経由）
- **特徴**: CSA入力、ノード制限設定可
- **最終更新**: 2020年12月、0スター
- **参考**: [高速な詰将棋アルゴリズムを完全に理解したい](https://qhapaq.hatenablog.com/entry/2020/07/19/233054)

### 2.2 詰将棋機能を持つ将棋エンジン

#### YaneuraOu（やねうら王）
- **URL**: https://github.com/yaneurao/YaneuraOu
- **言語**: C++ (96.4%)
- **詰将棋機能**: df-pnベースの詰将棋エンジン（yaneuraou-mate-engine）
- **特徴**:
  - 世界最強クラスの将棋エンジン（WCSC29優勝）
  - 専用詰将棋エンジンモード（V2: 省メモリ、長手数対応）
  - USI対応、最大256スレッド
  - クロスプラットフォーム（Windows/Linux/macOS/Android/WASM）
- **最終更新**: 2025年11月、635スター
- **参考**: [詰将棋アルゴリズムdf-pnのすべて](https://yaneuraou.yaneu.com/2024/05/08/all-about-df-pn/)

#### DeepLearningShogi（dlshogi）
- **URL**: https://github.com/TadaoYamaoka/DeepLearningShogi
- **言語**: C++ (79.9%), Python (18.4%)
- **詰将棋機能**: MCTS + df-pnで終盤の詰み探索
- **最終更新**: アクティブ、228スター

#### Fairy-Stockfish
- **URL**: https://github.com/fairy-stockfish/Fairy-Stockfish
- **言語**: C++（Stockfish派生）
- **詰将棋機能**: "Tsume"オプション有効化で詰将棋モード
- **備考**: 短手数向き。df-pnベースのソルバーには長手数で劣る

### 2.3 学術研究（公開リポジトリなし）

#### Sakuta & Iida の衝立詰将棋ソルバー
- **アルゴリズム**: 二重反復深化探索 + メタポジション概念
- **論文**: [Solving Problems with Uncertainty: A case study using Tsuitate-Tsume-Shogi](https://www.researchgate.net/publication/241918197_Solving_Problems_with_Uncertainty_A_case_study_using_Tsuitate-Tsume-Shogi) (1999/2000)
- **備考**: 衝立詰将棋をコンピュータで解く最初の学術研究。メタポジション概念を導入。tsuitate-solverのアプローチの学術的先駆者。

### 2.4 まとめ表

| # | リポジトリ | 言語 | アルゴリズム | 最大問題規模 | スター | 最終更新 |
|---|-----------|------|------------|------------|--------|---------|
| 1 | KomoringHeights | C++ | df-pn+ | 1525手以上 | 38 | 2024-06 |
| 2 | sugyan/tsumeshogi-solver | Rust | df-pn | 中〜長手数 | 21 | 2022-09 |
| 3 | sugyan/tsumeshogi-solver-wasm | Rust+JS | df-pn (WASM) | 中手数 | 3 | 2022-05 |
| 4 | hkijin/shtsume | C | PNS/df-pn | 1525手以上 | 20 | 2025-05 |
| 5 | semiexp/tsumeshogi | Rust | 不明 | 不明 | 0 | - |
| 6 | koba-e964/tsumeshogi-web-solver | TS+Rust | WASM | 短〜中手数 | 1 | 2024-12 |
| 7 | snipsnipsnip/narazu | Haskell | 全探索 | 短手数 | 1 | 2013 |
| 8 | checkmate-gps-osl | C++ | df-pn (GPS) | 長手数 | 0 | 2020-12 |
| 9 | YaneuraOu | C++ | df-pn | 1525手以上 | 635 | 2025-11 |
| 10 | DeepLearningShogi | C++/Python | MCTS+df-pn | 中手数 | 228 | アクティブ |
| 11 | Fairy-Stockfish | C++ | α-β+Tsume | 短〜中手数 | - | アクティブ |

### 2.5 重要な知見

1. **df-pnが事実上の標準**: 現代のほぼ全ての本格的詰将棋ソルバーがNagaiのdf-pnアルゴリズムの変種を使用
2. **Rustの台頭**: C++が依然として主流だが、Rust実装が複数登場（sugyan, semiexp）
3. **トップ2ソルバー**: KomoringHeights（最も高度な枝刈り技術）とshtsume（最速、ミクロコスモス約1分）
4. **公開された衝立詰将棋ソルバーはtsuitate-solverのみ**: Sakuta & Iidaの学術的コード以外に公開実装は存在しない
5. **無駄合い判定は最難関**: 専門開発者でも厳密な実装を避けるほどの複雑さ。KomoringHeightsはv1.0.0で無駄合い判定を削除した
