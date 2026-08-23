# tsuitate-solver

衝立詰将棋（ついたてつめしょうぎ）ソルバー。Tauri v2 + Svelte 5 + Rust。

## 衝立詰将棋のルール

衝立詰将棋は通常の詰将棋の変種で、攻め方（先手）と玉方（後手）の間に衝立（ついたて＝スクリーン）が置かれている。

### 通常の詰将棋との共通点
- 攻め方（先手）が毎手王手をかけ、玉方（後手）の玉を詰ませる
- 玉方は最善の応手（最も長く逃れる手）を選ぶ
- 攻め方の持ち駒以外の駒は全て玉方の持ち駒
- 無駄合い（取り返されて同じ結果になる合駒）は省略される

### 衝立詰将棋の固有ルール

**不完全情報**: 攻め方は玉方の応手を直接観察できない。攻め方が手を指した後に得られる情報（観測, Observation）は以下の4種のみ:

1. **NoCapture（駒取りなし）**: 攻め方が打った/動かした駒がそのまま盤上に残っている
2. **Captured（駒取りあり）**: 攻め方の駒が玉方に取られた（何の駒で取られたかは不明）
3. **Checkmate（詰み）**: 玉方に合法手がない（= 詰んだ）
4. **Illegal（反則）**: 攻め方の手がルール上指せなかった（例: 二歩、行き場のない駒）

**メタポジション**: 攻め方は玉方の実際の応手が分からないため、「現在の観測と矛盾しない全ての盤面状態の集合」を管理する。これを MetaPosition と呼ぶ。

**解の条件**: 攻め方の手順は、MetaPosition 内の**全ての盤面**に対して、**どの観測結果が返ってきても**詰みに導けなければならない。つまり解の手順木は観測結果ごとに分岐する。

**反則の利用**: 一部の盤面でのみ合法な手（プローブ手）を指すと、Illegal/NoCapture/Captured の観測で MetaPosition を分割でき、情報収集に使える。ただし攻め方には毎手王手の義務があるので、実際に使えるプローブ手は「指せた盤面では王手になっている手」に限られる。

### 通常の詰将棋との手数の違い

同じ盤面でも、衝立詰将棋では通常の詰将棋より長い手数が必要になることが多い。攻め方は玉の位置を直接知れないため、全ての可能性に対応する手順が必要。

### 解の手順木の構造

```
AttackMove(▲2二飛打)
├── Checkmate → 詰み
├── Captured → AttackMove(▲次の手) → ...
├── NoCapture → AttackMove(▲次の手) → ...
└── Illegal → AttackMove(▲別の手) → ... （反則時は同じ深さで再探索）
```

## アーキテクチャ

### バックエンド (Rust: `src-tauri/src/`)

```
src-tauri/src/
├── lib.rs              # Tauri プラグイン設定、CancelFlag 管理
├── main.rs             # エントリポイント
├── commands.rs         # Tauri IPC コマンド (solve, cancel_solve, validate_position)
├── shogi/
│   ├── types.rs        # 基本型: Color, PieceKind, Piece, Square, Move, HandPieces
│   ├── position.rs     # Position: 盤面状態、make_move/unmake_move、合法手生成、無駄合い判定
│   └── movegen.rs      # 擬似合法手生成、王手判定、王手回避手生成
└── solver/
    ├── solution.rs     # Observation, SolutionNode, SolutionBranch, SolutionData
    ├── metaposition.rs # MetaPosition: 可能局面集合の管理、応手展開、重複排除
    ├── solver.rs       # TsuitateSolver: 指数的反復深化ソルバー（旧メインソルバー）
    └── tsuitate_dfpn.rs # TsuitateDfpnSolver: df-pn ソルバー（現メインソルバー、GUIで使用）
```

### フロントエンド (Svelte 5 + TypeScript: `src/lib/`)

```
src/lib/
├── App.svelte          # メインレイアウト
├── Board.svelte        # 9x9 盤面表示・駒配置 UI
├── PieceSelector.svelte # 駒種選択パネル
├── Controls.svelte     # 解探索制御（解探索、キャンセル、余詰めチェック、JSON入出力）
├── Solution.svelte     # 解の手順木表示
├── stores.ts           # Svelte ストア（盤面状態、持ち駒、UI 状態）
└── types.ts            # TypeScript 型定義
```

## ソルバー

### TsuitateDfpnSolver（現メインソルバー）

GUIおよびベンチマークで使用する df-pn (Depth-First Proof Number Search) ベースのソルバー。

