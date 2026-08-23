use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use super::fasthash::{FxHashMap, FxHashSet, FxHasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::bitboard::Bitboard;
use crate::shogi::position::Position;
use crate::shogi::types::*;

/// 証明数/反証数の上限（sum時のオーバーフロー防止）
const INF: u32 = u32::MAX / 2;

/// メタポジションのサイズ上限
const MAX_META_POSITIONS: usize = 5000;

// ===== 無駄合い判定（衝立の正しい定義） =====
// 合駒 S が無駄合い ⟺ ①スライダーで S を取れて王手継続 ②後手が取り返せない
// ③取った後（スライダー@S）から先手が依然として詰ませられる。
// ③は完全情報の単一局面詰み探索で検証する（メタポジション探索を単一局面に再利用）。

thread_local! {
    /// muda_ai の結果キャッシュ（局面ハッシュ+手 → 真偽）。純関数なので安全。
    static MUDA_CACHE: std::cell::RefCell<FxHashMap<(u64, u64), bool>> =
        std::cell::RefCell::new(FxHashMap::default());
}

fn muda_move_key(m: &Move) -> u64 {
    let to = m.to.index() as u64;
    let from = m.from.map_or(127u64, |s| s.index() as u64);
    let promo = m.promotion as u64;
    let drop = m.drop_piece.map_or(15u64, |k| k as u64);
    to | (from << 7) | (promo << 14) | (drop << 15)
}

fn is_slider_kind(k: PieceKind) -> bool {
    matches!(
        k,
        PieceKind::Rook
            | PieceKind::Bishop
            | PieceKind::Lance
            | PieceKind::PromotedRook
            | PieceKind::PromotedBishop
    )
}

// pos（先手手番）から先手が詰ませられるか（完全情報の詰み検証、境界付き）。
// サブ探索中は無駄合い除外を無効化して再帰を防ぐ。
thread_local! {
    /// 完全情報の詰み検証の結果キャッシュ（局面ハッシュ → 詰ませられるか）。
    static MATE_CACHE: std::cell::RefCell<FxHashMap<u64, bool>> =
        std::cell::RefCell::new(FxHashMap::default());
    /// muda_folded_tree 1回あたりのサブ詰み探索の総ノード予算（残り）。
    /// 枯渇したら sente_can_mate は false を返す（＝無駄合いにしない＝枝を残す。
    /// 安全側：誤って短く報告しない）。病的問題の後処理暴走を防ぐ。
    static MUDA_BUDGET: std::cell::Cell<i64> = const { std::cell::Cell::new(0) };
    /// muda_folded_tree 1回あたりの実時間デッドライン。多数の小さな詰み検証で
    /// ノード予算が枯れない病的ケース（ソルバー生成オーバーヘッド律速）を止める。
    static MUDA_DEADLINE: std::cell::Cell<Option<std::time::Instant>> =
        const { std::cell::Cell::new(None) };
    /// 中断フラグ。デッドライン超過で立て、mfold は以降即座に戻る（O(木²)回避）。
    /// muda_folded_tree は立っていたら None を返す（畳み込みなし＝補正なし）。
    static MUDA_ABORTED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// サブ詰み探索に渡す共有キャンセルフラグ。タイマースレッドがデッドラインで立て、
    /// 走行中のサブ探索も含めて打ち切る（1回のサブ探索が長い病的ケースを止める）。
    static MUDA_CANCEL: std::cell::RefCell<Option<Arc<AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

fn sente_can_mate(pos: &Position) -> bool {
    let key = pos.zobrist_hash;
    if let Some(v) = MATE_CACHE.with(|c| c.borrow().get(&key).copied()) {
        return v; // キャッシュヒットは予算を消費しない
    }
    // 総予算 or 実時間デッドラインを超えたら詰み検証を諦める（保守的に false）
    if MUDA_BUDGET.with(|c| c.get()) <= 0 {
        return false;
    }
    if MUDA_DEADLINE.with(|c| c.get()).map_or(false, |d| std::time::Instant::now() >= d) {
        return false;
    }
    let meta = MetaPosition::new(pos.clone());
    // 予算を絞って有界化。無駄合いの詰みは短いので少数ノードで足りる。
    // 予算超過（Unknown）は「詰ませられない」と保守的に扱う（＝無駄合いにしない）。
    // 共有キャンセルフラグ（タイマー）で走行中でも実時間で打ち切る。
    let cancel = MUDA_CANCEL
        .with(|c| c.borrow().clone())
        .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
    let mut solver = TsuitateDfpnSolver::new(300_000, cancel);
    let r = solver.solve(&meta);
    // 消費ノードを総予算から差し引く
    MUDA_BUDGET.with(|c| c.set(c.get() - solver.nodes_searched as i64));
    let v = matches!(r, TsuitateDfpnResult::Proven);
    MATE_CACHE.with(|c| {
        let mut m = c.borrow_mut();
        if m.len() >= 200_000 {
            m.clear();
        }
        m.insert(key, v);
    });
    v
}

/// q（後手手番・王手されている）から先手が詰ませ切れるか（全応手に対して詰み）。
fn sente_mates_after(q: &Position) -> bool {
    let evasions = q.generate_check_evasions();
    if evasions.is_empty() {
        return true;
    }
    for ev in evasions {
        let mut r = q.clone();
        r.make_move(ev);
        if !sente_can_mate(&r) {
            return false;
        }
    }
    true
}

/// 無駄合い判定。pos: 後手手番・王手されている。def_mv: 後手の合駒。
pub fn muda_ai(pos: &Position, def_mv: &Move) -> bool {
    let key = (pos.zobrist_hash, muda_move_key(def_mv));
    if let Some(v) = MUDA_CACHE.with(|c| c.borrow().get(&key).copied()) {
        return v;
    }
    let v = muda_ai_impl(pos, def_mv);
    MUDA_CACHE.with(|c| {
        let mut m = c.borrow_mut();
        if m.len() >= 500_000 {
            m.clear();
        }
        m.insert(key, v);
    });
    v
}

fn muda_ai_impl(pos: &Position, def_mv: &Move) -> bool {
    let mut p = pos.clone();
    p.make_move(*def_mv); // 先手手番
    is_muda_position(&p)
}

/// p（先手手番。後手の合駒が先手スライダーの王手を塞いでいる局面）が無駄合い局面か。
/// スライダーで合駒を取って王手継続でき、後手が取り返せず、取った後に先手が詰ませ
/// られるなら真。合駒（＝取ると再王手になる後手駒）は列挙内で暗黙に特定される。
fn is_muda_position(p: &Position) -> bool {
    // 予算/デッドライン超過なら判定を諦める（保守的に false ＝無駄合いにしない）。
    if MUDA_ABORTED.with(|c| c.get()) || MUDA_BUDGET.with(|c| c.get()) <= 0 {
        return false;
    }
    if MUDA_DEADLINE.with(|c| c.get()).map_or(false, |d| std::time::Instant::now() >= d) {
        MUDA_ABORTED.with(|c| c.set(true));
        return false;
    }
    let moves = crate::shogi::movegen::generate_pseudo_legal_moves(p, Color::Sente);
    for mv in moves {
        let Some(from) = mv.from else { continue };
        let Some(piece) = p.piece_at(from) else { continue };
        if !is_slider_kind(piece.kind) {
            continue;
        }
        // 後手駒を取る手のみ（合駒を取る）
        match p.piece_at(mv.to) {
            Some(t) if t.color == Color::Gote => {}
            _ => continue,
        }
        let s = mv.to;
        let mut q = p.clone();
        q.make_move(mv); // 後手手番
        if q.is_in_check(Color::Sente) {
            continue;
        }
        if !q.is_in_check(Color::Gote) {
            continue; // 取った後に王手でなければ対象外
        }
        // 条件2: 後手が S を取り返せない
        let evs = q.generate_check_evasions();
        if evs.iter().any(|m| m.to == s) {
            continue;
        }
        // 条件3: スライダー@S から先手が詰ませられる
        if sente_mates_after(&q) {
            return true;
        }
    }
    false
}

fn muda_kanji_to_kind(s: &str) -> Option<PieceKind> {
    Some(match s {
        "飛" => PieceKind::Rook,
        "角" => PieceKind::Bishop,
        "金" => PieceKind::Gold,
        "銀" => PieceKind::Silver,
        "桂" => PieceKind::Knight,
        "香" => PieceKind::Lance,
        "歩" => PieceKind::Pawn,
        _ => return None,
    })
}

/// 余詰めチェックで「この手は使わない」を表す照合キー。
///
/// 除外は「同じ地点への同じ種類の着手」を弾くためのもので、動かす駒種は要らない。
/// `MoveData` からは駒種を復元できないが、`Move` を作って持ち回すと
/// 「指す手」として使い回せてしまう。`moved_piece_kind` は表示用と書いてある
/// くせに `Move` の Eq/Hash に参加しているため、駒種を落とした `Move` を情報集合に
/// 適用すると全局面が不合法になり、観測分岐の走査が**静かに**止まる。
/// 指せない専用の型にして、その取り違えを型で防ぐ。
/// 情報集合に適用する手が要るときは `md_to_move`（駒種を meta から解決する）を使う。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ExclusionKey {
    from: Option<Square>,
    to: Square,
    promotion: bool,
    drop_piece: Option<PieceKind>,
}

impl ExclusionKey {
    fn from_move_data(md: &MoveData) -> Self {
        Self {
            from: match (md.from_file, md.from_rank) {
                (Some(f), Some(r)) => Some(Square::new(f, r)),
                _ => None,
            },
            to: Square::new(md.to_file, md.to_rank),
            promotion: md.promotion,
            drop_piece: md.drop_piece.as_deref().and_then(muda_kanji_to_kind),
        }
    }

    /// 盤上手なら成/不成を反転したキー（除外は成・不成をまとめて弾く）
    fn promotion_counterpart(&self) -> Option<Self> {
        self.from?;
        Some(Self { promotion: !self.promotion, ..*self })
    }

    fn matches(&self, mv: &Move) -> bool {
        self.from == mv.from
            && self.to == mv.to
            && self.promotion == mv.promotion
            && self.drop_piece == mv.drop_piece
    }

    /// 成/不成の違いを無視して同じ着手か（主解の手を走査から外す用）
    fn same_target(&self, mv: &Move) -> bool {
        self.from == mv.from && self.to == mv.to && self.drop_piece == mv.drop_piece
    }
}

/// MoveData を Move に復元（盤上手は meta から動かす駒種を得る）。
///
/// **情報集合に適用する手は必ずこちらで作ること**。駒種を持たない復元は
/// `ExclusionKey`（除外手の照合専用で、指せない型）にしてある。
pub(crate) fn md_to_move(md: &MoveData, meta: &MetaPosition) -> Option<Move> {
    if let Some(dp) = &md.drop_piece {
        Some(Move::drop(Square::new(md.to_file, md.to_rank), muda_kanji_to_kind(dp)?))
    } else {
        let from = Square::new(md.from_file?, md.from_rank?);
        let kind = meta.positions.iter().find_map(|p| {
            p.piece_at(from)
                .filter(|pc| pc.color == Color::Sente)
                .map(|pc| pc.kind)
        })?;
        Some(Move::normal(
            from,
            Square::new(md.to_file, md.to_rank),
            md.promotion,
            kind,
        ))
    }
}

/// M0（反則枝の局面集合）が全て無駄合い局面か
fn all_muda(m0: &MetaPosition) -> bool {
    !m0.positions.is_empty() && m0.positions.iter().all(is_muda_position)
}

/// 事前計算した応手グループ列から観測 obs に一致するものを合成
fn group_union(groups: &[(Observation, MetaPosition)], obs: &Observation) -> MetaPosition {
    let mut positions = Vec::new();
    let mut seen: FxHashSet<u64> = FxHashSet::default();
    for (gobs, gmeta) in groups {
        if gobs == obs {
            for p in &gmeta.positions {
                if seen.insert(p.zobrist_hash) {
                    positions.push(p.clone());
                }
            }
        }
    }
    MetaPosition { positions }
}

/// 無駄合い反則枝を畳み込んだ表示木を構築する（後処理）。
/// 手順木をメタポジションと共に再生し、反則枝の局面集合が全て無駄合い局面なら
/// その枝を木から除去する。報告手数を畳み込み後の木の max_moves() から取ることで、
/// 報告手数と表示木に現れる最長手順が常に一致する。
///
/// 全反則枝の無駄合い判定（サブ詰み探索）が要りコストが高いため、メタ上限・
/// 実時間デッドライン・ノード予算・共有キャンセルで有界化する。中断時は None を
/// 返す（部分的に畳み込まれた木は使わず、呼び出し側は元の木・通常手数に
/// フォールバックすること。安全側：誤って短く報告しない）。
pub fn muda_folded_tree(tree: &SolutionNode, root: &MetaPosition) -> Option<SolutionNode> {
    // 1問あたりのサブ詰み探索の総ノード予算＋実時間デッドライン＋中断フラグをリセット。
    // 正当な無駄合い検証は通し、病的問題は打ち切って畳み込みなしにフォールバックする。
    MUDA_BUDGET.with(|c| c.set(5_000_000));
    MUDA_DEADLINE.with(|c| c.set(Some(std::time::Instant::now() + std::time::Duration::from_secs(8))));
    MUDA_ABORTED.with(|c| c.set(false));
    // 実時間デッドラインで発火する共有キャンセルフラグ＋タイマースレッドを用意。
    // 走行中のサブ探索も打ち切れる。タイマーは spawn しっぱなしで良い（8秒後に
    // 旧フラグを立てるだけ。次回呼び出しは新フラグに差し替わる）。
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_timer = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(8));
        cancel_timer.store(true, Ordering::Relaxed);
    });
    MUDA_CANCEL.with(|c| *c.borrow_mut() = Some(cancel));
    let folded = mfold(tree, root);
    MUDA_CANCEL.with(|c| *c.borrow_mut() = None);
    if MUDA_ABORTED.with(|c| c.get()) {
        None
    } else {
        Some(folded)
    }
}

/// 無駄合いを除外した手数を計算する（後処理）。
/// 畳み込み後の木の最長手順を返すため、muda_folded_tree の表示木と常に一致する。
/// 中断時は補正なし＝通常手数（安全側・誤って短く報告しない）。
pub fn muda_corrected_moves(tree: &SolutionNode, root: &MetaPosition) -> u32 {
    muda_folded_tree(tree, root).map_or_else(|| tree.max_moves(), |t| t.max_moves())
}

/// 木を再帰的に再構築し、無駄合い反則枝を除去する。
/// 中断時（MUDA_ABORTED）の戻り値は部分結果の可能性があるため、
/// muda_folded_tree が必ず破棄する。
fn mfold(node: &SolutionNode, meta: &MetaPosition) -> SolutionNode {
    // 既に中断済みなら即座に戻る（戻り値は muda_folded_tree で破棄される）。
    if MUDA_ABORTED.with(|c| c.get()) {
        return node.clone();
    }
    // 実時間デッドライン超過なら中断フラグを立てて即戻る（O(木²)フォールバック回避）。
    if MUDA_DEADLINE.with(|c| c.get()).map_or(false, |d| std::time::Instant::now() >= d) {
        MUDA_ABORTED.with(|c| c.set(true));
        return node.clone();
    }
    match node {
        SolutionNode::Checkmate { .. } => node.clone(),
        SolutionNode::AttackMove { mv, branches, meta_hash } => {
            // メタポジションが空/再構成不能、または大きすぎて再構成コストが高い場合は
            // このサブツリーの畳み込みを諦めてそのまま残す（保守的・安全側）。
            const MAX_MUDA_META: usize = 300;
            if meta.positions.is_empty() || meta.positions.len() > MAX_MUDA_META {
                return node.clone();
            }
            let Some(m) = md_to_move(mv, meta) else {
                return node.clone();
            };
            let (legal_after, illegal_before) = meta.apply_attack_move_split(m);
            // 展開先が大きい場合も畳み込みを諦める（expand_defense_moves の爆発回避）。
            if legal_after.positions.len() > MAX_MUDA_META
                || illegal_before.positions.len() > MAX_MUDA_META
            {
                return node.clone();
            }

            // 玉方応手の展開は1ノード1回だけ（幅広ノードでの再実行を回避）。
            // 反則/詰みしかない場合は展開不要。
            let need_groups = branches
                .iter()
                .any(|b| !matches!(b.observation, Observation::Illegal | Observation::Checkmate));
            let groups = if need_groups {
                legal_after.expand_defense_moves(m)
            } else {
                Vec::new()
            };

            // 反則枝は各ノードで高々1つ。局面集合が全て無駄合いなら枝ごと除去する。
            // 最長枝に限らず除去する（非最長の除去は手数に影響せず、表示から無駄合い
            // 変化を消すのが目的）。除去する枝の内部には再帰しない（サブ探索予算の節約。
            // all_muda を子の再帰より先に呼ぶため、予算消費前で判定が通りやすい）。
            let mut folded: Vec<SolutionBranch> = Vec::with_capacity(branches.len());
            for b in branches {
                match &b.observation {
                    Observation::Checkmate => folded.push(b.clone()),
                    Observation::Illegal => {
                        if all_muda(&illegal_before) {
                            continue; // 無駄合いにつき畳み込み
                        }
                        folded.push(SolutionBranch {
                            observation: b.observation.clone(),
                            sente_hand: b.sente_hand,
                            continuation: Box::new(mfold(&b.continuation, &illegal_before)),
                        });
                    }
                    obs => {
                        let sub = group_union(&groups, obs);
                        folded.push(SolutionBranch {
                            observation: b.observation.clone(),
                            sente_hand: b.sente_hand,
                            continuation: Box::new(mfold(&b.continuation, &sub)),
                        });
                    }
                }
            }
            // 防御: 全枝が消える手（反則枝しか持たない手）は証明木に現れないはず
            // だが、万一の場合は畳み込まずそのまま残す。
            if folded.is_empty() {
                return node.clone();
            }
            SolutionNode::AttackMove {
                mv: mv.clone(),
                branches: folded,
                meta_hash: *meta_hash,
            }
        }
    }
}

