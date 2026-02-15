# df-pn における不要ループ・最短手順の問題と対策

## 問題の概要

問題34で、解の手順木に「▲2二歩打→▲2一歩成→取られる」の繰り返しが5回含まれるが、このループは実行する意味がなく、直接▲3二金に進んだ方が短い手数で詰む。

## 根本原因: df-pn は最短手順を保証しない

複数の情報源が一致して指摘：

> **df-pnで見つかった詰み局面は最短手数であることが保証されていません**（Qhapaq ブログ）

df-pn は「詰みが存在すること」を証明するアルゴリズムであり、最短の詰み手順を見つけるアルゴリズムではない。不要なループを含む冗長な手順が解として出力されうるのは仕様通りの挙動。

---

## 対策手法（実装難易度順）

### 1. 証明木抽出時のループ検出（簡単）

`extract_or()` で訪問済み MetaPosition ハッシュと深さを記録し、同じハッシュがより深い位置で再出現したら「ループ」と判断して別の手を試す。

```rust
fn extract_or(
    &self,
    meta: &MetaPosition,
    depth: u32,
    path: &mut HashSet<u64>,
    visited_states: &mut HashMap<u64, u32>,  // hash -> first depth seen
) -> Option<SolutionNode> {
    let hash = meta_position_hash(meta);

    // 同じ MetaPosition がより浅い深さで既に出現していたらループ
    if let Some(&first_depth) = visited_states.get(&hash) {
        if depth > first_depth + 2 {
            return None;  // 親ノードに別の手を試させる
        }
    }
    visited_states.insert(hash, depth);
    // ... 残りの抽出処理
}
```

**効果**: 問題34のようなケースを直接解決。
**限界**: 転置表に記録された証明木の構造に依存するため、ループを含まない代替手が転置表に存在しない場合は機能しない。

### 2. 深さ制限付き df-pn 再探索（中程度、最も標準的）

1. まず通常の df-pn で解を見つけ、その深さ `D` を得る
2. 深さ `D-2` 以上の局面を「不詰」として扱い、df-pn を再実行
3. まだ詰みが見つかれば、さらに深さを縮める（二分探索）
4. 詰みが見つかる最小の深さが最短手順

> 「深さN以上の局面については強制的に不詰とすることで、手数をN手に縛った探索を行う」（Qhapaq ブログ）

**実装**: `mid_or` / `mid_and` に `depth_limit` パラメータを追加し、制限を超えた局面を `(INF, 0)`（不詰）として扱う。

**効果**: 最短手順を保証。
**コスト**: 再探索が必要だが、転置表が残っていれば高速。

### 3. 転置表に証明深さを記録（中程度）

`PnDn` エントリに `proven_depth`（何手で詰みが証明されたか）を追加：

```rust
struct PnDn {
    pn: u32,
    dn: u32,
    proven_depth: u32,  // 詰みまでの深さ（未証明なら u32::MAX）
}
```

抽出時により短い `proven_depth` の手を優先する。

**効果**: 抽出精度の改善。
**限界**: 探索中に最短経路が見つかっている必要がある。

### 4. 反復深化ラッパー（やや難）

df-pn を深さ 1, 3, 5, 7, ... と反復深化で呼び出す。浅い深さから探索するため最短手順が自然に保証される。

衝立詰将棋の元祖ソルバー（Sakuta & Iida）はこの方式を採用していた。

**効果**: 最初から最短を保証。
**コスト**: 浅い深さで不詰の場合の再探索コスト（ただし通常は高速）。

### 5. TCA + SNDA（難、完全なGHI対策）

岸本章宏氏の手法。サイクル検出を探索本体に組み込む：

- **TCA (Threshold Controlling Algorithm)**: サイクル検出時に閾値を増加させ、ループを越えて探索を継続
- **SNDA (Source Node Detection Algorithm)**: 過大評価の原因ノードを検出

KomoringHeights (GitHub: komori-n/KomoringHeights) が実装例。

**効果**: 全サイクル問題を正しく処理。
**コスト**: 実装が複雑。