- **OR ノード**: MetaPosition（攻め方が手を選ぶ）
- **AND ノード**: 観測分岐（Checkmate / Captured / NoCapture / Illegal、最大4分岐）
- AND ノードの分岐数が最大4であるため、通常の詰将棋の AND ノード（王手回避手＝数十手）に比べて大幅に効率的
- 転置表: OR テーブル（MetaPosition ハッシュ）と AND テーブル（meta_hash + move）の2つ
- ノード上限で探索を制御（深さ制限なし）
- **候補手は「全盤面の王手手の和集合」だけ**（`generate_attack_candidates`）。王手にならない手は、指せた盤面が1つでもあれば mid_and の「合法だった盤面の全てで王手」チェックで必ず反証されるため、候補に入れても AND ノードを無駄にするだけで証明に寄与しない。情報収集用のプローブ手も、王手になるものは各盤面の王手手の和集合に含まれている
- 手生成の段階で王手候補に絞る: 打ちは `drop_check_mask`（その駒種が玉に利きうるマス）だけを列挙する。空きマス全部を列挙すると1駒種あたり最大80手になり、ここが探索全体で最も重かった。盤上の駒の移動は駒数が少ないので全生成し、幾何学的事前フィルタ（`could_give_check`）で絞る。いずれも偽陰性なしの絞り込みで、最後に実際に指して王手かを確認する
- 情報集合の各盤面で「この手が指せるか」は `Position::is_legal_move` で1手だけ検証する（全合法手を生成して照合しない）。ルールの二重管理を避けるため、生成側と同じ `drop_targets` / `generate_piece_moves` を使う。`tests/shogi_tests.rs` の `is_legal_move_matches_generate_legal_moves` が全マス×全駒種で `generate_legal_moves` との一致を検査している
- AND ノード展開キャッシュ（`and_expansion_cache`）には件数上限がある。1件が情報集合まるごとを抱えるため、上限なしだとGB級に膨らみ、ヒットで得た分よりメモリ帯域で失う分が上回る
- 余詰めチェック対応: **常に主解を最短化してから**初手・内部手を除外して再探索（余詰めは「最短解に対する別手順」として判定。非最短の主解木を検査すると判定が探索順依存になるため。--shortest 指定の有無によらない）
- 証明木の抽出: 転置表を辿って SolutionNode ツリーを構築

### ソルバーCLI（tsuitate-solver-cli）

`src-tauri/src/bin/cli.rs`。Webサイト（tsuitate リポジトリ）の投稿検証・挑戦モードから spawn されるヘッドレスバイナリ。ビルド: `cargo build --release --bin tsuitate-solver-cli`

- 通常モード: `<question.json> [--find-second] [--shortest] [--node-limit N] [--timeout-secs N] [--memory-limit-mb N] [--estimate-rating] [--rating-node-limit N]`
- 挑戦モード: `--solve-meta <request.json> [--node-limit N] [--scan-node-limit N] [--parallel N]`（決定性のためタイムアウト・メモリ上限なし）
- 挑戦モード常駐版: `--solve-meta-server [--node-limit N] [--scan-node-limit N]`。stdin から行区切りJSONを受け続け、転置表を手をまたいで温存する。挑戦モードは1手ごとに「前の手の証明の部分木」を問い直すため、2手目以降がほぼ即答になる
- **--scan-node-limit `N|auto|unlimited`**: Proven 時の「最小証明深さスキャン」専用の追加ノード予算。スキャンは玉方のタイブレーク（どの観測分岐が一番粘れるか）用の精密化で、真偽判定には影響しない。既定は `auto`（= `SCAN_NODES_AUTO` = 50,000ノード）。スキャンの最後の1回は必ず「これ以上短くはできない」ことの反証になり、本解の証明より遥かに重くなりやすく、放っておくと1手の応答時間の5〜8割を占める。打ち切ってもタイブレークが粗くなるだけで、返す `provenDepth` は常に証明済みの上界のまま。**`0` は `auto` の別名**（「既定のつもりで 0」で無制限になる事故を避けるため。以前の版では 0 が無制限だった）。厳密な最短手数が要るときだけ `unlimited` を渡す
- `provenDepth` は「予算内で証明できた深さの上界」なので、転置表が温まっているほど小さくなり得る（常駐モードと単発実行で違う値が返り得る）。サイト側で保存・再検証に使うなら、値そのものではなく「その手数以内に詰む」という上界として扱うこと
- **--memory-limit-mb**: ピークRSSの上限。監視スレッドがソフト上限でキャンセルフラグを立て（出力に `memoryLimited: true`）、1.5倍のハード上限でフォールバックJSONを出して正常終了する（OOM killer 対策）。余詰め探索（find_second）は情報集合の再展開でGB級のメモリを食うことがあり、走査系（find_table_alternative / find_inner_alt_recursive）には MAX_META_POSITIONS 超のメタで打ち切って「判定不能」扱いにするガードがある。`expand_defense_moves` は協調キャンセル（`metaposition.rs` の `set_expansion_cancel`）を確認し、**キャンセル発火後の戻り値は部分結果の可能性があるため呼び出し側は必ず破棄する**こと（空の分岐を「詰み」と誤認すると偽の証明になる）