/// AND ノード展開キャッシュ1世代あたりの上限件数。
///
/// 1件が情報集合まるごと（観測分岐ごとの MetaPosition）を抱えるため、上限なしだと
/// GB 級に膨らみ、ヒットで得られる分よりメモリ帯域で失う分が上回る。df-pn は
/// 同じ AND ノードを短い周期で再訪するので、小さな作業セットがあれば十分。
/// 実測（挑戦モードの重い問題）では上限なしより 2〜3 倍速い。
///
/// 上限に達したら現世代を旧世代に落として新しい世代を始める（2世代方式）。
/// 一度に全部捨てると、作業セットが上限をわずかに超える問題でヒット率が
/// 崖のように落ちるため。旧世代のヒットは新世代に昇格するので、
/// 実際に使われているエントリは世代交代を生き延びる。
/// 再計算可能なキャッシュなので、どちらにせよ探索結果には影響しない。
const MAX_AND_EXPANSION_CACHE: usize = 5_000;

/// `solve_bounded` に 0（auto）を渡したときの最小証明深さスキャンのノード予算。
///
/// スキャンは玉方のタイブレーク（どの観測分岐が一番粘れるか）用の精密化で、
/// 詰む・詰まないの判定には影響しない。予算を使い切っても返す深さは
/// 「証明済みの上界」のままなので、打ち切りで壊れるのは精度だけ。
///
/// 本解の証明ノード数に比例させる案も試したが、応答時間はこの固定値と
/// ほぼ同じで（実測: 重い18問で 28.96s 対 28.96s、1手最大は固定値の方が速い）、
/// 予算が転置表の温まり具合で変わるぶん結果が読みにくくなるので固定にした。
pub const SCAN_NODES_AUTO: u64 = 50_000;

/// 衝立df-pnの探索結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsuitateDfpnResult {
    /// 詰みが証明された（攻め方勝ち）
    Proven,
    /// 不詰が証明された（玉方勝ち）
    Disproven,
    /// 探索が打ち切られた（ノード上限 or キャンセル）
    Unknown,
}

/// 転置表エントリ
///
/// budget は「エントリ確定時の残り深さ予算」（depth_limit - 遭遇深さ、制限なしは INF）。
/// 深さ制限付き探索の結果は予算に依存するため、再利用時に予算でゲートする:
/// - 証明 (pn=0): 「証明深さ k 以内で詰む」→ 現在の予算 >= k なら再利用可
/// - 反証 (dn=0): 「予算 budget 以内では詰まない」→ 現在の予算 <= budget なら再利用可
/// - 中間値: ヒューリスティックなのでそのまま再利用可
///
/// proof_k は「このノードは k 手以内で詰む」という恒久的な証明の記録。
/// 深さ制限付きプローブの再探索で pn/dn が反証で上書きされても、
/// 一度得られた証明（恒久的な真実）は proof_k として保持され続ける。
#[derive(Debug, Clone, Copy)]
struct PnDn {
    pn: u32,
    dn: u32,
    budget: u32,
    /// 証明深さの最小値（未証明は u32::MAX）
    proof_k: u32,
}

impl PnDn {
    /// 現在の残り深さ予算での実効値。再利用できない場合は None。
    fn effective(&self, budget_now: u32) -> Option<(u32, u32)> {
        if self.proof_k <= budget_now {
            return Some((0, INF)); // 予算内で証明済み
        }
        if self.pn == 0 {
            return None; // 証明はあるが現在の予算では不足 → 未知扱い
        }
        if self.dn == 0 {
            // 反証は「予算 budget 以内では詰まない」→ より小さい予算でのみ有効
            return if budget_now <= self.budget {
                Some((self.pn, self.dn))
            } else {
                None
            };
        }
        Some((self.pn, self.dn))
    }

    /// 証明されている場合、その証明深さ k を返す
    fn proven_bound(&self) -> Option<u32> {
        if self.proof_k != u32::MAX {
            Some(self.proof_k)
        } else {
            None
        }
    }
}

/// コンパクトな証明木ノード（リプレイキャッシュ用）
/// SolutionNode と異なり String を持たず、Move を直接格納
#[derive(Debug, Clone, PartialEq)]
enum ProofNode {
    Checkmate,
    Attack {
        mv: Move,
        branches: Vec<(ProofObs, ProofNode)>,
    },
}

impl ProofNode {
    /// 証明木の深さ（mate-in-k の k、深さ制限の予算と同じ単位）
    fn proof_depth(&self) -> u32 {
        match self {
            ProofNode::Checkmate => 0,
            ProofNode::Attack { branches, .. } => {
                let mut k: u32 = 1;
                for (obs, child) in branches {
                    let contrib = match obs {
                        ProofObs::Checkmate => 1,
                        ProofObs::Illegal => child.proof_depth(),
                        ProofObs::Captured(..) | ProofObs::NoCapture => {
                            child.proof_depth().saturating_add(2)
                        }
                    };
                    k = k.max(contrib);
                }
                k
            }
        }
    }
}

/// 証明木の観測タイプ（Observation の軽量版）
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProofObs {
    Checkmate,
    Captured(u8, u8),
    NoCapture,
    Illegal,
}

impl ProofObs {
    fn matches(&self, obs: &Observation) -> bool {
        match (self, obs) {
            (ProofObs::Checkmate, Observation::Checkmate) => true,
            (ProofObs::Captured(f, r), Observation::Captured { file, rank }) => f == file && r == rank,
            (ProofObs::NoCapture, Observation::NoCapture) => true,
            (ProofObs::Illegal, Observation::Illegal) => true,
            _ => false,
        }
    }
}

fn meta_position_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|p| p.zobrist_hash)
        .collect();
    hashes.sort();
    let mut h = FxHasher::default();
    hashes.hash(&mut h);
    h.finish()
}

/// MetaPosition の盤面セットハッシュ（sente_hand を除外）
/// 優越関係の判定用: 同じ盤面配置で sente_hand のみ異なるメタポジションを同一グループにまとめる
/// ソートしてから結合することで XOR より衝突に強いハッシュを生成
fn meta_board_set_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|pos| pos.hash_without_sente_hand())
        .collect();
    hashes.sort();
    let mut h = FxHasher::default();
    hashes.hash(&mut h);
    h.finish()
}

/// MetaPosition の盤面のみのハッシュ（両方の持ち駒を除外）
/// 手順ヒント用: 盤面配置が同じメタポジション間で証明手順を共有する
fn meta_board_only_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|pos| pos.hash_board_only())
        .collect();
    hashes.sort();
    let mut h = FxHasher::default();
    hashes.hash(&mut h);
    h.finish()
}

// ========================================================================
// 王手候補の幾何学的事前フィルタ
// ========================================================================

/// 駒種が to から target を大まかに攻撃可能か（遮蔽物無視）
/// 偽陰性なし（王手になりうる手を漏らさない）、偽陽性あり
fn can_attack_rough(kind: PieceKind, to: Square, target: Square, color: Color) -> bool {
    let df = target.file as i8 - to.file as i8;
    let dr = target.rank as i8 - to.rank as i8;
    if df == 0 && dr == 0 {
        return false;
    }
    match kind {
        PieceKind::King => df.abs() <= 1 && dr.abs() <= 1,
        PieceKind::Gold | PieceKind::PromotedSilver | PieceKind::PromotedKnight
        | PieceKind::PromotedLance | PieceKind::PromotedPawn => {
            // 金の動き（色依存）
            if df.abs() <= 1 && dr.abs() <= 1 {
                if color == Color::Sente {
                    // 後ろ斜めは不可: (-1,1),(1,1) が不可
                    !(dr == 1 && df.abs() == 1)
                } else {
                    // 前斜めは不可: (-1,-1),(1,-1) が不可
                    !(dr == -1 && df.abs() == 1)
                }
            } else {
                false
            }
        }
        PieceKind::Silver => {
            if df.abs() <= 1 && dr.abs() <= 1 {
                if color == Color::Sente {
                    // 銀: 前3 + 後斜め2 = (-1,-1),(0,-1),(1,-1),(-1,1),(1,1)
                    !(df == 0 && dr == 1) && !(df.abs() == 1 && dr == 0)
                } else {
                    !(df == 0 && dr == -1) && !(df.abs() == 1 && dr == 0)
                }
            } else {
                false
            }
        }
        PieceKind::Knight => {
            if color == Color::Sente {
                (df == -1 || df == 1) && dr == -2
            } else {
                (df == -1 || df == 1) && dr == 2
            }
        }
        PieceKind::Pawn => {
            if color == Color::Sente {
                df == 0 && dr == -1
            } else {
                df == 0 && dr == 1
            }
        }
        PieceKind::Lance => {
            // 同筋で前方向のみ（遮蔽物無視）
            if df != 0 {
                return false;
            }
            if color == Color::Sente { dr < 0 } else { dr > 0 }
        }
        PieceKind::Rook => {
            // 十字方向（遮蔽物無視）
            df == 0 || dr == 0
        }
        PieceKind::Bishop => {
            // 斜め方向（遮蔽物無視）
            df.abs() == dr.abs()
        }
        PieceKind::PromotedRook => {
            // 竜: 十字スライド + 斜め1マス
            df == 0 || dr == 0 || (df.abs() <= 1 && dr.abs() <= 1)
        }
        PieceKind::PromotedBishop => {
            // 馬: 斜めスライド + 十字1マス
            df.abs() == dr.abs() || (df.abs() <= 1 && dr.abs() <= 1)
        }
    }
}

/// ある方向にスライドできる攻め方の駒種か
fn is_slider_for_direction(kind: PieceKind, df: i8, dr: i8, color: Color) -> bool {
    match kind {
        PieceKind::Rook | PieceKind::PromotedRook => df == 0 || dr == 0,
        PieceKind::Bishop | PieceKind::PromotedBishop => df.abs() == dr.abs() && df != 0,
        PieceKind::Lance => {
            df == 0 && if color == Color::Sente { dr < 0 } else { dr > 0 }
        }
        _ => false,
    }
}

/// 開き王手の候補マスを計算
/// king_sq から8方向をスキャンし、攻め方の駒の後ろにスライダーがある場合、
/// 手前の駒のマスを discovery_square として返す
fn compute_discovery_squares(pos: &Position, king_sq: Square, attacker: Color) -> Vec<Square> {
    let directions: [(i8, i8); 8] = [
        (0, -1), (0, 1), (-1, 0), (1, 0),
        (-1, -1), (-1, 1), (1, -1), (1, 1),
    ];
    let mut result = Vec::new();

    for &(df, dr) in &directions {
        let mut f = king_sq.file as i8 + df;
        let mut r = king_sq.rank as i8 + dr;
        let mut first_piece_sq: Option<Square> = None;

        while Square::is_valid(f, r) {
            let sq = Square::new(f as u8, r as u8);
            if let Some(piece) = pos.piece_at(sq) {
                if piece.color == attacker {
                    if first_piece_sq.is_none() {
                        // 最初に見つけた攻め方の駒 → 開き王手の候補
                        first_piece_sq = Some(sq);
                    } else {
                        // 2番目の攻め方の駒 → スライダーならdiscovery確定
                        if is_slider_for_direction(piece.kind, -df, -dr, attacker) {
                            result.push(first_piece_sq.unwrap());
                        }
                        break;
                    }
                } else {
                    // 相手の駒に当たった → このラインは終了
                    break;
                }
            }
            f += df;
            r += dr;
        }
    }

    result
}

/// 「color の kind をどのマスに打てば ksq に利くか」のマスク。
///
/// `can_attack_rough` と同じ判定（遮蔽物無視）を全マス分だけ先に畳んだもの。
/// 打ちは駒を退かさないので開き王手にはならず、王手になる打ちは必ず
/// このマスクの中にある（偽陰性なし）。初回アクセス時に一度だけ構築する。
/// サイズは 2(色) × 7(持ち駒種) × 81(玉の位置) = 1134 個のビットボード。
fn drop_check_mask(color: Color, kind: PieceKind, ksq: Square) -> Bitboard {
    static MASKS: std::sync::OnceLock<Vec<Bitboard>> = std::sync::OnceLock::new();
    let masks = MASKS.get_or_init(|| {
        let mut v = vec![0 as Bitboard; 2 * 7 * 81];
        for (ci, c) in [Color::Sente, Color::Gote].into_iter().enumerate() {
            for (ki, k) in PieceKind::HAND_PIECES.into_iter().enumerate() {
                for k_idx in 0..81 {
                    let king_sq = Square::from_index(k_idx);
                    let mut mask: Bitboard = 0;
                    for s_idx in 0..81 {
                        let sq = Square::from_index(s_idx);
                        if can_attack_rough(k, sq, king_sq, c) {
                            mask |= 1u128 << s_idx;
                        }
                    }
                    v[(ci * 7 + ki) * 81 + k_idx] = mask;
                }
            }
        }
        v
    });
    let ci = match color {
        Color::Sente => 0,
        Color::Gote => 1,
    };
    let Some(ki) = PieceKind::HAND_PIECES.iter().position(|&k| k == kind) else {
        return 0;
    };
    masks[(ci * 7 + ki) * 81 + ksq.index()]
}

/// 手 mv が king_sq に王手を与えうるかの粗判定（偽陰性なし）
fn could_give_check(mv: &Move, king_sq: Square, discovery_sqs: &[Square], attacker: Color) -> bool {
    // A. 直接王手の可能性
    let piece_kind_after = if let Some(drop_kind) = mv.drop_piece {
        drop_kind
    } else if let Some(moved_kind) = mv.moved_piece_kind {
        if mv.promotion {
            moved_kind.promoted().unwrap_or(moved_kind)
        } else {
            moved_kind
        }
    } else {
        return true; // 安全側に倒す
    };

    if can_attack_rough(piece_kind_after, mv.to, king_sq, attacker) {
        return true;
    }

    // B. 開き王手の可能性（盤上の駒移動のみ）
    if let Some(from) = mv.from {
        if discovery_sqs.contains(&from) {
            // 移動先が同じラインに留まるかチェック
            let df_king = king_sq.file as i8 - from.file as i8;
            let dr_king = king_sq.rank as i8 - from.rank as i8;
            let df_to = mv.to.file as i8 - from.file as i8;
            let dr_to = mv.to.rank as i8 - from.rank as i8;

            // 同じライン上に留まるかの判定
            // ライン方向を正規化して比較
            let stays_on_line = if df_king == 0 {
                df_to == 0
            } else if dr_king == 0 {
                dr_to == 0
            } else {
                // 斜め方向: df_to/dr_to が df_king/dr_king と同じ方向比
                df_to != 0 && dr_to != 0
                    && df_king.signum() * dr_to == dr_king.signum() * df_to
            };

            if !stays_on_line {
                return true;
            }
        }
    }

    false
}

/// OR ノードの候補手生成結果キャッシュ
/// mid_or が同一 OR ノードを再訪問する際の generate_attack_candidates 再計算を回避
struct CachedOrCandidates {
    candidates: Vec<Move>,
}

/// AND ノードの展開結果キャッシュ
/// mid_and が同一 AND ノードを再訪問する際の expand_defense_moves 再計算を回避
#[derive(Clone)]
struct CachedAndExpansion {
    obs_terminal: Vec<bool>,
    obs_hash: Vec<u64>,
    obs_metas: Vec<MetaPosition>,
    obs_depth_inc: Vec<u32>,
    obs_hands: Vec<[u8; 7]>,
    parent_hand: [u8; 7],
}

