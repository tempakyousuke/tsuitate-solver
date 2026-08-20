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

**反則の利用**: 一部の盤面でのみ合法な手（プローブ手）を指すと、Illegal/NoCapture/Captured の観測で MetaPosition を分割でき、情報収集に使える。

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
- 余詰めチェック対応: **常に主解を最短化してから**初手・内部手を除外して再探索（余詰めは「最短解に対する別手順」として判定。非最短の主解木を検査すると判定が探索順依存になるため。--shortest 指定の有無によらない）
- 証明木の抽出: 転置表を辿って SolutionNode ツリーを構築

### ソルバーCLI（tsuitate-solver-cli）

`src-tauri/src/bin/cli.rs`。Webサイト（tsuitate リポジトリ）の投稿検証・挑戦モードから spawn されるヘッドレスバイナリ。ビルド: `cargo build --release --bin tsuitate-solver-cli`

- 通常モード: `<question.json> [--find-second] [--shortest] [--node-limit N] [--timeout-secs N] [--memory-limit-mb N] [--estimate-rating] [--rating-node-limit N]`
- 挑戦モード: `--solve-meta <request.json> [--node-limit N]`（決定性のためタイムアウト・メモリ上限なし）
- **--memory-limit-mb**: ピークRSSの上限。監視スレッドがソフト上限でキャンセルフラグを立て（出力に `memoryLimited: true`）、1.5倍のハード上限でフォールバックJSONを出して正常終了する（OOM killer 対策）。余詰め探索（find_second）は情報集合の再展開でGB級のメモリを食うことがあり、走査系（find_table_alternative / find_inner_alt_recursive）には MAX_META_POSITIONS 超のメタで打ち切って「判定不能」扱いにするガードがある。`expand_defense_moves` は協調キャンセル（`metaposition.rs` の `set_expansion_cancel`）を確認し、**キャンセル発火後の戻り値は部分結果の可能性があるため呼び出し側は必ず破棄する**こと（空の分岐を「詰み」と誤認すると偽の証明になる）

### 難易度レート推定（--estimate-rating）

`src-tauri/src/solver/rating.rs`。サイト（tsuitate）の**詰めチャレ**（1問ずつレート連動で出題するモード）が問題に付ける**初期レート**を求める。問題レートは実戦の Elo で自己補正されるので、ここで出すのは事前分布。

- 特徴量はすべて解答過程から決まる決定的な量（乱数も実測時間も使わない）: `depth` / `rootTries`（初期局面の合法手数）/ `rootChecks`（うち王手が保証される手）/ `rootSolutions`（うち制限手数内に詰む手）/ `solutionBranches` / `maxBranch` / `nodes` / `hasSecond`
- **初手ごとの判定**（`analyze_root_moves`）は、各合法手について「王手が保証されるか」→「玉方の全観測分岐で残り `depth-2` 手以内の詰みが証明できるか」を調べる。本解を1つ求めるより重いので、**オフラインの問題生成パイプライン専用**（サイトの投稿検証では使わない）
- 判定に使うソルバーは1つを使い回して転置表を温存する（初手が違っても同じ部分木を踏むため）。手の列挙順は固定なので結果は決定的
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
- `generate_attack_candidates` のソートは、単一局面の早期リターンより**前**に行う必要がある（そうしないとルートの手順が HashSet の反復順に依存し非決定的になる）

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

### 旧ソルバー（比較用）

`src-tauri/tests/benchmark_tests.rs` — 旧 TsuitateSolver のベンチマーク。

```bash
# 全問一括ベンチマーク
cd src-tauri && cargo test --release --test benchmark_tests bench_all_questions -- --ignored --nocapture
```

- 最大探索深さ: 15手
- 制限時間: 60秒（全問一括は120秒/問）
- `--nocapture` を付けると盤面表示・解の手順木・サマリー表が出力される