---

## GHI (Graph History Interaction) 問題

df-pn における千日手/ループの扱いは「GHI問題」として知られる。同じ局面でも到達経路によってゲーム理論的な値が異なるため、経路情報を無視した転置表では誤った結果を返しうる。

現ソルバーの `lookup_or()` にあるパスベースのサイクル検出：

```rust
if path.contains(&hash) {
    return (INF, 0);  // 現在のパス上のサイクルは不詰扱い
}
```

これは探索中の無限ループを防ぐが、「概念的には同じ状態だがハッシュが微妙に異なる」ループ（持ち駒の変化等）は検出できない。

### GHI対策の2つのアプローチ（日本の詰将棋ソルバーで使用）

1. **パス区別方式**: 異なるパスで到達した同一局面を完全に別ノードとして扱う（やねうら王の詰みエンジンで使用）。単純だが転置表の効率が下がる。

2. **Base/Twin テーブル方式**: 千日手に関わる結果と関わらない結果を区別して管理。KomoringHeights で採用。

---

## 参考資料

### 論文・学術資料

- [Kishimoto & Mueller, "A General Solution to the Graph History Interaction Problem" (AAAI 2004)](https://cdn.aaai.org/AAAI/2004/AAAI04-102.pdf)
- [Kishimoto & Mueller, "A solution to the GHI problem for depth-first proof-number search" (Information Sciences, 2005)](https://www.sciencedirect.com/science/article/abs/pii/S0020025504002749)
- [Kishimoto, "Dealing with Infinite Loops, Underestimation, and Overestimation of df-pn" (AAAI 2010)](https://ojs.aaai.org/index.php/AAAI/article/view/7534) — TCA + SNDA の提案論文
- [Kishimoto, Winands, Mueller, Saito, "Game-Tree Search Using Proof Numbers: The First Twenty Years" (ICGA 2012)](https://webdocs.cs.ualberta.ca/~mmueller/ps/ICGA2012PNS.pdf)
- [Sakuta & Iida, "Solving Problems with Uncertainty: Tsuitate-Tsume-Shogi" (1999)](https://www.researchgate.net/publication/241918197) — 衝立詰将棋の元祖ソルバー
- [Sakuta 研究ページ](https://www.fit.ac.jp/~sakuta/research/tts-papers.html)

### 日本語ブログ・解説

- [komorinfo.com - df-pnの基本](https://komorinfo.com/blog/df-pn-basics/) — 「証明木は最短手順とは限らない」の記述あり
- [komorinfo.com - AND/OR木のGHI問題](https://komorinfo.com/blog/and-or-tree-ghi-problem/) — GHI対策の詳細解説
- [Qhapaq ブログ - 詰将棋を高速に解くアルゴリズム](https://qhapaq.hatenablog.com/entry/2020/07/19/233054) — 深さ制限付き df-pn の解説
- [sugyan - df-pn詰将棋ソルバーの最善解・余詰め解抽出](https://memo.sugyan.com/entry/2018/02/25/184347)
- [sugyan - Rust詰将棋ソルバーの改良](https://memo.sugyan.com/entry/2021/12/10/153440)
- [やねうら王 - df-pnのすべて](https://yaneuraou.yaneu.com/2024/05/08/all-about-df-pn/)

### 実装例

- [KomoringHeights (GitHub)](https://github.com/komori-n/KomoringHeights) — TCA+SNDA を実装した詰将棋ソルバー
- [sugyan/tsumeshogi-solver (GitHub)](https://github.com/sugyan/tsumeshogi-solver) — Rust製 df-pn 詰将棋ソルバー

### その他

- [Chessprogramming Wiki - Proof-Number Search](https://www.chessprogramming.org/Proof-Number_Search)
- [Chessprogramming Wiki - Graph History Interaction](https://www.chessprogramming.org/Graph_History_Interaction)
- [minimax.dev - Depth-First Proof Number Search](https://minimax.dev/docs/ultimate/pn-search/dfpn/)