/// 衝立詰将棋 df-pn ソルバー
///
/// 通常の df-pn が Position で動くのに対し、MetaPosition で動く。
/// - OR ノード: MetaPosition（攻め方が手を選ぶ）
/// - AND ノード: 観測分岐（Checkmate / Captured / NoCapture / Illegal、最大4分岐）
///
/// AND ノードの分岐数が最大4であるため、通常の詰将棋の AND ノード
/// （王手回避手＝数十手）に比べて大幅に効率的。
pub struct TsuitateDfpnSolver {
    /// OR ノードの転置表: MetaPosition ハッシュ → (pn, dn)
    or_table: FxHashMap<u64, PnDn>,
    /// AND ノードの転置表: hash(meta_hash, move) → (pn, dn)
    and_table: FxHashMap<u64, PnDn>,
    /// 探索ノード数
    pub nodes_searched: u64,
    /// 優越関係ヒット数（診断用）
    pub dominance_hits: u64,
    /// ノード数上限
    node_limit: u64,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
    /// ルートで除外する手（余詰めチェック用）
    excluded_root_moves: Vec<ExclusionKey>,
    /// ルートのメタポジションハッシュ（除外判定用）
    root_hash: Option<u64>,
    /// 深さ上限（None = 制限なし）
    depth_limit: Option<u32>,
    /// 優越関係テーブル: board_set_hash → Vec<(proof_hand, budget)> (Pareto最小)
    /// proof_hand <= sente_hand (要素ごと) かつ 現在の残り深さ予算 >= budget で証明済み
    dominance_table: FxHashMap<u64, Vec<([u8; 7], u32)>>,
    /// OR ノードの証明駒: meta_hash → proof_hand (証明に必要な最小の攻め方持ち駒)
    or_proof_hands: FxHashMap<u64, [u8; 7]>,
    /// AND ノードの証明駒: and_key → proof_hand
    and_proof_hands: FxHashMap<u64, [u8; 7]>,
    /// 盤面のみのハッシュ → 証明された手（手順ヒント用）
    move_hints: FxHashMap<u64, Move>,
    /// 盤面のみのハッシュ → (証明深さ, 証明木) リスト（リプレイキャッシュ、複数手順対応）
    /// 証明深さは深さ制限付きプローブでのリプレイ可否の事前判定に使う
    proof_cache: FxHashMap<u64, Vec<(u32, ProofNode)>>,
    /// AND ノードの展開結果キャッシュ（現世代）: and_key → 構造データ
    /// mid_and が同一 AND ノードを再訪問する際の expand_defense_moves 再計算を回避
    and_expansion_cache: FxHashMap<u64, CachedAndExpansion>,
    /// AND ノードの展開結果キャッシュ（旧世代）。現世代が
    /// MAX_AND_EXPANSION_CACHE に達したときの退避先。ヒットしたら現世代に昇格する
    and_expansion_cache_old: FxHashMap<u64, CachedAndExpansion>,
    /// OR ノードの候補手キャッシュ: meta_hash → 候補手
    /// mid_or が同一 OR ノードを再訪問する際の generate_attack_candidates 再計算を回避
    or_candidate_cache: FxHashMap<u64, CachedOrCandidates>,
    /// 局面レベルの王手手キャッシュ: zobrist_hash → 王手になる手のリスト
    /// 同一局面が異なるMetaPositionに出現する場合の重複計算を回避
    check_moves_cache: FxHashMap<u64, Vec<Move>>,
    /// 診断用カウンタ
    pub proof_replay_attempts: u64,
    pub proof_replay_full_success: u64,
    pub proof_replay_partial: u64,
    pub proof_replay_fail: u64,
    pub expand_defense_calls: u64,
    pub mid_and_calls: u64,
    /// 診断: 探索中に遭遇した最大 MetaPosition サイズ
    pub max_meta_size: usize,
    /// 診断: generate_attack_candidates の累計時間 (ナノ秒)
    pub gen_candidates_nanos: u64,
    /// 診断: expand_defense_moves の累計時間 (ナノ秒)
    pub expand_defense_nanos: u64,
    /// 診断: all_effectively_checkmate の累計時間 (ナノ秒)
    pub aec_nanos: u64,
    /// 診断: generate_attack_candidates の非キャッシュ呼出回数
    pub gen_candidates_calls: u64,
    /// 診断: generate_attack_candidates で処理した局面の合計数
    pub gen_candidates_total_positions: u64,
    /// 診断: check_moves_cache ヒット数
    pub check_moves_cache_hits: u64,
    /// 診断: check_moves_cache ミス数
    pub check_moves_cache_misses: u64,
    /// 診断: try_replay_proof の累計時間 (ナノ秒)
    pub replay_nanos: u64,
    /// 優越関係チェックの抑制フラグ（証明の実体化再探索中に使用）
    /// 優越関係で証明されたノードは子の証明が転置表に残らず抽出できないため、
    /// 実体化のための再探索では優越関係による即時証明を無効にする
    dominance_suppressed: bool,
    /// 余詰めチェックの除外探索が打ち切られた（ノード上限・時間切れ）
    /// true の場合「余詰めなし」は確定ではなく判定不能として報告する
    second_search_aborted: bool,
    /// 余詰めチェックの除外探索1回あたりのノード上限（find_second_solution で設定）
    second_probe_cap: u64,
    /// 無駄合いを省くか（後処理で muda_folded_tree により表示木と報告手数を補正）。
    /// 最短探索とペアで使うのが正しい。既定 false。
    omit_futile: bool,
    /// 反証シングルトンレジストリ: 局面 zobrist → {p} が反証されたときの残り予算の最大値。
    /// 情報集合の単調性による反証の伝播: 攻め方の情報が増えて不利になることはないため、
    /// 単独局面 {p}（完全情報）で詰みがなければ、p を含む任意の情報集合でも詰みはない。
    /// 予算ゲートは転置表の反証と同じ向き（budget_now <= 記録予算 で再利用可）。
    /// 除外探索のルート（除外条件付きの反証）は記録しない。
    pos_disproven: FxHashMap<u64, u32>,
    /// 診断: シングルトン反証の記録数
    pub singleton_disproof_stores: u64,
    /// 診断: シングルトン反証による OR ノード即時反証の回数
    pub singleton_disproof_hits: u64,
}