### 難易度レート推定（--estimate-rating）

`src-tauri/src/solver/rating.rs`。サイト（tsuitate）の**詰めチャレ**（1問ずつレート連動で出題するモード）が問題に付ける**初期レート**を求める。問題レートは実戦の Elo で自己補正されるので、ここで出すのは事前分布。

- 特徴量はすべて解答過程から決まる決定的な量（乱数も実測時間も使わない）: `depth` / `rootTries`（初期局面の合法手数）/ `rootChecks`（うち王手が保証される手）/ `rootSolutions`（うち制限手数内に詰む手）/ `solutionBranches` / `maxBranch` / `nodes` / `hasSecond`
- **初手ごとの判定**（`analyze_root_moves`）は、各合法手について「王手が保証されるか」→「玉方の全観測分岐で残り `depth-2` 手以内の詰みが証明できるか」を調べる。本解を1つ求めるより重いので、**オフラインの問題生成パイプライン専用**（サイトの投稿検証では使わない）
- 判定に使うソルバーは1つを使い回して転置表を温存する（初手が違っても同じ部分木を踏むため）。手の列挙順は固定なので結果は決定的
- **`--timeout-secs` / `--memory-limit-mb` のキャンセルフラグを共有する**。本解より重いので共有しないとタイムアウトがこの区間に効かない（呼び出し側の mine_tsume に外側のタイムアウトはない）。発火したら `rating` を付けずに出力する（途中まで数えた特徴量は実際より易しいレートになるうえ、`expand_defense_moves` の部分結果は信用できない）。採掘側はレートのない問題を捨てる
- 難易度の分母は `rootTries` ではなく **`rootChecks`**。合法手数はどの問題でも100前後でほぼ定数になり識別力がないが、王手が保証される手の数は解き手が実際に読む候補の幅そのもの
- **v1 の係数は実測データのない暫定値**。詰めチャレの実戦結果（問題レートの収束先）と突き合わせて校正する前提で、特徴量を出力にそのまま載せてある（後から重みだけ引き直せる）。式を変えたら `FORMULA_VERSION` を上げること

### TsuitateSolver（旧ソルバー）

指数的反復深化ベースのソルバー。ベンチマーク比較用に残してある。

1. **指数的反復深化**: 深さ 1, 3, 7, 15, ... の順に探索
2. **転置表**: 失敗した MetaPosition をハッシュでキャッシュし再探索を回避
3. **候補手ソート**: 打ち駒優先 → 玉に近い手 → 接触王手 → 長距離利き駒の順
4. **枝刈り**: メタポジションサイズ上限(5000)、合駒可能マス数制限、候補手数制限
5. **無駄合い判定**: 取り返して再王手→詰みになる合駒を再帰的に判定して省略

## 既知の重要な実装上の注意

- `is_square_attacked()` (movegen.rs): 非対称駒（金・銀・桂・歩・香）は、ターゲットマスから攻撃者を探す際に**相手側**のオフセットを使う必要がある
- `make_move`/`unmake_move`: 玉は持ち駒に加えない（玉の捕獲はソルバーの探索上発生しうるが、持ち駒に加えると不正な状態になる）
- 解の手順木の `MoveData` から `Move` を復元して情報集合に適用するときは、必ず `md_to_move`（盤上手の駒種を meta から解決する）を使う。`moved_piece_kind` はコメント上「表示用」だが `Move` の Eq/Hash に参加しているため、駒種を落とした手は `Position::try_make_legal` で全局面が不合法と判定され、観測分岐の走査が**静かに**止まる（余詰めの見逃しを「完全作」として確定報告する）。復元できないときは代用せず `second_search_aborted` を立てて「判定不能」にすること。除外手の照合（from/to/成/打ちしか見ない）には駒種を持たない `ExclusionKey` を使う — **指せない型**にして取り違えを型で防いでいるので、ここから `Move` を作らないこと
- 情報集合に1手を適用する合法性判定は `Position::try_make_legal`（判定と着手を兼ねてクローン1回）。`is_legal_move` はその薄いラッパで、`tests/shogi_tests.rs` の `is_legal_move_matches_generate_legal_moves` が全マス×全駒種で `generate_legal_moves` との一致を検査している。判定ロジックを `apply_attack_move_split` 側に写して二重管理にしないこと（総当たりテストが本番経路を守らなくなる）

## ビルド・テスト

```bash
# Rust テスト
cd src-tauri && cargo test
cd src-tauri && cargo test --release --test solver_tests

# 長時間テスト（#[ignore] 付き）
cd src-tauri && cargo test --release -- --ignored

# フロントエンドビルド
npx vite build

# アプリ起動
npm run tauri dev
```

## ベンチマークテスト

`sample-questions/` 配下の問題ファイル（1.json〜147.json）を使ったベンチマークテストがある。全テストに `#[ignore]` が付いているため `--ignored` フラグが必要。性能測定のため `--release` ビルドを推奨。結果ファイルは `benchmark/` ディレクトリに出力される。

### df-pn ソルバー（推奨）

`src-tauri/tests/dfpn_benchmark_tests.rs` — 現メインソルバーのベンチマーク。

```bash
# 個別の問題を実行（例: 問題1）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_question_01 -- --ignored --nocapture

# 全問一括ベンチマーク（サマリー表付き）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_all -- --ignored --nocapture

# 分割ベンチマーク（通常、5分割: part1=1-30, part2=31-60, part3=61-90, part4=91-120, part5=121-147）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_part1 -- --ignored --nocapture
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_part2 -- --ignored --nocapture

# 個別の問題を実行（最短経路探索あり、例: 問題34）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_shortest_question_34 -- --ignored --nocapture

# 分割ベンチマーク（最短経路探索あり）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_shortest_part1 -- --ignored --nocapture

# 個別の問題を実行（余詰めチェック、例: 問題52）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_second_question_52 -- --ignored --nocapture

# 分割ベンチマーク（余詰めチェック）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_second_part1 -- --ignored --nocapture

# 全問一括ベンチマーク（全種類）
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_all -- --ignored --nocapture
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_all_shortest -- --ignored --nocapture
cd src-tauri && cargo test --release --test dfpn_benchmark_tests dfpn_bench_all_second -- --ignored --nocapture
```

- ノード上限: 50,000,000
- 制限時間: 120秒/問
- 最短経路探索: 初回探索で得た解の深さに対し、深さ制限付き二分探索で最短解を求める（GUIでは「最短経路を調べる」オプションで有効化）
- 余詰めチェック: 主解の初手を除外して再探索し、別解がないか確認する

### 挑戦モードの応答時間（challenge_bench）

`src-tauri/tests/challenge_bench.rs` — サイトの挑戦モードが1手ごとに返す応答の時間を測る。攻め方（＝挑戦者）の指し手は解の手順木から採り（計測対象外）、玉方の各観測分岐について `solve_meta_query_with` を投げて最も粘れる分岐を選ぶ、という `--solve-meta-server` と同じ流れをシミュレートする。ソルバーは1問を通して使い回すので、常駐モードと同じく2手目以降は温まった転置表が効く。

```bash
# 重い問題だけを流す（環境変数で問題番号を指定）
cd src-tauri && CHALLENGE_QUESTIONS=8,101,111,114,117,128,134 \
  cargo test --release --no-default-features --test challenge_bench challenge_bench_pick -- --ignored --nocapture

# 問題1〜40
cd src-tauri && cargo test --release --no-default-features --test challenge_bench challenge_bench_all -- --ignored --nocapture
```

- `CHALLENGE_NODE_LIMIT`（既定 2,000,000）、`CHALLENGE_SCAN_NODE_LIMIT`（既定 0 = 自動）で CLI と同じ予算を再現できる
- `CHALLENGE_VERBOSE=1` で1手ごとの内訳（応手展開・候補手生成・リプレイの時間）を出す
- 応答時間は初手（＝転置表が空の状態）に集中する。多くの問題は数ms〜数百msだが、深い問題では秒オーダーになる

### 旧ソルバー（比較用）

`src-tauri/tests/benchmark_tests.rs` — 旧 TsuitateSolver のベンチマーク。

```bash
# 全問一括ベンチマーク
cd src-tauri && cargo test --release --test benchmark_tests bench_all_questions -- --ignored --nocapture
```

- 最大探索深さ: 15手
- 制限時間: 60秒（全問一括は120秒/問）
- `--nocapture` を付けると盤面表示・解の手順木・サマリー表が出力される