impl TsuitateDfpnSolver {
    pub fn new(node_limit: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            or_table: FxHashMap::default(),
            and_table: FxHashMap::default(),
            nodes_searched: 0,
            dominance_hits: 0,
            node_limit,
            cancelled,
            excluded_root_moves: Vec::new(),
            root_hash: None,
            depth_limit: None,
            dominance_table: FxHashMap::default(),
            or_proof_hands: FxHashMap::default(),
            and_proof_hands: FxHashMap::default(),
            move_hints: FxHashMap::default(),
            proof_cache: FxHashMap::default(),
            and_expansion_cache: FxHashMap::default(),
            and_expansion_cache_old: FxHashMap::default(),
            or_candidate_cache: FxHashMap::default(),
            check_moves_cache: FxHashMap::default(),
            proof_replay_attempts: 0,
            proof_replay_full_success: 0,
            proof_replay_partial: 0,
            proof_replay_fail: 0,
            expand_defense_calls: 0,
            mid_and_calls: 0,
            max_meta_size: 0,
            gen_candidates_nanos: 0,
            expand_defense_nanos: 0,
            aec_nanos: 0,
            gen_candidates_calls: 0,
            gen_candidates_total_positions: 0,
            check_moves_cache_hits: 0,
            check_moves_cache_misses: 0,
            replay_nanos: 0,
            dominance_suppressed: false,
            second_search_aborted: false,
            second_probe_cap: u64::MAX,
            omit_futile: false,
            pos_disproven: FxHashMap::default(),
            singleton_disproof_stores: 0,
            singleton_disproof_hits: 0,
        }
    }

    /// 無駄合いを省いた手数を報告するか設定する（最短探索とペアで使う）
    pub fn set_omit_futile(&mut self, v: bool) {
        self.omit_futile = v;
    }

    /// ノード上限を再設定する（常駐モードでクエリごとの予算を
    /// nodes_searched 起点で適用するため）
    pub fn set_node_limit(&mut self, node_limit: u64) {
        self.node_limit = node_limit;
    }

    /// 探索の再開用キャッシュ（メモリの大半を占める）を解放し、
    /// 証明・反証の事実（or/and 転置表・優越関係・証明駒）だけを残す。
    ///
    /// 常駐モード（--solve-meta-server）でリクエストの合間に呼ぶ。
    /// 転置表エントリは予算ゲート（PnDn::effective）で再利用可否を判定する
    /// 恒久的事実なので、後続クエリの warm start に安全に使える。
    /// 解放するのは再計算可能なキャッシュのみで、探索結果には影響しない
    pub fn trim_transient_caches(&mut self) {
        self.proof_cache = FxHashMap::default();
        self.and_expansion_cache = FxHashMap::default();
        self.and_expansion_cache_old = FxHashMap::default();
        self.or_candidate_cache = FxHashMap::default();
        self.check_moves_cache = FxHashMap::default();
    }

    /// メタポジションが詰むかどうかを判定する
    pub fn solve(&mut self, meta: &MetaPosition) -> TsuitateDfpnResult {
        let hash = meta_position_hash(meta);
        self.root_hash = Some(hash);
        let mut path = vec![hash];

        self.mid_or(meta, hash, INF, INF, &mut path, 0);

        let budget = self.current_budget(0);
        let (pn, dn) = self
            .or_table
            .get(&hash)
            .and_then(|e| e.effective(budget))
            .unwrap_or((1, 1));
        if pn == 0 {
            TsuitateDfpnResult::Proven
        } else if dn == 0 {
            TsuitateDfpnResult::Disproven
        } else {
            TsuitateDfpnResult::Unknown
        }
    }

    /// 深さ制限付きの詰み判定（挑戦モードの応手・観測分岐の選択用）。
    ///
    /// depth_limit 手（先手・後手合計）以内に攻め方が詰ませられるかを判定する。
    /// Proven の場合、「証明できた最小の深さ制限」（最短詰み手数の上界）を
    /// 下方向スキャン（shorten_solution と同じ proof_k ジャンプ）で求めて返す。
    /// ノード上限は全スキャンの合計に適用される。タイムアウトを併用しなければ
    /// 同一入力に対して決定的。
    ///
    /// scan_node_limit は下方向スキャン専用の追加ノード予算。スキャンは
    /// タイブレーク（最長抵抗の選択）用の精密化であり真偽判定には影響しない
    /// ため、予算を使い切ったら「そこまでに証明できた深さ」を返して打ち切る
    /// （ノード数ベースなので決定的）。u64::MAX で無制限。
    ///
    /// **0 を渡すと自動**（`SCAN_NODES_AUTO` ノード）。スキャンの最後の1回は
    /// 必ず「これ以上短くはできない」ことの反証になり、証明より遥かに重く
    /// なりやすい。実測では挑戦モードの1手の応答時間の 5〜8 割をこの反証が
    /// 占めていた。打ち切ってもタイブレークが粗くなるだけで、返す深さは
    /// 常に「証明済みの上界」のままなので正しい。
    ///
    /// なお予算内でスキャンがどこまで進めるかは転置表の温まり具合に依存する
    /// （常駐モードでは単発実行より深く詰められることがある）。予算自体を
    /// 固定値にしてあるのは、そこに探索コスト依存の変動を重ねないため。
    pub fn solve_bounded(
        &mut self,
        meta: &MetaPosition,
        depth_limit: u32,
        scan_node_limit: u64,
    ) -> (TsuitateDfpnResult, Option<u32>) {
        let root_hash = meta_position_hash(meta);
        let debug_timing = std::env::var("META_SOLVE_TIMING").is_ok();
        let t0 = std::time::Instant::now();
        self.depth_limit = Some(depth_limit);
        let result = self.solve(meta);
        if debug_timing {
            eprintln!(
                "[timing] first solve d{}: {:?} nodes={}",
                depth_limit,
                t0.elapsed(),
                self.nodes_searched
            );
        }
        // 0 = 自動（固定予算 SCAN_NODES_AUTO）
        let scan_node_limit = if scan_node_limit == 0 { SCAN_NODES_AUTO } else { scan_node_limit };
        let mut proven_limit: Option<u32> = None;
        if result == TsuitateDfpnResult::Proven {
            // スキャン中だけノード上限を「現在ノード + スキャン予算」に絞る。
            // 予算に達したイテレーションは Unknown で返り、ループが終了する
            let full_node_limit = self.node_limit;
            self.node_limit = full_node_limit
                .min(self.nodes_searched.saturating_add(scan_node_limit));
            let mut limit = self
                .or_table
                .get(&root_hash)
                .and_then(|e| e.proven_bound())
                .unwrap_or(depth_limit)
                .min(depth_limit)
                .max(1);
            proven_limit = Some(limit);
            while limit > 2 && !self.should_stop() {
                let target = limit - 2;
                self.depth_limit = Some(target);
                let ti = std::time::Instant::now();
                let r = self.solve(meta);
                if debug_timing {
                    eprintln!(
                        "[timing] scan d{}: {:?} nodes={} → {:?}",
                        target,
                        ti.elapsed(),
                        self.nodes_searched,
                        r
                    );
                }
                if r != TsuitateDfpnResult::Proven {
                    break;
                }
                limit = self
                    .or_table
                    .get(&root_hash)
                    .and_then(|e| e.proven_bound())
                    .unwrap_or(target)
                    .min(target)
                    .max(1);
                proven_limit = Some(limit);
            }
            self.node_limit = full_node_limit;
        }
        self.depth_limit = None;
        (result, proven_limit)
    }

    /// 深さ depth における残り深さ予算（深さ制限なしは INF）
    fn current_budget(&self, depth: u32) -> u32 {
        match self.depth_limit {
            Some(limit) => limit.saturating_sub(depth),
            None => INF,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn should_stop(&self) -> bool {
        self.nodes_searched >= self.node_limit || self.is_cancelled()
    }

    fn and_key(meta_hash: u64, mv: &Move) -> u64 {
        let mut h = FxHasher::default();
        h.write_u64(meta_hash);
        mv.hash(&mut h);
        h.finish()
    }

    /// or_table への書き込み（既存の proof_k を保持し、証明なら更新）
    fn store_or(&mut self, hash: u64, pn: u32, dn: u32, budget: u32) {
        let e = self.or_table.entry(hash).or_insert(PnDn {
            pn: 1,
            dn: 1,
            budget: 0,
            proof_k: u32::MAX,
        });
        if pn == 0 {
            e.proof_k = e.proof_k.min(budget);
        }
        e.pn = pn;
        e.dn = dn;
        e.budget = budget;
    }

    /// and_table への書き込み（既存の proof_k を保持し、証明なら更新）
    fn store_and(&mut self, key: u64, pn: u32, dn: u32, budget: u32) {
        let e = self.and_table.entry(key).or_insert(PnDn {
            pn: 1,
            dn: 1,
            budget: 0,
            proof_k: u32::MAX,
        });
        if pn == 0 {
            e.proof_k = e.proof_k.min(budget);
        }
        e.pn = pn;
        e.dn = dn;
        e.budget = budget;
    }

    /// ルートで除外されている手かどうか（余詰めチェック用）
    fn is_excluded_root_move(&self, mv: &Move) -> bool {
        self.excluded_root_moves.iter().any(|exc| exc.matches(mv))
    }

    /// OR ノード（攻め方）: 候補手から1つでも詰みに導ければ成功
    ///
    /// pn = min{pn_c}, dn = sum{dn_c}
    ///
    /// hash は meta_position_hash(meta) の事前計算値（呼び出し元が既に持って
    /// いるため再計算を避ける。巨大メタポジションではハッシュ計算が重い）
    fn mid_or(
        &mut self,
        meta: &MetaPosition,
        hash: u64,
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
        depth: u32,
    ) {
        self.nodes_searched += 1;
        let budget_now = self.current_budget(depth);

        // MetaPosition サイズ記録
        if meta.positions.len() > self.max_meta_size {
            self.max_meta_size = meta.positions.len();
        }

        // 終端チェック
        if meta.is_empty() {
            // 深さ非依存の反証 → どの予算でも再利用可
            self.store_or(hash, INF, 0, INF);
            return;
        }
        let aec_start = std::time::Instant::now();
        let is_aec = meta.all_effectively_checkmate();
        self.aec_nanos += aec_start.elapsed().as_nanos() as u64;
        if is_aec {
            // 即詰み → 予算0で証明（どの予算でも再利用可）
            self.store_or(hash, 0, INF, 0);
            let proof_hand = [0u8; 7];
            self.or_proof_hands.insert(hash, proof_hand);
            if !meta.positions.is_empty() {
                let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
                let board_hash = meta_board_set_hash(meta);
                self.add_dominance_entry(board_hash, sente_hand, 0);
            }
            return;
        }
        if meta.positions.len() > MAX_META_POSITIONS {
            self.store_or(hash, INF, 0, INF);
            return;
        }

        // 深さ制限チェック
        if let Some(limit) = self.depth_limit {
            if depth >= limit {
                // 予算0での反証（より浅い遭遇では再利用されない）
                self.store_or(hash, INF, 0, 0);
                return;
            }
        }

        // ルートで手が除外されている場合（余詰めチェック）、除外手を使った
        // 証明由来の優越関係・証明木リプレイで早期終了しないようスキップする
        let exclusions_at_root =
            self.root_hash == Some(hash) && !self.excluded_root_moves.is_empty();

        // シングルトン反証の伝播: 情報集合内に「単独でも詰まない」ことが確定した
        // 局面が含まれていれば、この情報集合にも詰みはない（情報の単調性）。
        // 反証の予算ゲートは転置表と同じ向き（budget_now <= 記録予算）
        if !exclusions_at_root && !self.dominance_suppressed && !self.pos_disproven.is_empty() {
            let mut matched_budget: Option<u32> = None;
            for pos in &meta.positions {
                if let Some(&b) = self.pos_disproven.get(&pos.zobrist_hash) {
                    if budget_now <= b {
                        matched_budget = Some(b);
                        break;
                    }
                }
            }
            if let Some(b) = matched_budget {
                self.store_or(hash, INF, 0, b);
                self.singleton_disproof_hits += 1;
                return;
            }
        }

        // 優越関係チェック: proof_hand <= sente_hand (要素ごと) かつ予算が足りる場合
        // 同じ盤面セット + 同じ gote_hand で、sente_hand が proof_hand 以上なら証明済み
        // （実体化再探索中は抑制: 子の証明を転置表に残すため）
        if !exclusions_at_root && !self.dominance_suppressed && !meta.positions.is_empty() {
            let board_hash = meta_board_set_hash(meta);
            let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
            let matched = self.dominance_table.get(&board_hash).and_then(|entries| {
                entries
                    .iter()
                    .find(|(ph, b)| {
                        *b <= budget_now
                            && ph.iter().zip(sente_hand.iter()).all(|(p, s)| p <= s)
                    })
                    .copied()
            });
            if let Some((matched_ph, matched_budget)) = matched {
                self.store_or(hash, 0, INF, matched_budget);
                self.or_proof_hands.insert(hash, matched_ph);
                self.dominance_hits += 1;
                return;
            }
        }

        // 証明木リプレイ: board-only hash でキャッシュされた証明木を試す（複数候補対応）
        // 証明深さが現在の残り予算を超える証明はリプレイしても必ず失敗するため
        // 事前に除外する（深さ制限付きプローブでのリプレイ嵐を防ぐ）
        if !exclusions_at_root && !meta.positions.is_empty() {
            let board_only_hash = meta_board_only_hash(meta);
            let proofs: Vec<ProofNode> = self
                .proof_cache
                .get(&board_only_hash)
                .map(|entry| {
                    entry
                        .iter()
                        .filter(|(pd, _)| *pd <= budget_now)
                        .map(|(_, p)| p.clone())
                        .collect()
                })
                .unwrap_or_default();
            if !proofs.is_empty() {
                self.proof_replay_attempts += 1;
                let or_table_size_before = self.or_table.len();
                let mut replayed = false;
                let replay_start = std::time::Instant::now();
                for proof in &proofs {
                    if let Some(_or_ph) = self.try_replay_proof(meta, proof, hash, path, depth) {
                        self.proof_replay_full_success += 1;
                        replayed = true;
                        break;
                    }
                }
                self.replay_nanos += replay_start.elapsed().as_nanos() as u64;
                if !replayed {
                    if self.or_table.len() > or_table_size_before {
                        self.proof_replay_partial += 1;
                    } else {
                        self.proof_replay_fail += 1;
                    }
                } else {
                    return;
                }
            }
        }

        // 候補手を生成（キャッシュ活用、remove/re-insert パターン）
        let candidates_raw = if let Some(cached) = self.or_candidate_cache.remove(&hash) {
            cached.candidates
        } else {
            let gen_start = std::time::Instant::now();
            let candidates = self.generate_attack_candidates(meta);
            self.gen_candidates_nanos += gen_start.elapsed().as_nanos() as u64;
            candidates
        };
        if self.should_stop() {
            self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw });
            return;
        }
        let mut candidates = candidates_raw.clone();

        // ルートノードでは除外手をフィルタ
        if self.root_hash == Some(hash) && !self.excluded_root_moves.is_empty() {
            candidates.retain(|mv| !self.is_excluded_root_move(mv));
        }

        if candidates.is_empty() {
            self.store_or(hash, INF, 0, INF);
            // 王手候補なし＝どの予算でも詰まない恒久的な反証。シングルトンなら伝播用に記録
            // （除外探索のルートは除外条件付きの反証のため記録しない）
            if !exclusions_at_root && meta.positions.len() == 1 {
                let e = self.pos_disproven.entry(meta.positions[0].zobrist_hash).or_insert(0);
                *e = (*e).max(INF);
                self.singleton_disproof_stores += 1;
            }
            self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw });
            return;
        }

        // 手順ヒントによる候補手の並べ替え
        if !meta.positions.is_empty() {
            let board_only_hash = meta_board_only_hash(meta);
            if let Some(hint_mv) = self.move_hints.get(&board_only_hash).copied() {
                if let Some(pos) = candidates.iter().position(|mv| *mv == hint_mv) {
                    if pos > 0 {
                        let mv = candidates.remove(pos);
                        candidates.insert(0, mv);
                    }
                }
            }
        }

        // 子ノード（AND）の初期化（予算ゲートを通ったエントリのみ再利用）
        let mut children: Vec<(Move, u64, u32, u32)> = Vec::with_capacity(candidates.len());
        for mv in &candidates {
            let ak = Self::and_key(hash, mv);
            let (cpn, cdn) = self
                .and_table
                .get(&ak)
                .and_then(|e| e.effective(budget_now))
                .unwrap_or((1, 1));
            children.push((*mv, ak, cpn, cdn));
        }

        loop {
            if self.should_stop() {
                self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw });
                return;
            }

            // OR 集約: pn = min{pn_c}, dn = sum{dn_c}
            let pn_n = children.iter().map(|c| c.2).min().unwrap_or(INF);
            let dn_n = children.iter().map(|c| c.3).fold(0u32, |a, d| a.saturating_add(d));

            if pn_n == 0 || dn_n == 0 {
                // 証明時は budget に実際の証明深さ k（k手以内で詰む）を記録する。
                // 探索時の残り予算ではなく k を使うことで、より小さい予算の
                // プローブでも budget_now >= k なら証明を再利用できる。
                let entry_budget = if pn_n == 0 {
                    children
                        .iter()
                        .filter(|c| c.2 == 0)
                        .map(|c| {
                            self.and_table
                                .get(&c.1)
                                .and_then(|e| e.proven_bound())
                                .unwrap_or(budget_now)
                        })
                        .min()
                        .unwrap_or(budget_now)
                } else {
                    budget_now
                };
                self.store_or(hash, pn_n, dn_n, entry_budget);
                if pn_n == 0 {
                    // 証明駒の計算: 証明された AND 子ノードの proof_hand の要素ごと最小値
                    let fallback_ph = meta.positions.first()
                        .map(|p| p.hand(Color::Sente).counts_array())
                        .unwrap_or([0; 7]);
                    let mut or_ph = [u8::MAX; 7];
                    for child in &children {
                        if child.2 == 0 {
                            let and_ph = self.and_proof_hands.get(&child.1)
                                .copied().unwrap_or(fallback_ph);
                            for j in 0..7 {
                                or_ph[j] = or_ph[j].min(and_ph[j]);
                            }
                        }
                    }
                    if or_ph[0] == u8::MAX {
                        or_ph = fallback_ph;
                    }
                    self.or_proof_hands.insert(hash, or_ph);
                    self.record_proven_dominance(meta, or_ph, entry_budget);
                    // 盤面のみのハッシュで手順ヒントを記録
                    if !meta.positions.is_empty() {
                        let board_only_hash = meta_board_only_hash(meta);
                        if let Some(proven_child) = children.iter().find(|c| c.2 == 0) {
                            self.move_hints.insert(board_only_hash, proven_child.0);
                        }
                        // 証明木をキャッシュ（リプレイ用、複数手順対応、重複排除）
                        // 深さ制限付きプローブ中はスキップ: 抽出（応手展開の再帰）が
                        // 探索本体より高コストになる。プローブ間の証明の引き継ぎは
                        // budget 付き転置表エントリが担う。
                        if self.depth_limit.is_none() {
                            let max_proofs = 20;
                            let should_add = self.proof_cache
                                .get(&board_only_hash)
                                .map_or(true, |v| v.len() < max_proofs);
                            if should_add {
                                if let Some(proof) = self.extract_compact_or(meta) {
                                    let entry = self.proof_cache.entry(board_only_hash)
                                        .or_default();
                                    if !entry.iter().any(|(_, p)| *p == proof) {
                                        entry.push((proof.proof_depth(), proof));
                                    }
                                }
                            }
                        }
                    }
                } else {
                    // dn_n == 0: 反証確定。シングルトンなら「単独局面で予算 budget_now 以内に
                    // 詰みなし」として記録し、以降この局面を含む任意の情報集合を即時反証する。
                    // 除外探索のルートは除外条件付きの反証（真の反証ではない）ため記録しない
                    if !exclusions_at_root && meta.positions.len() == 1 {
                        let e = self.pos_disproven.entry(meta.positions[0].zobrist_hash).or_insert(0);
                        *e = (*e).max(entry_budget);
                        self.singleton_disproof_stores += 1;
                    }
                }
                self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw });
                return;
            }

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.store_or(hash, pn_n, dn_n, budget_now);
                self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw });
                return;
            }

            // 最善の子（pn 最小）を選択
            let (best_idx, _) = children.iter().enumerate().min_by_key(|(_, c)| c.2).unwrap();
            let pn_2nd = children
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best_idx)
                .map(|(_, c)| c.2)
                .min()
                .unwrap_or(INF);
            let best_dn = children[best_idx].3;

            // 子のしきい値を計算（1+ε trick）
            let child_pn_limit =
                pn_limit.min(pn_2nd.saturating_add(1).saturating_add(pn_2nd / 4));
            let child_dn_limit = dn_limit.saturating_sub(dn_n).saturating_add(best_dn);

            // AND ノードに再帰
            let best_mv = children[best_idx].0;
            self.mid_and(
                meta,
                hash,
                best_mv,
                child_pn_limit,
                child_dn_limit,
                path,
                depth,
            );

            // AND テーブルから値を読み戻す（予算ゲート付き）
            let ak = children[best_idx].1;
            let (cpn, cdn) = self
                .and_table
                .get(&ak)
                .and_then(|e| e.effective(budget_now))
                .unwrap_or((1, 1));
            children[best_idx].2 = cpn;
            children[best_idx].3 = cdn;
        }
    }

    /// AND ノード（観測分岐）: 全分岐で詰めば証明
    ///
    /// pn = sum{pn_c}, dn = min{dn_c}
    /// 分岐は最大4つ: Illegal, Checkmate, Captured, NoCapture
    ///
    /// meta_hash は meta_position_hash(meta) の事前計算値
    #[allow(clippy::too_many_arguments)]
    fn mid_and(
        &mut self,
        meta: &MetaPosition,
        meta_hash: u64,
        mv: Move,
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
        depth: u32,
    ) {
        self.mid_and_calls += 1;
        let and_key = Self::and_key(meta_hash, &mv);
        let budget_now = self.current_budget(depth);

        // Phase 1: キャッシュから展開結果を取得、なければ計算
        // remove で所有権を取り、ループ後に再挿入（borrow checker 回避）
        let cached_expansion = self
            .and_expansion_cache
            .remove(&and_key)
            .or_else(|| self.and_expansion_cache_old.remove(&and_key));
        let expansion = if let Some(cached) = cached_expansion {
            cached
        } else {
            // 手を適用して合法/不正に分割
            let (legal_meta, illegal_meta) = meta.apply_attack_move_split(mv);

            if legal_meta.is_empty() && illegal_meta.is_empty() {
                // 構造的な反証（深さ非依存）→ どの予算でも再利用可
                self.store_and(and_key, INF, 0, INF);
                return;
            }

            // 全合法盤面で王手がかかっているか（衝立詰将棋: 毎手王手が必要）
            if !legal_meta.is_empty()
                && !legal_meta
                    .positions
                    .iter()
                    .all(|pos| pos.is_in_check(pos.side_to_move))
            {
                self.store_and(and_key, INF, 0, INF);
                return;
            }

            // 持ち駒取得: 全 position 同一なら正確値、混合なら成分最大値（保守的推定）
            let parent_hand = Self::parent_sente_hand(meta, [0; 7]);

            let mut obs_terminal: Vec<bool> = Vec::new();
            let mut obs_hash: Vec<u64> = Vec::new();
            let mut obs_metas: Vec<MetaPosition> = Vec::new();
            let mut obs_depth_inc: Vec<u32> = Vec::new();
            let mut obs_hands: Vec<[u8; 7]> = Vec::new();

            // Illegal 分岐
            if !illegal_meta.is_empty() {
                let bh = meta_position_hash(&illegal_meta);
                obs_terminal.push(false);
                obs_hash.push(bh);
                obs_hands.push(parent_hand);
                obs_metas.push(illegal_meta);
                obs_depth_inc.push(0);
            }

            // 合法分岐
            if !legal_meta.is_empty() {
                if legal_meta.all_effectively_checkmate() {
                    if !legal_meta.positions.is_empty() {
                        let board_hash = meta_board_set_hash(&legal_meta);
                        self.add_dominance_entry(board_hash, [0; 7], 0);
                    }
                    let legal_hand = legal_meta.positions.first()
                        .map(|p| p.hand(Color::Sente).counts_array())
                        .unwrap_or(parent_hand);
                    obs_terminal.push(true);
                    obs_hash.push(0);
                    obs_hands.push(legal_hand);
                    obs_metas.push(MetaPosition { positions: Vec::new() });
                    obs_depth_inc.push(0);
                } else {
                    self.expand_defense_calls += 1;
                    let ed_start = std::time::Instant::now();
                    let raw_branches = legal_meta.expand_defense_moves(mv);
                    self.expand_defense_nanos += ed_start.elapsed().as_nanos() as u64;

                    // キャンセル発火時、expand_defense_moves は部分結果（または空）を
                    // 返すことがある。空を「応手なし＝詰み」と誤認して偽の証明を
                    // 作らないよう、結果を捨てて打ち切る
                    if self.is_cancelled() {
                        return;
                    }

                    // 同一観測タイプの分岐をマージ
                    // sente_hand_hash で分かれた分岐を統合し、AND の分岐数を削減
                    // 異なる合駒で生じた持ち駒の違いは legal/illegal split で処理される
                    let branches = Self::merge_observation_branches(raw_branches);

                    for (obs, branch_meta) in branches {
                        match obs {
                            Observation::Checkmate => {
                                // 持ち駒の成分最小値を使用（混合対応）
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                obs_terminal.push(true);
                                obs_hash.push(0);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(0);
                            }
                            Observation::Captured { .. } | Observation::NoCapture => {
                                if branch_meta.positions.len() > MAX_META_POSITIONS {
                                    self.store_and(and_key, INF, 0, INF);
                                    return;
                                }
                                // 持ち駒の成分最小値を使用（混合対応、保守的推定）
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                let bh = meta_position_hash(&branch_meta);
                                obs_terminal.push(false);
                                obs_hash.push(bh);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(2);
                            }
                            Observation::Illegal => {}
                        }
                    }
                }
            }

            if obs_terminal.is_empty() {
                self.store_and(and_key, INF, 0, INF);
                return;
            }

            CachedAndExpansion {
                obs_terminal,
                obs_hash,
                obs_metas,
                obs_depth_inc,
                obs_hands,
                parent_hand,
            }
        };

        let n = expansion.obs_terminal.len();

        // Phase 2: 転置表から最新の pn/dn を取得（子の予算でゲート）
        let mut obs_pn: Vec<u32> = Vec::with_capacity(n);
        let mut obs_dn: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            if expansion.obs_terminal[i] {
                obs_pn.push(0);
                obs_dn.push(INF);
            } else {
                let child_budget = self.current_budget(depth + expansion.obs_depth_inc[i]);
                let (cpn, cdn) = self.lookup_or(expansion.obs_hash[i], path, child_budget);
                obs_pn.push(cpn);
                obs_dn.push(cdn);
            }
        }

        // Phase 3: メインループ（break で統一して最後にキャッシュ再挿入）
        loop {
            if self.should_stop() {
                break;
            }

            // AND 集約: pn = sum{pn_c}, dn = min{dn_c}
            let pn_n = obs_pn.iter().fold(0u32, |a, &p| a.saturating_add(p));
            let dn_n = obs_dn.iter().copied().min().unwrap_or(INF);

            if pn_n == 0 || dn_n == 0 {
                // 証明時は budget に実際の証明深さ k を記録する:
                // k = max(1, max_i (子の証明深さ + 深さ増分))
                // （攻め方の1手 + 各観測分岐の証明深さ。Checkmate 分岐は寄与1）
                let entry_budget = if pn_n == 0 {
                    let mut k: u32 = 1;
                    for i in 0..n {
                        if expansion.obs_terminal[i] {
                            continue;
                        }
                        let inc = expansion.obs_depth_inc[i];
                        let child_k = self
                            .or_table
                            .get(&expansion.obs_hash[i])
                            .and_then(|e| e.proven_bound())
                            .unwrap_or_else(|| self.current_budget(depth + inc));
                        k = k.max(child_k.saturating_add(inc));
                    }
                    k
                } else {
                    budget_now
                };
                self.store_and(and_key, pn_n, dn_n, entry_budget);
                if pn_n == 0 {
                    // AND 証明駒の計算
                    let mut and_ph = [0u8; 7];
                    for i in 0..n {
                        let child_ph = if expansion.obs_terminal[i] {
                            [0u8; 7]
                        } else {
                            self.or_proof_hands.get(&expansion.obs_hash[i])
                                .copied().unwrap_or(expansion.parent_hand)
                        };
                        let child_hand = expansion.obs_hands[i];
                        for j in 0..7 {
                            let eff = (child_ph[j] as i16) + (expansion.parent_hand[j] as i16)
                                - (child_hand[j] as i16);
                            let eff = eff.max(0) as u8;
                            and_ph[j] = and_ph[j].max(eff);
                        }
                    }
                    self.and_proof_hands.insert(and_key, and_ph);
                }
                break;
            }

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.store_and(and_key, pn_n, dn_n, budget_now);
                break;
            }

            // 最弱の非終端子（dn 最小）を選択
            let best = (0..n)
                .filter(|&i| !expansion.obs_terminal[i])
                .min_by_key(|&i| obs_dn[i]);
            let best_idx = match best {
                Some(i) => i,
                None => {
                    self.store_and(and_key, pn_n, dn_n, budget_now);
                    break;
                }
            };

            let dn_2nd = (0..n)
                .filter(|&i| i != best_idx)
                .map(|i| obs_dn[i])
                .min()
                .unwrap_or(INF);
            let best_pn = obs_pn[best_idx];

            // 子のしきい値を計算（1+ε trick）
            let child_dn_limit =
                dn_limit.min(dn_2nd.saturating_add(1).saturating_add(dn_2nd / 4));
            let child_pn_limit = pn_limit.saturating_sub(pn_n).saturating_add(best_pn);

            // OR ノードに再帰
            let child_hash = expansion.obs_hash[best_idx];
            let child_depth = depth + expansion.obs_depth_inc[best_idx];
            path.push(child_hash);
            self.mid_or(
                &expansion.obs_metas[best_idx],
                child_hash,
                child_pn_limit,
                child_dn_limit,
                path,
                child_depth,
            );
            path.pop();

            // OR テーブルから値を読み戻す（子の予算でゲート）
            let child_budget = self.current_budget(child_depth);
            let (cpn, cdn) = self
                .or_table
                .get(&child_hash)
                .and_then(|e| e.effective(child_budget))
                .unwrap_or((1, 1));
            obs_pn[best_idx] = cpn;
            obs_dn[best_idx] = cdn;
        }

        // Phase 4: キャッシュに再挿入（上限に達したら世代交代）
        if self.and_expansion_cache.len() >= MAX_AND_EXPANSION_CACHE {
            self.and_expansion_cache_old = std::mem::take(&mut self.and_expansion_cache);
        }
        self.and_expansion_cache.insert(and_key, expansion);
    }

    /// OR ノードの初期値を取得（転置表 or デフォルト、予算ゲート付き）
    /// ループ検出時は反証扱い
    fn lookup_or(&self, hash: u64, path: &[u64], budget: u32) -> (u32, u32) {
        if path.contains(&hash) {
            return (INF, 0);
        }
        self.or_table
            .get(&hash)
            .and_then(|e| e.effective(budget))
            .unwrap_or((1, 1))
    }

    /// 同一観測タイプの分岐をマージ（sente_hand_hash グループ統合）
    /// 異なる合駒で生じた持ち駒の違いは、後続の legal/illegal split で正しく処理される
    /// 3 つ以上の同一観測グループがある場合のみマージ（少数グループではコスト対効果が悪い）
    /// 同一観測タイプの分岐をマージ（sente_hand_hash グループ統合）
    /// マージ条件:
    ///   1. 同一観測タイプが 3 グループ以上存在する
    ///   2. マージ対象の合計 position 数が MAX_MERGE_POSITIONS 以下
    /// 条件2により、深い探索でのcompound merging（マージ→展開→再マージ）による
    /// MetaPosition サイズの指数的膨張を防ぐ
    const MAX_MERGE_POSITIONS: usize = 7;

    fn merge_observation_branches(
        branches: Vec<(Observation, MetaPosition)>,
    ) -> Vec<(Observation, MetaPosition)> {
        use super::metaposition::position_hash;

        // 各観測タイプの出現回数と合計 position 数をカウント
        let mut obs_counts: FxHashMap<Observation, usize> = FxHashMap::default();
        let mut obs_total_positions: FxHashMap<Observation, usize> = FxHashMap::default();
        for (obs, meta) in &branches {
            *obs_counts.entry(obs.clone()).or_default() += 1;
            *obs_total_positions.entry(obs.clone()).or_default() += meta.positions.len();
        }

        let mut result: Vec<(Observation, MetaPosition)> = Vec::new();
        for (obs, meta) in branches {
            let count = obs_counts.get(&obs).copied().unwrap_or(0);
            let total_pos = obs_total_positions.get(&obs).copied().unwrap_or(0);
            if count >= 3 && total_pos <= Self::MAX_MERGE_POSITIONS {
                // マージ対象: 同一観測の既存分岐に統合（重複排除つき）
                if let Some(existing) = result.iter_mut().find(|(o, _)| *o == obs) {
                    let mut seen: FxHashSet<u64> = existing.1.positions.iter()
                        .map(|p| position_hash(p))
                        .collect();
                    for pos in meta.positions {
                        if seen.insert(position_hash(&pos)) {
                            existing.1.positions.push(pos);
                        }
                    }
                } else {
                    result.push((obs, meta));
                }
            } else {
                // マージ対象外: そのまま保持
                result.push((obs, meta));
            }
        }
        result
    }

    /// MetaPosition の sente_hand を取得（proof_hand 計算用）
    /// 全 position が同一の sente_hand を持つ場合はその値を返す（正確）
    /// 混合 sente_hand の場合（マージ後）は成分最小値を返す（保守的: proof_hand を大きくする方向）
    fn child_sente_hand(meta: &MetaPosition, default: [u8; 7]) -> [u8; 7] {
        if meta.positions.is_empty() {
            return default;
        }
        let first = meta.positions[0].hand(Color::Sente).counts_array();
        // 全 position が同一かチェック（ほとんどの場合 true → 高速パス）
        let all_same = meta.positions[1..].iter().all(|pos| {
            pos.hand(Color::Sente).counts_array() == first
        });
        if all_same {
            return first;
        }
        // 混合の場合: 成分最小値（proof_hand を大きくする保守的推定）
        let mut result = first;
        for pos in &meta.positions[1..] {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].min(h[j]);
            }
        }
        result
    }

    /// MetaPosition の sente_hand を取得（parent_hand 計算用）
    /// 全 position が同一の sente_hand を持つ場合はその値を返す（正確）
    /// 混合 sente_hand の場合（マージ後）は成分最大値を返す（保守的: proof_hand を大きくする方向）
    fn parent_sente_hand(meta: &MetaPosition, default: [u8; 7]) -> [u8; 7] {
        if meta.positions.is_empty() {
            return default;
        }
        let first = meta.positions[0].hand(Color::Sente).counts_array();
        let all_same = meta.positions[1..].iter().all(|pos| {
            pos.hand(Color::Sente).counts_array() == first
        });
        if all_same {
            return first;
        }
        // 混合の場合: 成分最大値（proof_hand を大きくする保守的推定）
        let mut result = first;
        for pos in &meta.positions {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].max(h[j]);
            }
        }
        result
    }

    /// 優越関係テーブルに proof_hand（証明に必要な最小の攻め方持ち駒）を記録する
    /// gote_hand を含むハッシュと組み合わせて使用: 同じ盤面+同じgote_hand で proof_hand <= S_new
    /// budget は証明時の残り深さ予算（この予算以内で詰むことを保証）
    fn record_proven_dominance(&mut self, meta: &MetaPosition, proof_hand: [u8; 7], budget: u32) {
        if meta.positions.is_empty() {
            return;
        }
        let board_hash = meta_board_set_hash(meta);
        self.add_dominance_entry(board_hash, proof_hand, budget);
    }

    /// dominance_table にエントリを追加（Pareto 最小を維持）
    /// (hand, budget) の両方で支配判定: hand が小さく budget も小さいエントリが優越
    /// 追加された場合 true を返す
    fn add_dominance_entry(&mut self, board_hash: u64, hand: [u8; 7], budget: u32) -> bool {
        let entries = self.dominance_table.entry(board_hash).or_default();
        if entries.iter().any(|(eh, eb)| {
            *eb <= budget && eh.iter().zip(hand.iter()).all(|(ei, hi)| ei <= hi)
        }) {
            return false;
        }
        entries.retain(|(eh, eb)| {
            !(budget <= *eb && eh.iter().zip(hand.iter()).all(|(ei, hi)| hi <= ei))
        });
        entries.push((hand, budget));
        true
    }

    /// 攻め方の候補手を生成する（＝全盤面の「王手になる手」の和集合）。
    ///
    /// 王手にならない手は候補にしない。衝立詰将棋では攻め方に毎手王手の義務が
    /// あり、mid_and が「指せた盤面の全てで王手になっていること」を要求して
    /// そうでない手をその場で反証するため、王手にならない手を候補に入れても
    /// AND ノードを1つ無駄にするだけで証明には決して寄与しない。
    /// 「ある盤面では王手・別の盤面では反則」という情報収集用のプローブ手は、
    /// 各盤面の王手手の和集合を取っている時点でこの候補集合に含まれている。
    ///
    /// 手生成の段階で王手候補に絞る:
    /// - 打ち: その駒種が玉に利きうるマス（`drop_check_mask`）だけに打つ。
    ///   空きマス全部を列挙すると1駒種あたり最大80手になり、ここが探索全体の
    ///   最大のコストになっていた
    /// - 盤上の駒の移動: 駒数が少ないので全生成し、幾何学的事前フィルタ
    ///   （直接王手＝駒種の利き / 開き王手＝移動元が discovery_squares）で絞る
    ///
    /// 絞り込みは偽陰性なし（王手になる手を落とさない）で、最後に実際に手を
    /// 指して玉が攻撃されているかを確認するため、結果は厳密な王手手の集合。
    fn generate_attack_candidates(&mut self, meta: &MetaPosition) -> Vec<Move> {
        if meta.positions.is_empty() {
            return Vec::new();
        }

        let n = meta.positions.len();
        self.gen_candidates_calls += 1;
        self.gen_candidates_total_positions += n as u64;

        let mut seen = FxHashSet::default();
        let mut check_moves = Vec::new();
        let mut raw: Vec<Move> = Vec::new();

        // 全盤面から王手の手を収集（union）
        for pos in meta.positions.iter() {
            if self.is_cancelled() {
                return Vec::new();
            }
            let attacker = pos.side_to_move;
            let Some(ksq) = pos.find_king(attacker.opponent()) else { continue };

            // 局面レベルの王手手キャッシュを確認
            let pos_hash = pos.zobrist_hash;
            if let Some(cached_checks) = self.check_moves_cache.get(&pos_hash) {
                self.check_moves_cache_hits += 1;
                for mv in cached_checks {
                    if seen.insert(*mv) {
                        check_moves.push(*mv);
                    }
                }
                continue;
            }
            self.check_moves_cache_misses += 1;

            // 開き王手の候補マスを事前計算
            let discovery_sqs = compute_discovery_squares(pos, ksq, attacker);
            // 自玉があるときだけ自殺手の除去が要る（詰将棋の攻め方には玉がない）
            let has_own_king = pos.find_king(attacker).is_some();

            raw.clear();
            crate::shogi::movegen::generate_board_moves(pos, attacker, &mut raw);
            crate::shogi::movegen::generate_drop_moves_masked(
                pos,
                attacker,
                |kind| drop_check_mask(attacker, kind, ksq),
                &mut raw,
            );

            let mut test_pos = pos.clone();
            let mut pos_checks = Vec::new();
            for mv in raw.iter() {
                // 玉を取る手は王手ではない
                if mv.to == ksq {
                    continue;
                }
                // 幾何学的事前フィルタ: 王手になりえない手をスキップ
                if !could_give_check(mv, ksq, &discovery_sqs, attacker) {
                    continue;
                }
                // 正確な王手判定（＋自殺手の除去）
                let undo = test_pos.make_move(*mv);
                let is_check =
                    test_pos.is_attacked(ksq, attacker) && !(has_own_king && test_pos.is_in_check(attacker));
                if is_check {
                    pos_checks.push(*mv);
                }
                test_pos.unmake_move(&undo);
            }

            // キャッシュに保存
            for mv in &pos_checks {
                if seen.insert(*mv) {
                    check_moves.push(*mv);
                }
            }
            self.check_moves_cache.insert(pos_hash, pos_checks);
        }

        // 候補手のソート
        let king_sq = meta.positions[0].find_king(Color::Gote);
        let sort_key = |mv: &Move| -> (u8, u8, u8, u8, u8, u8) {
            let is_drop = if mv.drop_piece.is_some() { 0u8 } else { 1u8 };
            let dist = if let Some(ksq) = king_sq {
                let df = (mv.to.file as i8 - ksq.file as i8).unsigned_abs();
                let dr = (mv.to.rank as i8 - ksq.rank as i8).unsigned_abs();
                df.max(dr)
            } else {
                0
            };
            let piece_kind = if let Some(drop_kind) = mv.drop_piece {
                drop_kind
            } else if let Some(from) = mv.from {
                meta.positions
                    .iter()
                    .find_map(|pos| {
                        pos.piece_at(from).map(|p| {
                            if mv.promotion {
                                p.kind.promoted().unwrap_or(p.kind)
                            } else {
                                p.kind
                            }
                        })
                    })
                    .unwrap_or(PieceKind::Pawn)
            } else {
                PieceKind::Pawn
            };
            let is_slider = matches!(
                piece_kind,
                PieceKind::Rook
                    | PieceKind::PromotedRook
                    | PieceKind::Bishop
                    | PieceKind::PromotedBishop
                    | PieceKind::Lance
            );
            let interposition = if is_slider && dist > 1 { dist - 1 } else { 0 };
            let piece_priority = match piece_kind {
                PieceKind::Rook | PieceKind::PromotedRook => 0,
                PieceKind::Bishop | PieceKind::PromotedBishop => 1,
                PieceKind::Gold
                | PieceKind::PromotedSilver
                | PieceKind::PromotedKnight
                | PieceKind::PromotedLance
                | PieceKind::PromotedPawn => 2,
                PieceKind::Silver => 3,
                PieceKind::Knight => 4,
                PieceKind::Lance => 5,
                PieceKind::Pawn => 6,
                _ => 7,
            };
            (
                is_drop,
                dist,
                interposition,
                piece_priority,
                mv.to.file,
                mv.to.rank,
            )
        };
        check_moves.sort_by_key(|mv| sort_key(mv));

        // 合駒可能マスが多い長距離王手を除外（打ち駒のみ）
        // 盤上の駒を動かす手は持ち駒を消費しないため除外しない
        let king_sq_for_filter = meta.positions[0].find_king(Color::Gote);
        check_moves.retain(|mv| {
            // 盤上の駒を動かす手は常に候補に残す
            if mv.drop_piece.is_none() {
                return true;
            }
            let piece_kind = mv.drop_piece.unwrap();
            let is_slider = matches!(
                piece_kind,
                PieceKind::Rook
                    | PieceKind::Bishop
                    | PieceKind::Lance
            );
            if !is_slider {
                return true;
            }
            if let Some(ksq) = king_sq_for_filter {
                let dist = ((mv.to.file as i8 - ksq.file as i8).unsigned_abs())
                    .max((mv.to.rank as i8 - ksq.rank as i8).unsigned_abs());
                let interp = if dist > 1 { dist - 1 } else { 0 };
                interp <= 4
            } else {
                true
            }
        });

        check_moves
    }

    // ========================================================================
    // 証明木の抽出
    // ========================================================================

    /// 探索して SolutionData を返す（既存ソルバーと同じインターフェース）
    pub fn solve_to_solution(&mut self, meta: &MetaPosition, find_shortest: bool) -> SolutionData {
        self.solve_to_solution_inner(meta, false, find_shortest)
    }

    /// 余詰めチェック付きで探索して SolutionData を返す
    pub fn solve_to_solution_with_second(
        &mut self,
        meta: &MetaPosition,
        find_shortest: bool,
    ) -> SolutionData {
        self.solve_to_solution_inner(meta, true, find_shortest)
    }

    fn solve_to_solution_inner(
        &mut self,
        meta: &MetaPosition,
        find_second: bool,
        find_shortest: bool,
    ) -> SolutionData {
        let result = self.solve(meta);
        match result {
            TsuitateDfpnResult::Proven => {
                let tree = self.extract_solution(meta);
                let depth = tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
                let initial_nodes = self.nodes_searched;

                // 最短解を探す
                // 余詰めチェック時は常に最短化する: 余詰めチェックは「見つかった主解木」の
                // 内部を除外プローブする実装のため、非最短の主解木を検査すると判定が
                // 探索順依存で不安定になる（例: 問題115 は最短19手だが最短化なしでは
                // 21手の木が主解になり、その木に対する別手順で「余詰めあり」になる）。
                // 常に最短木を対象にすることで「最短解に対する余詰め」として一意に定まる
                let (tree, depth, shorten_nodes) = if (find_shortest || find_second) && depth > 1 {
                    self.shorten_solution(meta, tree, depth)
                } else {
                    (tree, depth, 0)
                };

                // 無駄合い反則枝を畳み込む（最短の木に対して適用するのが正しい）。
                // 表示木と報告手数の両方を畳み込み後の木から取り、常に一致させる。
                // 中断時（None）は補正なし＝元の木と手数のまま（安全側）。
                // 余詰めチェックには畳み込み前の木を渡す: is_subsumed_by の
                // サブツリーハッシュ包含判定は、畳み込みされない第2解の木と
                // 同じ形で比較する必要があるため。
                let folded_tree = if self.omit_futile {
                    tree.as_ref().and_then(|t| muda_folded_tree(t, meta))
                } else {
                    None
                };
                let depth = folded_tree
                    .as_ref()
                    .map(|t| t.max_moves())
                    .unwrap_or(depth);
                let nodes_before_second = initial_nodes + shorten_nodes;

                // 余詰めチェック
                // find_second_solution は self.nodes_searched を first_phase_nodes として使うため、
                // shorten 分を含めた累計値を設定しておく
                self.nodes_searched = nodes_before_second;
                let (second_tree, kizu_trees, total_nodes, second_msg) = if find_second {
                    if let Some(ref t) = tree {
                        self.find_second_solution(meta, t)
                    } else {
                        (None, vec![], nodes_before_second, String::new())
                    }
                } else {
                    (None, vec![], nodes_before_second, String::new())
                };

                let kizu_suffix = if !kizu_trees.is_empty() {
                    format!("、キズ: {}件", kizu_trees.len())
                } else {
                    String::new()
                };

                let message = if second_tree.is_some() {
                    let second_depth = second_tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
                    format!(
                        "余詰めあり: {}手詰めと{}手詰めが見つかりました (探索ノード数: {}{})",
                        depth, second_depth, total_nodes, kizu_suffix
                    )
                } else if find_second && second_msg == "aborted" {
                    format!(
                        "{}手詰めが見つかりました（余詰め探索打ち切り・判定不能{}、探索ノード数: {}）",
                        depth, kizu_suffix, total_nodes
                    )
                } else if find_second && !second_msg.is_empty() {
                    if kizu_trees.is_empty() {
                        format!(
                            "{}手詰めが見つかりました（余詰めなし、探索ノード数: {}）",
                            depth, total_nodes
                        )
                    } else {
                        format!(
                            "{}手詰めが見つかりました（余詰めなし{}、探索ノード数: {}）",
                            depth, kizu_suffix, total_nodes
                        )
                    }
                } else {
                    format!(
                        "{}手詰めが見つかりました (探索ノード数: {})",
                        depth, total_nodes
                    )
                };

                SolutionData {
                    found: true,
                    // 畳み込みに成功していれば表示木も畳み込み後のものに置き換える
                    tree: folded_tree.or(tree),
                    second_tree,
                    kizu_trees,
                    message,
                    trace: Vec::new(),
                }
            }
            TsuitateDfpnResult::Disproven => SolutionData {
                found: false,
                tree: None,
                second_tree: None,
                kizu_trees: vec![],
                message: format!(
                    "詰みは存在しません (探索ノード数: {})",
                    self.nodes_searched
                ),
                trace: Vec::new(),
            },
            TsuitateDfpnResult::Unknown => SolutionData {
                found: false,
                tree: None,
                second_tree: None,
                kizu_trees: vec![],
                message: format!(
                    "探索を打ち切りました (探索ノード数: {})",
                    self.nodes_searched
                ),
                trace: Vec::new(),
            },
        }
    }

    /// 下方向スキャンで最短の深さを求める
    ///
    /// 初回解の深さ−2 を depth_limit として探索し、証明に成功したら
    /// 抽出した解の実際の深さまで一気に上限を狭めて繰り返す。
    /// 反証されたら現在の best が最短と確定する。
    ///
    /// 上方向の線形スキャン（深さ1, 3, 5, ...）と比べ、最短解が初回解と
    /// 同じ深さの場合に反証探索1回で済む（97手詰めなら48回→1回）。
    ///
    /// 最適化:
    /// - 転置表エントリは残り深さ予算（budget）でゲートされるため、
    ///   反復間・深さ制限の有無をまたいでも安全に保持できる。
    ///   下方向スキャンでは前回プローブの反証エントリがそのまま再利用される。
    /// - 深さ非依存のキャッシュ（and_expansion_cache, or_candidate_cache,
    ///   move_hints, proof_cache, check_moves_cache）も
    ///   全反復で保持。proof_cache のリプレイは try_replay_proof が
    ///   depth_limit を検証するため深さ制限下でも安全。
    fn shorten_solution(
        &mut self,
        meta: &MetaPosition,
        initial_tree: Option<SolutionNode>,
        initial_depth: u32,
    ) -> (Option<SolutionNode>, u32, u64) {
        let mut best_tree = initial_tree;
        let mut best_depth = initial_depth;
        let mut total_nodes: u64 = 0;

        let root_hash = meta_position_hash(meta);
        // 証明に成功した最小の深さ制限（ループ終了後の抽出で使用）
        let mut proven_limit: Option<u32> = None;

        let mut target = best_depth.saturating_sub(2);
        while target >= 1 {
            if self.should_stop() {
                break;
            }

            self.nodes_searched = 0;
            self.depth_limit = Some(target);

            let probe_start = std::time::Instant::now();
            let result = self.solve(meta);
            total_nodes += self.nodes_searched;
            if std::env::var("SHORTEN_DEBUG").is_ok() {
                eprintln!(
                    "[shorten] target={} result={:?} nodes={} time={:.3}s",
                    target, result, self.nodes_searched,
                    probe_start.elapsed().as_secs_f64(),
                );
            }

            if result != TsuitateDfpnResult::Proven {
                break; // 反証（または打ち切り）→ 現在の証明済み深さが最短
            }

            // 証明深さ k（ルートエントリの proof_k）で次のターゲットへジャンプ
            let k = self
                .or_table
                .get(&root_hash)
                .and_then(|e| e.proven_bound())
                .unwrap_or(target)
                .min(target);
            proven_limit = Some(k.max(1));
            if k <= 2 {
                break; // これ以上短くならない（1手詰めが証明済み）
            }
            target = k - 2;
        }

        // 抽出はループ終了後に1回だけ行う（プローブごとの抽出は高コスト）。
        // 最後に証明された深さ制限の下で抽出することで、混在した転置表から
        // 深い方の解を拾わないよう budget ゲートを効かせる。
        if let Some(limit) = proven_limit {
            self.depth_limit = Some(limit);
            let tree = self.extract_solution(meta);
            let actual_depth = tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
            if actual_depth > 0 && actual_depth < best_depth {
                best_tree = tree;
                best_depth = actual_depth;
            }
        }

        self.depth_limit = None;
        (best_tree, best_depth, total_nodes)
    }

    /// キズ（許容余詰め）の判定
    ///
    /// 主解の手と別解の手が両方ともプローブ手（Illegal分岐を持つ）である場合、
    /// これはプローブ代替手に過ぎず、真の余詰めではないと判断する。
    fn is_kizu(main_node: &SolutionNode, alt_node: &SolutionNode) -> bool {
        fn has_illegal_branch(node: &SolutionNode) -> bool {
            match node {
                SolutionNode::AttackMove { branches, .. } => {
                    branches.iter().any(|b| b.observation == Observation::Illegal)
                }
                _ => false,
            }
        }
        has_illegal_branch(main_node) && has_illegal_branch(alt_node)
    }

    fn find_second_solution(
        &mut self,
        meta: &MetaPosition,
        first_tree: &SolutionNode,
    ) -> (Option<SolutionNode>, Vec<SolutionNode>, u64, String) {
        let first_key = match first_tree {
            SolutionNode::AttackMove { mv, .. } => ExclusionKey::from_move_data(mv),
            SolutionNode::Checkmate { .. } => {
                return (None, vec![], self.nodes_searched, String::new());
            }
        };

        // 注: 以前はここで「最長応手経路上の証明手の一意性」プレチェック
        // （has_unique_longest_path）により早期リターンしていたが、
        // 「転置表に証明済みの手が1つしかない」ことは「別手順が存在しない」
        // ことを意味しないため不健全（初回探索が偶発的に別手を証明したか
        // どうかで判定が変わり、実際に余詰めのある問題92を完全作と誤答した）。
        // 除外プローブにノード上限が付いたため、常に実探索で確認する。
        let root_hash = meta_position_hash(meta);

        let first_hashes = first_tree.collect_attack_subtree_hashes();
        let initial_nodes = self.nodes_searched;
        let mut kizu_trees: Vec<SolutionNode> = vec![];
        self.second_search_aborted = false;
        self.second_probe_cap = self.second_probe_node_cap(initial_nodes);

        // Phase 0: 転置表の既存証明から別手順を直接抽出する
        // 初回探索が偶発的に複数の手を証明している場合（プレチェックが
        // 一意でないと判定した場合）、除外付き再探索なしで別手順が見つかる。
        // 問題によっては除外付き再探索が極端に高コスト（数百秒）になるため、
        // まずテーブルだけで抽出を試みる。
        if let Some(alt) = self.find_table_alternative(
            meta, first_tree, &first_hashes, &mut kizu_trees, 0,
        ) {
            if std::env::var("SECOND_DEBUG").is_ok() {
                eprintln!("[second] phase0 found table alternative");
            }
            return (Some(alt), kizu_trees, self.nodes_searched, "found_table".to_string());
        }

        // Phase 1: 初手除外で再探索
        // 転置表・キャッシュは保持する: ルート以外のエントリと証明成果物
        // （dominance, proof_cache 等）は除外条件に依存しないため再利用できる。
        // ルートの or エントリのみ除外条件に依存するため、探索の前後で削除する。
        // （事前削除は必須: 残したまま探索が打ち切られると、solve() が
        //   初回探索の証明＝除外手を使った証明を読んで偽の Proven を返す）
        let mut excluded = vec![first_key];
        excluded.extend(first_key.promotion_counterpart());

        self.or_table.remove(&root_hash);
        self.nodes_searched = 0;
        self.excluded_root_moves = excluded;

        let saved_limit = self.node_limit;
        self.node_limit = saved_limit.min(self.second_probe_cap);
        let phase1_start = std::time::Instant::now();
        let result = self.solve(meta);
        self.node_limit = saved_limit;
        if result == TsuitateDfpnResult::Unknown {
            self.second_search_aborted = true;
        }
        let mut total_nodes = initial_nodes + self.nodes_searched;
        if std::env::var("SECOND_DEBUG").is_ok() {
            eprintln!(
                "[second] phase1 exclude={:?} result={:?} nodes={} time={:.3}s",
                first_key, result, self.nodes_searched,
                phase1_start.elapsed().as_secs_f64(),
            );
        }

        // 抽出はルート除外フィルタが効くよう excluded_root_moves を保持したまま行う
        let second_tree = if result == TsuitateDfpnResult::Proven {
            self.extract_solution(meta)
        } else {
            None
        };
        self.excluded_root_moves.clear();
        self.or_table.remove(&root_hash);

        if std::env::var("SECOND_DEBUG").is_ok() {
            match &second_tree {
                None => eprintln!("[second] phase1 discard: extraction=None"),
                Some(t) => eprintln!(
                    "[second] phase1 tree: depth={} subsumed={} kizu={}",
                    t.max_moves(),
                    t.is_subsumed_by(&first_hashes),
                    Self::is_kizu(first_tree, t),
                ),
            }
        }
        if let Some(ref second) = second_tree {
            if !second.is_subsumed_by(&first_hashes) {
                if Self::is_kizu(first_tree, second) {
                    // キズ（プローブ代替手）として収集、探索を続行
                    kizu_trees.push(second_tree.unwrap());
                } else {
                    // 真の余詰め → 即座に返す
                    return (second_tree, kizu_trees, total_nodes, "found".to_string());
                }
            }
        }

        // Phase 2: 内部ORノードで別手順を探索
        if let Some(alt_tree) = self.find_inner_alternative(
            meta, first_tree, &first_hashes, &mut total_nodes, &mut kizu_trees,
        ) {
            return (Some(alt_tree), kizu_trees, total_nodes, "found".to_string());
        }

        // 打ち切られた除外探索がある場合、「余詰めなし」は確定ではない
        let method = if self.second_search_aborted {
            "aborted"
        } else if kizu_trees.is_empty() {
            "not_found"
        } else {
            "kizu_only"
        };
        (None, kizu_trees, total_nodes, method.to_string())
    }

    /// 余詰めチェックの除外探索1回あたりのノード上限
    /// 初回探索の規模に比例させ、極端に難しい除外探索（存在しない別解の反証）で
    /// 制限時間全体を消費しないようにする。上限到達時は判定不能として報告される。
    fn second_probe_node_cap(&self, first_solve_nodes: u64) -> u64 {
        first_solve_nodes.saturating_mul(4).max(100_000)
    }

    /// 転置表に既にある証明だけで別手順（余詰め）を探す（Phase 0）
    ///
    /// 1つ目の解の手順木を最長応手分岐に沿って辿り、各 OR ノードで
    /// 主解と異なる証明済みの手（AND テーブルで pn=0）を探して抽出する。
    /// 初回探索が偶発的に複数の手を証明していた場合、除外付き再探索なしで
    /// 別手順が得られる。抽出は自己修復（実体化）付き。
    fn find_table_alternative(
        &mut self,
        meta: &MetaPosition,
        node: &SolutionNode,
        first_hashes: &HashSet<u64>,
        kizu_trees: &mut Vec<SolutionNode>,
        depth: u32,
    ) -> Option<SolutionNode> {
        // 打ち切り・巨大メタは「余詰めなし」ではなく「判定不能」として報告する
        if self.should_stop() || meta.positions.len() > MAX_META_POSITIONS {
            self.second_search_aborted = true;
            return None;
        }
        let (mv_data, branches) = match node {
            SolutionNode::Checkmate { .. } => return None,
            SolutionNode::AttackMove { mv, branches, .. } => (mv, branches),
        };
        let chosen = ExclusionKey::from_move_data(mv_data);

        // このノードで転置表上の別の証明手を探す（最終手は余詰の対象外）
        // 注: 子へ降りるには「指せる」主解手が要る（下の chosen_mv）。照合だけなら
        // 駒種の要らない chosen で足りる
        if !node.is_final_move() && !meta.is_empty() {
            let hash = meta_position_hash(meta);
            let budget_now = self.current_budget(depth);

            // 転置表に AND エントリが立ちうるのは探索が候補にした手だけなので、
            // 全合法手を作る必要はない（打ちの列挙が支配的で、走査系で一番重かった）
            let all_moves = self.generate_attack_candidates(meta);

            for mv in &all_moves {
                // 主解の手（成/不成違いを含む）は対象外
                if chosen.same_target(mv) {
                    continue;
                }
                let ak = Self::and_key(hash, mv);
                let and_proven = self
                    .and_table
                    .get(&ak)
                    .and_then(|e| e.proven_bound())
                    .map_or(false, |k| k <= budget_now);
                if !and_proven {
                    continue;
                }
                let extracted = self.extract_and(meta, *mv, depth);
                if std::env::var("SECOND_DEBUG").is_ok() {
                    eprintln!(
                        "[second] phase0 candidate depth={} chosen={} alt={:?} extract_ok={}",
                        depth, mv_data.notation, mv, extracted.is_some(),
                    );
                }
                if let Some(alt_branches) = extracted {
                    let alt = SolutionNode::AttackMove {
                        mv: MoveData::from_move(*mv, Color::Sente),
                        branches: alt_branches,
                        meta_hash: Some(hash),
                    };
                    let subsumed = alt.is_subsumed_by(first_hashes);
                    let kizu = Self::is_kizu(node, &alt);
                    if std::env::var("SECOND_DEBUG").is_ok() {
                        eprintln!(
                            "[second] phase0 extracted alt_depth={} subsumed={} kizu={}",
                            alt.max_moves(), subsumed, kizu,
                        );
                    }
                    if !subsumed {
                        if kizu {
                            kizu_trees.push(alt);
                        } else {
                            return Some(alt);
                        }
                    }
                }
            }
        }

        // 最長応手分岐のみ子へ再帰（find_inner_alt_recursive と同じ走査）
        // 情報集合に適用する手は駒種まで復元する（ExclusionKey は照合専用で指せない）。
        // 復元できないのは手順木と情報集合の食い違いなので、代用せず判定不能にする
        let Some(chosen_mv) = md_to_move(mv_data, meta) else {
            self.second_search_aborted = true;
            return None;
        };
        let (legal_meta, illegal_meta) = meta.apply_attack_move_split_fast(chosen_mv);
        let obs_metas: Vec<(Observation, MetaPosition)> =
            if !legal_meta.is_empty() && !legal_meta.all_effectively_checkmate() {
                let raw = legal_meta.expand_defense_moves(chosen_mv);
                Self::merge_observation_branches(raw)
            } else {
                vec![]
            };
        if self.is_cancelled() {
            // 展開が協調キャンセルで部分結果の可能性があるため使わない
            self.second_search_aborted = true;
            return None;
        }

        let max_depth = node.max_moves();
        // find_inner_alt_recursive と同じ理由: 最善防御が反則分岐にある場合、
        // 非反則の同長分岐は玉方最善で到達しないため走査しない。
        let has_longest_illegal = branches.iter().any(|b| {
            b.observation == Observation::Illegal && b.continuation.max_moves() >= max_depth
        });
        for (branch_idx, branch) in branches.iter().enumerate() {
            let branch_depth = match branch.observation {
                Observation::Checkmate => 1,
                Observation::Illegal => branch.continuation.max_moves(),
                _ => 2 + branch.continuation.max_moves(),
            };
            if branch_depth < max_depth {
                continue;
            }
            if has_longest_illegal && branch.observation != Observation::Illegal {
                continue;
            }

            let (child_meta, child_depth) = match &branch.observation {
                Observation::Checkmate => continue,
                Observation::Illegal => (illegal_meta.clone(), depth),
                obs => match obs_metas.iter().find(|(o, _)| o == obs) {
                    Some((_, m)) => (m.clone(), depth + 2),
                    None => continue,
                },
            };

            if let Some(alt_sub) = self.find_table_alternative(
                &child_meta,
                &branch.continuation,
                first_hashes,
                kizu_trees,
                child_depth,
            ) {
                let mut new_branches = branches.clone();
                new_branches[branch_idx].continuation = Box::new(alt_sub);
                return Some(SolutionNode::AttackMove {
                    mv: mv_data.clone(),
                    branches: new_branches,
                    meta_hash: None,
                });
            }
        }

        None
    }

    /// 手順木の内部ORノードで余詰め（別手順）を探す
    ///
    /// 1つ目の解の手順木を辿りながら、各ORノードで選ばれた手を除外して
    /// 再探索し、別の詰み手順がないかチェックする。
    /// キズ（プローブ代替手）は kizu_trees に収集し、真の余詰めのみ返す。
    fn find_inner_alternative(
        &mut self,
        meta: &MetaPosition,
        first_tree: &SolutionNode,
        first_hashes: &HashSet<u64>,
        total_nodes: &mut u64,
        kizu_trees: &mut Vec<SolutionNode>,
    ) -> Option<SolutionNode> {
        self.find_inner_alt_recursive(meta, first_tree, first_tree, first_hashes, total_nodes, kizu_trees)
    }

    fn find_inner_alt_recursive(
        &mut self,
        meta: &MetaPosition,
        node: &SolutionNode,
        root_node: &SolutionNode,
        first_hashes: &HashSet<u64>,
        total_nodes: &mut u64,
        kizu_trees: &mut Vec<SolutionNode>,
    ) -> Option<SolutionNode> {
        // 打ち切り・巨大メタは「余詰めなし」ではなく「判定不能」として報告する。
        // 探索本体（mid_or/mid_and）は MAX_META_POSITIONS 超の分岐を不詰扱いで
        // 枝刈りするが、この走査は手順木を再展開するため同じ上限で守らないと
        // 情報集合が指数的に膨張してメモリを食い潰す（実測: 数GB）
        if self.should_stop() || meta.positions.len() > MAX_META_POSITIONS {
            self.second_search_aborted = true;
            return None;
        }
        let (mv_data, branches) = match node {
            SolutionNode::Checkmate { .. } => return None,
            SolutionNode::AttackMove { mv, branches, .. } => (mv, branches),
        };

        // 復元できないのは手順木と情報集合が食い違っているとき（本来起きない）。
        // 駒種を落とした手で代用すると全局面が不合法になり、観測分岐を1つも
        // 走査しないまま「余詰めなし」を確定報告してしまうので、判定不能にする
        let Some(mv) = md_to_move(mv_data, meta) else {
            self.second_search_aborted = true;
            return None;
        };

        // この攻め手を適用して観測分岐を復元
        let (legal_meta, illegal_meta) = meta.apply_attack_move_split_fast(mv);

        let obs_metas: Vec<(Observation, MetaPosition)> =
            if !legal_meta.is_empty() && !legal_meta.all_effectively_checkmate() {
                let raw = legal_meta.expand_defense_moves(mv);
                Self::merge_observation_branches(raw)
            } else {
                vec![]
            };
        if self.is_cancelled() {
            // 展開が協調キャンセルで部分結果の可能性があるため使わない
            self.second_search_aborted = true;
            return None;
        }

        // 最長応手分岐のみ探索
        let max_depth = node.max_moves();

        // プローブ手の Illegal 分岐が最長応手（＝玉方の最善防御）である場合、
        // 非 Illegal 分岐（Captured/NoCapture）は玉方が最善を尽くせば到達しない
        // 変化であり、そこで見つかる別手順は余詰めに当たらない（ユーザー判断）。
        // 例: 問題46 では ▲８四桂打（プローブ手）の反則分岐が最善防御で先手の
        // 続手が一意に決まる。取られる分岐（非最善）での別手順は余詰めではない。
        let has_longest_illegal = branches.iter().any(|b| {
            b.observation == Observation::Illegal && b.continuation.max_moves() >= max_depth
        });

        // 各ブランチの子MetaPositionを特定して再帰
        for (branch_idx, branch) in branches.iter().enumerate() {
            // 最長でない分岐はスキップ
            let branch_depth = match branch.observation {
                Observation::Checkmate => 1,
                Observation::Illegal => branch.continuation.max_moves(),
                _ => 2 + branch.continuation.max_moves(),
            };
            if branch_depth < max_depth {
                continue;
            }
            // 最善防御が反則分岐にある場合、非反則の同長分岐は走査しない
            if has_longest_illegal && branch.observation != Observation::Illegal {
                continue;
            }

            let child_meta = match &branch.observation {
                Observation::Checkmate => continue,
                Observation::Illegal => illegal_meta.clone(),
                obs => match obs_metas.iter().find(|(o, _)| o == obs) {
                    Some((_, m)) => m.clone(),
                    None => continue,
                },
            };

            // この子ORノードで別の手を試す（最終手は余詰の対象外）
            if let SolutionNode::AttackMove { mv: child_mv, .. } = branch.continuation.as_ref() {
                if !branch.continuation.is_final_move() {
                    let child_move = ExclusionKey::from_move_data(child_mv);
                    if let Some(alt_subtree) =
                        self.try_solve_excluding(&child_meta, child_move, first_hashes, total_nodes)
                    {
                        if Self::is_kizu(&branch.continuation, &alt_subtree) {
                            // キズとして収集、full tree を構築して kizu_trees に追加
                            let mut new_branches = branches.clone();
                            new_branches[branch_idx].continuation = Box::new(alt_subtree);
                            kizu_trees.push(Self::rebuild_full_tree(root_node, node, branch_idx, new_branches.clone()));
                            // continue — 真の余詰めを探し続ける
                        } else {
                            // 真の余詰め → 即座に返す
                            let mut new_branches = branches.clone();
                            new_branches[branch_idx].continuation = Box::new(alt_subtree);
                            return Some(SolutionNode::AttackMove {
                                mv: mv_data.clone(),
                                branches: new_branches,
                                meta_hash: None,
                            });
                        }
                    }
                }
            }

            // 更に深いノードで試す
            if let Some(deep_alt) = self.find_inner_alt_recursive(
                &child_meta,
                &branch.continuation,
                root_node,
                first_hashes,
                total_nodes,
                kizu_trees,
            ) {
                let mut new_branches = branches.clone();
                new_branches[branch_idx].continuation = Box::new(deep_alt);
                return Some(SolutionNode::AttackMove {
                    mv: mv_data.clone(),
                    branches: new_branches,
                    meta_hash: None,
                });
            }
        }

        None
    }

    /// find_inner_alt_recursive でキズを検出した際、ルートからの完全な手順木を構築する
    ///
    /// inner_alt_recursive は再帰的に呼び出されるため、検出時点では部分木しか持っていない。
    /// キズの表示にはルートからの完全な手順木が必要なので、root_node をベースに
    /// 変更箇所のみ差し替えた完全な手順木を返す。
    ///
    /// 簡易実装: 現在のノードレベルの new_branches を使って AttackMove を構築する。
    /// find_inner_alt_recursive の戻り値と同様の形式（親ノードの branches を差し替えた部分木）。
    fn rebuild_full_tree(
        _root_node: &SolutionNode,
        current_node: &SolutionNode,
        _branch_idx: usize,
        new_branches: Vec<SolutionBranch>,
    ) -> SolutionNode {
        match current_node {
            SolutionNode::AttackMove { mv, .. } => SolutionNode::AttackMove {
                mv: mv.clone(),
                branches: new_branches,
                meta_hash: None,
            },
            _ => current_node.clone(),
        }
    }

    /// 指定MetaPositionで特定の手を除外して解を探す
    fn try_solve_excluding(
        &mut self,
        meta: &MetaPosition,
        excluded_move: ExclusionKey,
        first_hashes: &HashSet<u64>,
        total_nodes: &mut u64,
    ) -> Option<SolutionNode> {
        if self.is_cancelled() {
            return None;
        }

        let mut excluded = vec![excluded_move];
        excluded.extend(excluded_move.promotion_counterpart());

        // 転置表・キャッシュは保持（find_second_solution Phase 1 と同じ理由）
        // ルートの or エントリは除外条件に依存するため探索の前後で削除する
        // （事前削除は必須: 打ち切り時に既存の証明を読んで偽の Proven になる）
        let probe_root_hash = meta_position_hash(meta);
        self.or_table.remove(&probe_root_hash);
        self.nodes_searched = 0;
        self.excluded_root_moves = excluded;

        let saved_limit = self.node_limit;
        self.node_limit = saved_limit.min(self.second_probe_cap);
        let probe_start = std::time::Instant::now();
        let result = self.solve(meta);
        self.node_limit = saved_limit;
        if result == TsuitateDfpnResult::Unknown {
            self.second_search_aborted = true;
        }
        *total_nodes += self.nodes_searched;
        if std::env::var("SECOND_DEBUG").is_ok() {
            eprintln!(
                "[second] exclude={:?} meta_size={} result={:?} nodes={} time={:.3}s",
                excluded_move, meta.positions.len(), result, self.nodes_searched,
                probe_start.elapsed().as_secs_f64(),
            );
        }

        // 抽出はルート除外フィルタが効くよう excluded_root_moves を保持したまま行う
        let ret = match result {
            TsuitateDfpnResult::Proven => {
                let tree = self.extract_solution(meta);
                match tree {
                    Some(ref t) if !t.is_subsumed_by(first_hashes) => tree,
                    _ => None,
                }
            }
            _ => None,
        };
        self.excluded_root_moves.clear();
        self.or_table.remove(&probe_root_hash);
        ret
    }

    /// 証明木を抽出する（solve() で Proven になった後に呼ぶ）
    ///
    /// 抽出が完全に失敗した場合（優越関係などで「証明済み」の記録だけがあり、
    /// 子の証明が転置表に実体化されていない場合）は、優越関係を抑制した
    /// 限定再探索で証明を実体化してから1回だけ再試行する（自己修復）。
    /// 実体化はトップレベルでのみ行う: 再帰中の各ノードで行うと、抽出の
    /// 通常の失敗パス（親が別の手を試すための None）でも発火して激重になる。
    pub fn extract_solution(&mut self, meta: &MetaPosition) -> Option<SolutionNode> {
        if let Some(tree) = self.extract_or(meta, 0) {
            return Some(tree);
        }
        if meta.is_empty() {
            return None;
        }
        let hash = meta_position_hash(meta);
        let proven = self
            .or_table
            .get(&hash)
            .and_then(|e| e.proven_bound())
            .map_or(false, |k| k <= self.current_budget(0));
        if !proven {
            return None;
        }
        if self.materialize_proof(meta, hash, 0) {
            return self.extract_or(meta, 0);
        }
        None
    }

    /// 証明の実体化再探索のノード上限
    const MATERIALIZE_NODE_BUDGET: u64 = 1_000_000;

    /// OR ノードから証明木を抽出する
    fn extract_or(&mut self, meta: &MetaPosition, depth: u32) -> Option<SolutionNode> {
        if self.is_cancelled() {
            return None;
        }
        if meta.is_empty() {
            return None;
        }
        if meta.all_effectively_checkmate() {
            return Some(SolutionNode::Checkmate {
                depth,
                hand_count: meta.max_sente_hand_count(),
            });
        }

        let hash = meta_position_hash(meta);
        let budget_now = self.current_budget(depth);
        let proven = self
            .or_table
            .get(&hash)
            .and_then(|e| e.proven_bound())
            .map_or(false, |k| k <= budget_now);
        if !proven {
            return None; // 証明されていない（または現在の深さ予算では未証明）
        }

        // 攻め方の候補手（＝全盤面の王手手の和集合）。転置表に AND エントリが
        // 立ちうるのはこの集合の手だけなので、全合法手を作る必要はない
        let mut all_moves = self.generate_attack_candidates(meta);

        // 候補手をソート（解の見た目を良くするため）
        let king_sq = meta.positions[0].find_king(Color::Gote);
        all_moves.sort_by_key(|mv| {
            let is_drop = if mv.drop_piece.is_some() { 0u8 } else { 1u8 };
            let dist = if let Some(ksq) = king_sq {
                let df = (mv.to.file as i8 - ksq.file as i8).unsigned_abs();
                let dr = (mv.to.rank as i8 - ksq.rank as i8).unsigned_abs();
                df.max(dr)
            } else {
                0
            };
            (is_drop, dist, mv.to.file, mv.to.rank)
        });

        // ルートでは除外手をスキップ（余詰めチェック中に保持された
        // 転置表の除外手エントリから主解を再抽出しないため）
        let at_root_with_exclusions =
            self.root_hash == Some(hash) && !self.excluded_root_moves.is_empty();

        self.extract_or_move(
            meta, hash, depth, budget_now, &all_moves, at_root_with_exclusions,
        )
    }

    /// 転置表から証明された手を探して抽出する（extract_or の本体）
    #[allow(clippy::too_many_arguments)]
    fn extract_or_move(
        &mut self,
        meta: &MetaPosition,
        hash: u64,
        depth: u32,
        budget_now: u32,
        all_moves: &[Move],
        at_root_with_exclusions: bool,
    ) -> Option<SolutionNode> {
        // AND テーブルで pn=0 の手を探す
        for mv in all_moves {
            if at_root_with_exclusions && self.is_excluded_root_move(mv) {
                continue;
            }
            let ak = Self::and_key(hash, mv);
            let and_proven = self
                .and_table
                .get(&ak)
                .and_then(|e| e.proven_bound())
                .map_or(false, |k| k <= budget_now);
            if and_proven {
                if let Some(branches) = self.extract_and(meta, *mv, depth) {
                    return Some(SolutionNode::AttackMove {
                        mv: MoveData::from_move(*mv, Color::Sente),
                        branches,
                        meta_hash: Some(hash),
                    });
                }
            }
        }

        // AND テーブルで見つからない場合（優越関係で証明されたノード）
        // 各候補手で直接 extract_and を試す
        for mv in all_moves {
            if at_root_with_exclusions && self.is_excluded_root_move(mv) {
                continue;
            }
            if let Some(branches) = self.extract_and(meta, *mv, depth) {
                return Some(SolutionNode::AttackMove {
                    mv: MoveData::from_move(*mv, Color::Sente),
                    branches,
                    meta_hash: Some(hash),
                });
            }
        }
        None
    }

    /// 証明の実体化: 優越関係チェックを抑制したノード上限付き再探索で、
    /// このノードの証明木（子ノードの証明エントリ）を転置表に展開する。
    /// 再探索内の証明木リプレイは有効なため、リプレイ可能ならそちらで実体化される。
    fn materialize_proof(&mut self, meta: &MetaPosition, hash: u64, depth: u32) -> bool {
        if self.should_stop() {
            return false;
        }

        let saved_limit = self.node_limit;
        self.node_limit = saved_limit
            .min(self.nodes_searched.saturating_add(Self::MATERIALIZE_NODE_BUDGET));
        let saved_suppress = self.dominance_suppressed;
        self.dominance_suppressed = true;

        let mut path = vec![hash];
        self.mid_or(meta, hash, INF, INF, &mut path, depth);

        self.dominance_suppressed = saved_suppress;
        self.node_limit = saved_limit;
        true
    }

    /// AND ノード（観測分岐）から証明木を抽出する
    fn extract_and(
        &mut self,
        meta: &MetaPosition,
        mv: Move,
        depth: u32,
    ) -> Option<Vec<SolutionBranch>> {
        let (legal_meta, illegal_meta) =
            meta.apply_attack_move_split(mv);

        let mut branches = Vec::new();

        // Illegal 分岐
        if !illegal_meta.is_empty() {
            let continuation = self.extract_or(&illegal_meta, depth)?;
            branches.push(SolutionBranch {
                observation: Observation::Illegal,
                sente_hand: None,
                continuation: Box::new(continuation),
            });
        }

        if legal_meta.is_empty() {
            return if branches.is_empty() {
                None
            } else {
                Some(branches)
            };
        }

        // 全合法盤面で実質詰み
        if legal_meta.all_effectively_checkmate() {
            branches.push(SolutionBranch {
                observation: Observation::Checkmate,
                sente_hand: Some(legal_meta.min_sente_hand_counts()),
                continuation: Box::new(SolutionNode::Checkmate {
                    depth: depth + 1,
                    hand_count: legal_meta.max_sente_hand_count(),
                }),
            });
            return Some(branches);
        }

        // 玉方の応手を展開（mid_and と同じマージを適用）
        let raw_obs_branches = legal_meta.expand_defense_moves(mv);
        if self.is_cancelled() {
            // 協調キャンセルによる部分展開を抽出に使わない
            return None;
        }
        let obs_branches = Self::merge_observation_branches(raw_obs_branches);
        for (obs, branch_meta) in obs_branches {
            match obs {
                Observation::Checkmate => {
                    branches.push(SolutionBranch {
                        observation: Observation::Checkmate,
                        sente_hand: Some(branch_meta.min_sente_hand_counts()),
                        continuation: Box::new(SolutionNode::Checkmate {
                            depth: depth + 1,
                            hand_count: branch_meta.max_sente_hand_count(),
                        }),
                    });
                }
                Observation::Captured { .. } | Observation::NoCapture => {
                    // depth + 2: 攻め方の手(+1) + 玉方の応手(+1)
                    let continuation = self.extract_or(&branch_meta, depth + 2)?;
                    branches.push(SolutionBranch {
                        observation: obs,
                        sente_hand: Some(branch_meta.min_sente_hand_counts()),
                        continuation: Box::new(continuation),
                    });
                }
                Observation::Illegal => {}
            }
        }

        Some(branches)
    }

    // ========================================================================
    // コンパクト証明木の抽出（リプレイキャッシュ用）
    // ========================================================================

    /// コンパクト証明木を抽出（リプレイキャッシュ用）
    fn extract_compact_or(&mut self, meta: &MetaPosition) -> Option<ProofNode> {
        if meta.is_empty() { return None; }
        if meta.all_effectively_checkmate() { return Some(ProofNode::Checkmate); }

        let hash = meta_position_hash(meta);
        let entry = self.or_table.get(&hash)?;
        if entry.proven_bound().is_none() { return None; }

        // 候補手を収集（extract_or と同じ）
        let all_moves = self.generate_attack_candidates(meta);

        // ルートでは除外手をスキップ（extract_or と同じ理由）
        let at_root_with_exclusions =
            self.root_hash == Some(hash) && !self.excluded_root_moves.is_empty();

        // pn=0 の手を探す
        for mv in &all_moves {
            if at_root_with_exclusions && self.is_excluded_root_move(mv) {
                continue;
            }
            let ak = Self::and_key(hash, mv);
            if let Some(e) = self.and_table.get(&ak) {
                if e.proven_bound().is_some() {
                    if let Some(branches) = self.extract_compact_and(meta, *mv) {
                        return Some(ProofNode::Attack { mv: *mv, branches });
                    }
                }
            }
        }

        // dominance で証明されたノード: 各候補手で直接試す
        for mv in &all_moves {
            if at_root_with_exclusions && self.is_excluded_root_move(mv) {
                continue;
            }
            if let Some(branches) = self.extract_compact_and(meta, *mv) {
                return Some(ProofNode::Attack { mv: *mv, branches });
            }
        }
        None
    }

    fn extract_compact_and(
        &mut self,
        meta: &MetaPosition,
        mv: Move,
    ) -> Option<Vec<(ProofObs, ProofNode)>> {
        let (legal_meta, illegal_meta) = meta.apply_attack_move_split(mv);
        let mut branches = Vec::new();

        if !illegal_meta.is_empty() {
            let node = self.extract_compact_or(&illegal_meta)?;
            branches.push((ProofObs::Illegal, node));
        }
        if legal_meta.is_empty() {
            return if branches.is_empty() { None } else { Some(branches) };
        }
        if legal_meta.all_effectively_checkmate() {
            branches.push((ProofObs::Checkmate, ProofNode::Checkmate));
            return Some(branches);
        }

        let raw_obs_branches = legal_meta.expand_defense_moves(mv);
        if self.is_cancelled() {
            // 協調キャンセルによる部分展開を証明木に使わない
            return None;
        }
        let obs_branches = Self::merge_observation_branches(raw_obs_branches);
        for (obs, branch_meta) in obs_branches {
            match obs {
                Observation::Checkmate => {
                    branches.push((ProofObs::Checkmate, ProofNode::Checkmate));
                }
                Observation::Captured { file, rank } => {
                    let node = self.extract_compact_or(&branch_meta)?;
                    branches.push((ProofObs::Captured(file, rank), node));
                }
                Observation::NoCapture => {
                    let node = self.extract_compact_or(&branch_meta)?;
                    branches.push((ProofObs::NoCapture, node));
                }
                Observation::Illegal => {}
            }
        }
        Some(branches)
    }

    // ========================================================================
    // 証明木リプレイ
    // ========================================================================

    /// 証明木のリプレイを試みる
    /// 成功: Some(proof_hand) — 転置表を更新済み
    /// 失敗: None — フォールバック
    fn try_replay_proof(
        &mut self,
        meta: &MetaPosition,
        proof: &ProofNode,
        hash: u64,
        path: &[u64],
        depth: u32,
    ) -> Option<[u8; 7]> {
        // ループ検出
        if path.contains(&hash) { return None; }
        // 深さ制限チェック
        if let Some(limit) = self.depth_limit {
            if depth >= limit { return None; }
        }
        // should_stop チェック
        if self.should_stop() { return None; }

        match proof {
            ProofNode::Checkmate => {
                if meta.all_effectively_checkmate() {
                    let proof_hand = [0u8; 7];
                    self.store_or(hash, 0, INF, 0);
                    self.or_proof_hands.insert(hash, proof_hand);
                    Some(proof_hand)
                } else {
                    None
                }
            }
            ProofNode::Attack { mv, branches: proof_branches } => {
                // 高速分割（全合法手生成をスキップ）
                let (legal_meta, illegal_meta) = meta.apply_attack_move_split_fast(*mv);

                // 全合法盤面で王手がかかっているか（毎手王手の要件）
                if !legal_meta.is_empty()
                    && !legal_meta.positions.iter()
                        .all(|pos| pos.is_in_check(pos.side_to_move))
                {
                    return None;
                }

                // 実際の観測分岐を構築
                let mut actual_branches: Vec<(Observation, MetaPosition)> = Vec::new();

                // Illegal 分岐
                if !illegal_meta.is_empty() {
                    actual_branches.push((Observation::Illegal, illegal_meta));
                }

                // 合法分岐
                if !legal_meta.is_empty() {
                    if legal_meta.all_effectively_checkmate() {
                        actual_branches.push((Observation::Checkmate, MetaPosition { positions: Vec::new() }));
                    } else {
                        let raw_obs = legal_meta.expand_defense_moves(*mv);
                        if self.is_cancelled() {
                            // 協調キャンセルによる部分展開をリプレイに使わない
                            // （不完全な分岐で store_or に偽の証明を書かないため）
                            return None;
                        }
                        let obs = Self::merge_observation_branches(raw_obs);
                        actual_branches.extend(obs);
                    }
                }

                if actual_branches.is_empty() { return None; }

                // 各実際の分岐に対応する証明分岐を探してリプレイ
                // 部分リプレイ: 一部の分岐が失敗しても、成功した分岐の結果は
                // 転置表に残す（再帰呼び出し内で or_table に記録される）。
                // これにより、フォールバック時の df-pn で成功済みの分岐をスキップできる。
                let parent_hand = meta.positions.first()
                    .map(|p| p.hand(Color::Sente).counts_array())
                    .unwrap_or([0; 7]);
                let mut and_ph = [0u8; 7];
                let mut k_and: u32 = 1; // 証明深さ（攻め方の1手 + 分岐の最大）
                let mut all_succeeded = true;

                for (actual_obs, actual_meta) in &actual_branches {
                    // 証明分岐から一致するもの全てを収集（sente_hand_hash グループ化対応）
                    let matching_proofs: Vec<&ProofNode> = proof_branches.iter()
                        .filter(|(po, _)| po.matches(actual_obs))
                        .map(|(_, sp)| sp)
                        .collect();
                    if matching_proofs.is_empty() {
                        all_succeeded = false;
                        continue;
                    }

                    match actual_obs {
                        Observation::Checkmate => {
                            // Checkmate は proof_hand 寄与 0
                        }
                        Observation::Illegal => {
                            let child_hash = meta_position_hash(actual_meta);
                            let mut child_path = path.to_vec();
                            child_path.push(hash);
                            let mut branch_ok = false;
                            for sub_proof in &matching_proofs {
                                if let Some(child_ph) = self.try_replay_proof(
                                    actual_meta, sub_proof, child_hash, &child_path, depth,
                                ) {
                                    for j in 0..7 {
                                        let eff = (child_ph[j] as i16 + parent_hand[j] as i16
                                            - parent_hand[j] as i16).max(0) as u8;
                                        and_ph[j] = and_ph[j].max(eff);
                                    }
                                    let child_k = self
                                        .or_table
                                        .get(&child_hash)
                                        .and_then(|e| e.proven_bound())
                                        .unwrap_or_else(|| self.current_budget(depth));
                                    k_and = k_and.max(child_k);
                                    branch_ok = true;
                                    break;
                                }
                            }
                            if !branch_ok { all_succeeded = false; }
                        }
                        Observation::Captured { .. } | Observation::NoCapture => {
                            let child_hash = meta_position_hash(actual_meta);
                            let child_hand = actual_meta.positions.first()
                                .map(|p| p.hand(Color::Sente).counts_array())
                                .unwrap_or(parent_hand);
                            let mut child_path = path.to_vec();
                            child_path.push(hash);
                            let mut branch_ok = false;
                            for sub_proof in &matching_proofs {
                                if let Some(child_ph) = self.try_replay_proof(
                                    actual_meta, sub_proof, child_hash, &child_path, depth + 2,
                                ) {
                                    for j in 0..7 {
                                        let eff = (child_ph[j] as i16 + parent_hand[j] as i16
                                            - child_hand[j] as i16).max(0) as u8;
                                        and_ph[j] = and_ph[j].max(eff);
                                    }
                                    let child_k = self
                                        .or_table
                                        .get(&child_hash)
                                        .and_then(|e| e.proven_bound())
                                        .unwrap_or_else(|| self.current_budget(depth + 2));
                                    k_and = k_and.max(child_k.saturating_add(2));
                                    branch_ok = true;
                                    break;
                                }
                            }
                            if !branch_ok { all_succeeded = false; }
                        }
                    }
                }

                // move_hints は部分成功でも記録（df-pn フォールバック時に候補手ソートで有効）
                if !meta.positions.is_empty() {
                    let board_only_hash = meta_board_only_hash(meta);
                    self.move_hints.entry(board_only_hash).or_insert(*mv);
                }

                if all_succeeded {
                    // 全分岐成功 → OR ノードの proof_hand
                    let or_ph = and_ph;

                    self.store_or(hash, 0, INF, k_and);
                    self.or_proof_hands.insert(hash, or_ph);
                    self.record_proven_dominance(meta, or_ph, k_and);

                    // and_table にも記録
                    let and_key = Self::and_key(hash, mv);
                    self.store_and(and_key, 0, INF, k_and);
                    self.and_proof_hands.insert(and_key, and_ph);

                    Some(or_ph)
                } else {
                    // 部分失敗: 成功したサブ分岐は再帰呼び出し内で転置表に記録済み
                    None
                }
            }
        }
    }
}

#[cfg(test)]
mod move_restore_tests {
    use super::*;

    /// 後手玉5一、先手金5三、先手番の情報集合
    fn single_meta() -> MetaPosition {
        let mut pos = Position::new();
        pos.set_piece(Square::new(5, 1), Piece::new(Color::Gote, PieceKind::King));
        pos.set_piece(Square::new(5, 3), Piece::new(Color::Sente, PieceKind::Gold));
        pos.side_to_move = Color::Sente;
        pos.init_zobrist_hash();
        MetaPosition::new(pos)
    }

    /// 解の手順木の MoveData から復元した「盤上の駒の移動」が、
    /// 元の手と同じように情報集合を分割できること。
    ///
    /// `Move` の Eq/Hash は `moved_piece_kind` を含むため、駒種を落として
    /// 復元した手は `Position::is_legal_move`（生成した手との比較で判定）で
    /// 全局面が不合法になる。走査系（余詰めチェックの内部ノード探索）は
    /// これに気づかず「分岐なし」として静かに進むので、単体で固定しておく。
    #[test]
    fn md_to_move_round_trips_board_moves() {
        let meta = single_meta();
        let mv = Move::normal(
            Square::new(5, 3),
            Square::new(5, 2),
            false,
            PieceKind::Gold,
        );
        let md = MoveData::from_move(mv, Color::Sente);

        let restored = md_to_move(&md, &meta).expect("盤上手を復元できるはず");
        assert_eq!(restored, mv, "md_to_move は駒種まで復元する");

        let (legal, illegal) = meta.apply_attack_move_split(restored);
        assert_eq!(legal.positions.len(), 1, "復元した手は指せるはず");
        assert!(illegal.is_empty());
    }

    /// 除外キーは駒種を持たないが、成/不成まで含めて元の手に一致すること。
    /// （`ExclusionKey` は指せない型なので、情報集合に適用する取り違えは
    /// 型レベルで起きない。ここで見るのは照合が効くことだけ）
    #[test]
    fn exclusion_key_matches_original_move() {
        for promotion in [false, true] {
            let mv = Move::normal(Square::new(5, 3), Square::new(5, 2), promotion, PieceKind::Gold);
            let key = ExclusionKey::from_move_data(&MoveData::from_move(mv, Color::Sente));

            assert!(key.matches(&mv), "元の手に一致する");
            assert!(key.same_target(&mv));

            let counterpart = key.promotion_counterpart().expect("盤上手には成/不成の相方がある");
            assert!(!counterpart.matches(&mv), "成/不成が違えば matches は外れる");
            assert!(counterpart.same_target(&mv), "same_target は成/不成を無視する");
        }

        // 打ちには成/不成の相方がない
        let drop = Move::drop(Square::new(5, 2), PieceKind::Gold);
        let key = ExclusionKey::from_move_data(&MoveData::from_move(drop, Color::Sente));
        assert!(key.matches(&drop));
        assert!(key.promotion_counterpart().is_none());
    }
}
