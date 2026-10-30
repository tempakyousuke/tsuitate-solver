//! ついたて詰み逃れ（受け方が衝立の向こうの王手から逃れる問題）の探索。
//!
//! ついたて詰将棋の**裏返し**。初期局面は両者とも全公開だが、以後の攻め方の手は
//! 受け方から見えない。受け方は自分の手の受理/反則・取った駒の種類・自駒が
//! 取られたマス・王手の宣言だけを手がかりに逃げる。
//!
//! ## 攻め方は「観測を変えない範囲で過去の手を改変できる」
//!
//! 攻め方の手を1つに固定せず、受け方の観測と矛盾しない**局面の集合**
//! （情報集合）を丸ごと保持する。攻め方はその中から都合のよい分岐を後出しで
//! 選べる（＝受け方の観測履歴を変えない範囲で、途中に指した手を差し替えられる）。
//! 局面を多く残すほど攻め方の選択肢が増えるので、攻め方にとっては集合を
//! 丸ごと残すのが常に最善になる（集合が大きいほど攻め方は弱くならない）。
//!
//! ## 勝敗
//!
//! - **受け方の勝ち（逃れ）**: 攻め方の手番で、情報集合のどの局面にも王手が無い
//! - **攻め方の勝ち（詰み）**: 情報集合のどこかの局面に、受け方を詰ます王手がある
//! - **攻め方の勝ち（反則負け）**: 受け方の手がどこかの局面で反則になり得て、
//!   許された反則の残りが 0
//! - **攻め方の勝ち（手数切れ）**: 攻め方の王手の回数が上限に達した後も、まだ
//!   王手できる。連続王手で逃げ回るだけでは勝ちにならない（勝利条件は
//!   「攻め方が王手できなくなること」）。上限は探索を有限にするための予算でもある
//!
//! ## 局面集合の分割（観測）
//!
//! - 攻め方の手番: 全局面の全ての王手を指し、「取られた自駒のマス（無ければ無し）」で
//!   分ける。王手は必ず宣言されるので観測は取られたマスだけ
//! - 受け方の手番: 手 m を全局面に当て、不可能な局面（反則）と、可能な局面を
//!   「取った駒の種類」「m で攻め方に王手をかけたか（双玉のときだけ分かれる）」で分ける
//!
//! 受け方自身の駒と持ち駒は観測から常に正確に分かる（取られたマスは通知され、
//! 取った駒の種類も分かる）ので、情報集合の全局面で受け方の駒は一致する。
//! 分かれるのは攻め方の駒と持ち駒だけ。
//!
//! ## 探索
//!
//! 受け方の手番が OR（どれか1手で逃れればよい）、攻め方の分岐選択が AND
//! （どの分岐を選ばれても逃れる必要がある）の深さ制限つき AND/OR 探索。
//! 深さは攻め方の残り王手回数で数える。決定性のため時間ではなくノード数で打ち切り、
//! 打ち切りは `Verdict::Unknown` になる。
//!
//! 受け方の「どの局面でも反則になる手」は探索しない。反則の残りを1つ減らして
//! 同じ集合に戻るだけなので、反則の残りが多いほど受け方が有利（単調）である以上、
//! 指さない手より良くなることは無い。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::fasthash::{FxHashMap, FxHashSet};
use super::tsuitate_dfpn::{compute_discovery_squares, could_give_check, drop_check_mask};
use crate::shogi::movegen::{generate_board_moves, generate_drop_moves_masked};
use crate::shogi::position::Position;
use crate::shogi::types::*;

/// 受け方から見た判定
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// 受け方は予算内に逃れを強制できる
    Escape,
    /// 攻め方は予算内に受け方を負かせる（詰み・反則負け・手数切れ）
    Caught,
    /// ノード上限・キャンセルで判定できなかった
    Unknown,
}

/// 情報集合（観測と矛盾しない局面の集合）。局面は Zobrist ハッシュで重複排除し、
/// ハッシュ順に並べる（集合のハッシュを並び順に依存させないため）
#[derive(Debug, Clone)]
pub struct InfoSet {
    pub positions: Vec<Position>,
    hash: u64,
}

impl InfoSet {
    pub fn new(mut positions: Vec<Position>) -> Self {
        positions.sort_by_key(|p| p.zobrist_hash);
        positions.dedup_by_key(|p| p.zobrist_hash);
        let mut hash: u64 = 0x2545_F491_4F6C_DD1D ^ positions.len() as u64;
        for p in &positions {
            hash = (hash.rotate_left(23) ^ p.zobrist_hash).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        }
        Self { positions, hash }
    }

    /// Zobrist ハッシュ未設定の局面（JSON から作った直後など）から作る
    pub fn from_fresh(mut positions: Vec<Position>) -> Self {
        for p in &mut positions {
            p.init_zobrist_hash();
        }
        Self::new(positions)
    }

    pub fn len(&self) -> usize {
        self.positions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    pub fn hash(&self) -> u64 {
        self.hash
    }
}

/// 攻め方の手番の展開結果
#[derive(Debug)]
pub enum AttackExpansion {
    /// どの局面にも王手が無い（受け方の逃れ）
    NoCheck,
    /// どこかの局面に受け方を詰ます王手がある
    Mate,
    /// 「取られた自駒のマス」ごとの情報集合（受け方の手番）
    Groups(Vec<(Option<Square>, InfoSet)>),
}

/// 受け方の手 m を情報集合に当てた結果
#[derive(Debug)]
pub struct DefenseSplit {
    /// m が不可能だった局面（m を指す前のまま）
    pub foul: Option<InfoSet>,
    /// m が受理された局面を (取った駒の種類, 攻め方に王手をかけたか) で分けたもの。
    /// 局面は m を指した後（攻め方の手番）
    pub accepted: Vec<(Option<PieceKind>, bool, InfoSet)>,
}

/// 手番側（攻め方）の王手になる合法手を全て返す
pub fn checking_moves(pos: &Position) -> Vec<Move> {
    let attacker = pos.side_to_move;
    let Some(ksq) = pos.find_king(attacker.opponent()) else {
        return Vec::new();
    };
    let discovery_sqs = compute_discovery_squares(pos, ksq, attacker);
    let has_own_king = pos.find_king(attacker).is_some();

    let mut raw = Vec::new();
    generate_board_moves(pos, attacker, &mut raw);
    generate_drop_moves_masked(pos, attacker, |kind| drop_check_mask(attacker, kind, ksq), &mut raw);

    let mut test = pos.clone();
    let mut checks = Vec::new();
    for mv in raw {
        // 玉を取る手は王手ではない（そもそも合法な局面では起こらない）
        if mv.to == ksq {
            continue;
        }
        if !could_give_check(&mv, ksq, &discovery_sqs, attacker) {
            continue;
        }
        let undo = test.make_move(mv);
        let is_check =
            test.is_attacked(ksq, attacker) && !(has_own_king && test.is_in_check(attacker));
        test.unmake_move(&undo);
        if is_check {
            checks.push(mv);
        }
    }
    checks
}

/// 手番側が王手されていて、合法手が1つも無い
fn is_mated(pos: &Position) -> bool {
    pos.is_in_check(pos.side_to_move) && pos.generate_check_evasions().is_empty()
}

/// 攻め方の手番の情報集合を、全王手で展開して観測ごとに分ける
pub fn expand_attack(set: &InfoSet) -> AttackExpansion {
    let mut groups: Vec<(Option<Square>, Vec<Position>)> = Vec::new();
    let mut seen: FxHashSet<u64> = FxHashSet::default();
    for pos in &set.positions {
        for mv in checking_moves(pos) {
            let captured = if mv.drop_piece.is_none() {
                pos.piece_at(mv.to).map(|_| mv.to)
            } else {
                None
            };
            let mut next = pos.clone();
            next.make_move(mv);
            if is_mated(&next) {
                return AttackExpansion::Mate;
            }
            if !seen.insert(next.zobrist_hash) {
                continue;
            }
            match groups.iter_mut().find(|(sq, _)| *sq == captured) {
                Some((_, list)) => list.push(next),
                None => groups.push((captured, vec![next])),
            }
        }
    }
    if groups.is_empty() {
        return AttackExpansion::NoCheck;
    }
    // 観測の並びを決定的にする（取られなかった分岐が先、以降はマス順）
    groups.sort_by_key(|(sq, _)| sq.map(|s| s.index() as i32).unwrap_or(-1));
    AttackExpansion::Groups(groups.into_iter().map(|(sq, ps)| (sq, InfoSet::new(ps))).collect())
}

/// 受け方の候補手（情報集合のどこかの局面で合法な手の和集合）。並びは決定的
pub fn defense_candidates(set: &InfoSet) -> Vec<Move> {
    let mut seen: FxHashSet<Move> = FxHashSet::default();
    let mut moves = Vec::new();
    for pos in &set.positions {
        for mv in pos.generate_check_evasions() {
            if seen.insert(mv) {
                moves.push(mv);
            }
        }
    }
    moves
}

/// 受け方の手 m を情報集合に当てて、反則と受理（観測ごと）に分ける
pub fn split_defense(set: &InfoSet, mv: &Move) -> DefenseSplit {
    let mut foul = Vec::new();
    let mut accepted: Vec<(Option<PieceKind>, bool, Vec<Position>)> = Vec::new();
    for pos in &set.positions {
        let defender = pos.side_to_move;
        match pos.try_make_legal(mv) {
            None => foul.push(pos.clone()),
            Some(next) => {
                let captured = if mv.drop_piece.is_none() {
                    pos.piece_at(mv.to).map(|p| p.kind.unpromoted())
                } else {
                    None
                };
                let check = next.is_in_check(defender.opponent());
                match accepted.iter_mut().find(|(k, c, _)| *k == captured && *c == check) {
                    Some((_, _, list)) => list.push(next),
                    None => accepted.push((captured, check, vec![next])),
                }
            }
        }
    }
    accepted.sort_by_key(|(k, c, _)| (k.map(hand_order).unwrap_or(0), *c));
    DefenseSplit {
        foul: if foul.is_empty() { None } else { Some(InfoSet::new(foul)) },
        accepted: accepted.into_iter().map(|(k, c, ps)| (k, c, InfoSet::new(ps))).collect(),
    }
}

fn hand_order(kind: PieceKind) -> u8 {
    match kind {
        PieceKind::Pawn => 1,
        PieceKind::Lance => 2,
        PieceKind::Knight => 3,
        PieceKind::Silver => 4,
        PieceKind::Gold => 5,
        PieceKind::Bishop => 6,
        PieceKind::Rook => 7,
        _ => 8,
    }
}

/// 転置表の1件: (反則の残り, 王手の残り予算, 逃れたか)
type TtEntry = (u8, u32, bool);

/// 転置表がこれを超えたら捨てる（メモリの頭打ち）
const MAX_TT_KEYS: usize = 2_000_000;

/// 詰み逃れの探索器。転置表（証明・反証の恒久的事実）を問い合わせをまたいで
/// 温存できるので、常駐モードでは使い回す
pub struct EscapeSolver {
    /// (集合ハッシュ, 手番の種類) → 判定済みの (反則の残り, 予算, 逃れ)
    tt: FxHashMap<(u64, u8), Vec<TtEntry>>,
    pub nodes: u64,
    node_limit: u64,
    cancel: Arc<AtomicBool>,
    aborted: bool,
}

const ATTACK: u8 = 0;
const DEFENSE: u8 = 1;

impl EscapeSolver {
    pub fn new(node_limit: u64, cancel: Arc<AtomicBool>) -> Self {
        Self { tt: FxHashMap::default(), nodes: 0, node_limit, cancel, aborted: false }
    }

    /// ノード上限を設定する（累計 nodes に対する絶対値）
    pub fn set_node_limit(&mut self, node_limit: u64) {
        self.node_limit = node_limit;
    }

    fn tick(&mut self) -> bool {
        self.nodes += 1;
        if self.nodes > self.node_limit || self.cancel.load(Ordering::Relaxed) {
            self.aborted = true;
        }
        self.aborted
    }

    /// 単調性つきの転置表参照。逃れは (反則の残り, 予算) が多いほど成り立ち、
    /// 捕まりは少ないほど成り立つ
    fn tt_lookup(&self, hash: u64, kind: u8, fouls: u8, budget: u32) -> Option<bool> {
        let entries = self.tt.get(&(hash, kind))?;
        for &(f, b, escape) in entries {
            if escape && f <= fouls && b <= budget {
                return Some(true);
            }
            if !escape && f >= fouls && b >= budget {
                return Some(false);
            }
        }
        None
    }

    fn tt_store(&mut self, hash: u64, kind: u8, fouls: u8, budget: u32, escape: bool) {
        if self.tt.len() >= MAX_TT_KEYS {
            self.tt.clear();
        }
        let entries = self.tt.entry((hash, kind)).or_default();
        // 新しい事実に含意される古い事実は捨てる
        entries.retain(|&(f, b, e)| {
            if e != escape {
                return true;
            }
            if escape {
                !(fouls <= f && budget <= b)
            } else {
                !(fouls >= f && budget >= b)
            }
        });
        entries.push((fouls, budget, escape));
    }

    /// 攻め方の手番。budget は攻め方がまだ指せる王手の回数
    pub fn attack(&mut self, set: &InfoSet, fouls: u8, budget: u32) -> Verdict {
        if let Some(escape) = self.tt_lookup(set.hash(), ATTACK, fouls, budget) {
            return if escape { Verdict::Escape } else { Verdict::Caught };
        }
        if self.tick() {
            return Verdict::Unknown;
        }
        let verdict = match expand_attack(set) {
            AttackExpansion::NoCheck => Verdict::Escape,
            AttackExpansion::Mate => Verdict::Caught,
            AttackExpansion::Groups(_) if budget == 0 => Verdict::Caught,
            AttackExpansion::Groups(mut groups) => {
                // 局面の多い分岐ほど攻め方に有利なので先に調べる（早く捕まえて打ち切る）
                groups.sort_by_key(|(_, g)| std::cmp::Reverse(g.len()));
                let mut result = Verdict::Escape;
                for (_, group) in &groups {
                    match self.defense(group, fouls, budget - 1) {
                        Verdict::Caught => {
                            result = Verdict::Caught;
                            break;
                        }
                        Verdict::Unknown => result = Verdict::Unknown,
                        Verdict::Escape => {}
                    }
                }
                result
            }
        };
        if verdict != Verdict::Unknown {
            self.tt_store(set.hash(), ATTACK, fouls, budget, verdict == Verdict::Escape);
        }
        verdict
    }

    /// 受け方の手番（王手されている）。budget は攻め方がまだ指せる王手の回数
    pub fn defense(&mut self, set: &InfoSet, fouls: u8, budget: u32) -> Verdict {
        if let Some(escape) = self.tt_lookup(set.hash(), DEFENSE, fouls, budget) {
            return if escape { Verdict::Escape } else { Verdict::Caught };
        }
        if self.tick() {
            return Verdict::Unknown;
        }

        let mut options: Vec<(Move, DefenseSplit)> = Vec::new();
        for mv in defense_candidates(set) {
            let split = split_defense(set, &mv);
            // どの局面でも受理されない手（確定反則）は指す意味がない
            if split.accepted.is_empty() {
                continue;
            }
            // 反則の残りが無いのに反則になり得る手は即負け
            if split.foul.is_some() && fouls == 0 {
                continue;
            }
            options.push((mv, split));
        }
        // 反則の心配が無く、受理後の集合が小さい手から試す（逃れが見つかれば打ち切る）
        options.sort_by_key(|(_, s)| {
            let foul = s.foul.as_ref().map_or(0, |f| f.len());
            let largest = s.accepted.iter().map(|(_, _, g)| g.len()).max().unwrap_or(0);
            (foul > 0, largest, foul)
        });

        let mut any_unknown = false;
        let mut verdict = Verdict::Caught;
        for (_, split) in &options {
            match self.evaluate_split(split, fouls, budget) {
                Verdict::Escape => {
                    verdict = Verdict::Escape;
                    break;
                }
                Verdict::Unknown => any_unknown = true,
                Verdict::Caught => {}
            }
            if self.aborted {
                any_unknown = true;
                break;
            }
        }
        if verdict != Verdict::Escape && any_unknown {
            verdict = Verdict::Unknown;
        }
        if verdict != Verdict::Unknown {
            self.tt_store(set.hash(), DEFENSE, fouls, budget, verdict == Verdict::Escape);
        }
        verdict
    }

    /// 受け方の1手について、攻め方がどの分岐を選んでも逃れられるか
    pub fn evaluate_split(&mut self, split: &DefenseSplit, fouls: u8, budget: u32) -> Verdict {
        let mut result = Verdict::Escape;
        if let Some(foul) = &split.foul {
            if fouls == 0 {
                return Verdict::Caught;
            }
            match self.defense(foul, fouls - 1, budget) {
                Verdict::Caught => return Verdict::Caught,
                Verdict::Unknown => result = Verdict::Unknown,
                Verdict::Escape => {}
            }
        }
        let mut order: Vec<&InfoSet> = split.accepted.iter().map(|(_, _, g)| g).collect();
        order.sort_by_key(|g| std::cmp::Reverse(g.len()));
        for group in order {
            match self.attack(group, fouls, budget) {
                Verdict::Caught => return Verdict::Caught,
                Verdict::Unknown => result = Verdict::Unknown,
                Verdict::Escape => {}
            }
        }
        result
    }

    /// 反復深化: 予算 0..=max_budget の順に調べ、逃れられる最小の予算を返す。
    /// 戻り値の Option は Escape のときだけ Some
    pub fn solve_min_budget(
        &mut self,
        set: &InfoSet,
        attack_to_move: bool,
        fouls: u8,
        max_budget: u32,
    ) -> (Verdict, Option<u32>) {
        for budget in 0..=max_budget {
            self.aborted = false;
            let v = if attack_to_move {
                self.attack(set, fouls, budget)
            } else {
                self.defense(set, fouls, budget)
            };
            match v {
                Verdict::Escape => return (Verdict::Escape, Some(budget)),
                Verdict::Unknown => return (Verdict::Unknown, None),
                Verdict::Caught => {}
            }
        }
        (Verdict::Caught, None)
    }
}

/// 問題の検証結果
#[derive(Debug, Clone)]
pub struct EscapeAnalysis {
    pub verdict: Verdict,
    /// 逃れに必要な最少の反則回数（Escape のときだけ）
    pub min_fouls: Option<u8>,
    /// その反則回数で逃れるのに要する攻め方の王手の回数（最悪の場合）
    pub checks: Option<u32>,
    pub nodes: u64,
    /// 最初の王手の後に考えられる局面の数（攻め方の初手の可能性の広さ）
    pub first_check_positions: usize,
    /// 最初の王手の観測の分岐数
    pub first_check_groups: usize,
    /// 受け方の初手について、攻め方に最も都合のよい分岐での
    /// (候補手の数, そのうち逃れられる手の数)。逃れられない・不明のときは None
    pub root_tries: Option<u32>,
    pub root_solutions: Option<u32>,
}

/// 初期局面（攻め方の手番）から、反則 0..=max_fouls 回・王手 max_checks 回以内で
/// 逃れられるかを調べ、逃れに必要な最少の反則回数と王手の回数を求める
pub fn analyze_escape(
    root: Position,
    max_fouls: u8,
    max_checks: u32,
    node_limit: u64,
    cancel: Arc<AtomicBool>,
) -> EscapeAnalysis {
    let set = InfoSet::from_fresh(vec![root]);
    let mut solver = EscapeSolver::new(node_limit, cancel);

    let (first_check_positions, first_check_groups) = match expand_attack(&set) {
        AttackExpansion::Groups(groups) => {
            (groups.iter().map(|(_, g)| g.len()).sum(), groups.len())
        }
        _ => (0, 0),
    };

    let mut analysis = EscapeAnalysis {
        verdict: Verdict::Caught,
        min_fouls: None,
        checks: None,
        nodes: 0,
        first_check_positions,
        first_check_groups,
        root_tries: None,
        root_solutions: None,
    };

    for fouls in 0..=max_fouls {
        let (verdict, budget) = solver.solve_min_budget(&set, true, fouls, max_checks);
        match verdict {
            Verdict::Escape => {
                analysis.verdict = Verdict::Escape;
                analysis.min_fouls = Some(fouls);
                analysis.checks = budget;
                break;
            }
            Verdict::Unknown => {
                analysis.verdict = Verdict::Unknown;
                break;
            }
            Verdict::Caught => {}
        }
    }

    if let (Some(fouls), Some(checks)) = (analysis.min_fouls, analysis.checks) {
        if checks > 0 {
            if let Some((tries, solutions)) = root_move_stats(&mut solver, &set, fouls, checks) {
                analysis.root_tries = Some(tries);
                analysis.root_solutions = Some(solutions);
            }
        }
    }
    analysis.nodes = solver.nodes;
    analysis
}

/// 最初の王手の各分岐で受け方の候補手を1手ずつ調べ、逃れられる手の割合が
/// 最も小さい分岐の (候補手の数, 逃れられる手の数) を返す。
/// 攻め方はその分岐を選べるので、受け方が最初に直面する難しさの目安になる
fn root_move_stats(
    solver: &mut EscapeSolver,
    root: &InfoSet,
    fouls: u8,
    checks: u32,
) -> Option<(u32, u32)> {
    let AttackExpansion::Groups(groups) = expand_attack(root) else {
        return None;
    };
    let mut worst: Option<(u32, u32)> = None;
    for (_, group) in &groups {
        let mut tries = 0u32;
        let mut solutions = 0u32;
        for mv in defense_candidates(group) {
            let split = split_defense(group, &mv);
            if split.accepted.is_empty() {
                continue;
            }
            tries += 1;
            solver.aborted = false;
            match solver.evaluate_split(&split, fouls, checks - 1) {
                Verdict::Escape => solutions += 1,
                Verdict::Unknown => return None,
                Verdict::Caught => {}
            }
        }
        let worse = match worst {
            None => true,
            // solutions/tries の小さい方（交差乗算で比較）
            Some((t, s)) => (solutions as u64) * (t as u64) < (s as u64) * (tries as u64),
        };
        if worse {
            worst = Some((tries, solutions));
        }
    }
    worst
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sq(file: u8, rank: u8) -> Square {
        Square::new(file, rank)
    }

    /// ユーザー提示の例: 受け方（先手）の玉5九だけ、攻め方（後手）は持ち駒に金1枚。
    /// 攻め方の手番から始まる
    fn lone_king_vs_gold() -> Position {
        let mut pos = Position::new();
        pos.set_piece(sq(5, 9), Piece::new(Color::Sente, PieceKind::King));
        pos.gote_hand.add(PieceKind::Gold);
        pos.side_to_move = Color::Gote;
        pos.init_zobrist_hash();
        pos
    }

    fn king_move(from: Square, to: Square) -> Move {
        Move::normal(from, to, false, PieceKind::King)
    }

    fn no_cancel() -> Arc<AtomicBool> {
        Arc::new(AtomicBool::new(false))
    }

    #[test]
    fn 金の王手は5通り() {
        let set = InfoSet::from_fresh(vec![lone_king_vs_gold()]);
        match expand_attack(&set) {
            AttackExpansion::Groups(groups) => {
                assert_eq!(groups.len(), 1);
                assert_eq!(groups[0].0, None);
                assert_eq!(groups[0].1.len(), 5); // 4八・5八・6八・4九・6九
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    #[test]
    fn 反則1回なら逃れられ0回では逃れられない() {
        let root = lone_king_vs_gold();
        let analysis = analyze_escape(root, 3, 6, 1_000_000, no_cancel());
        assert_eq!(analysis.verdict, Verdict::Escape);
        assert_eq!(analysis.min_fouls, Some(1));
        // 例の手順: 王手 → 4八玉（反則）→ 5八玉（何も取れず）→ 王手 → 6七玉で逃れ
        assert_eq!(analysis.checks, Some(2));
        assert_eq!(analysis.first_check_positions, 5);
    }

    #[test]
    fn 例の手順をたどる() {
        let set = InfoSet::from_fresh(vec![lone_king_vs_gold()]);
        let AttackExpansion::Groups(groups) = expand_attack(&set) else { panic!() };
        let s1 = &groups[0].1;

        // 4八玉は 5八金・4九金 の局面で反則
        let split = split_defense(s1, &king_move(sq(5, 9), sq(4, 8)));
        let foul = split.foul.expect("反則の分岐がある");
        assert_eq!(foul.len(), 2);

        // 反則の残り 0 で 4九玉・6八玉・6九玉 は 5八金 の局面で反則になり得るので負け
        let mut solver = EscapeSolver::new(1_000_000, no_cancel());
        for to in [sq(4, 9), sq(6, 8), sq(6, 9)] {
            let split = split_defense(&foul, &king_move(sq(5, 9), to));
            assert!(split.foul.is_some(), "{:?}", to);
            assert_eq!(solver.evaluate_split(&split, 0, 1), Verdict::Caught);
        }

        // 5八玉はどちらの局面でも指せて、取れたら金が無くなり、取れなければ金は4九
        let split = split_defense(&foul, &king_move(sq(5, 9), sq(5, 8)));
        assert!(split.foul.is_none());
        assert_eq!(split.accepted.len(), 2);
        assert_eq!(solver.evaluate_split(&split, 0, 1), Verdict::Escape);

        // 何も取れなかった分岐: 次の王手は 4八金 か 5九金
        let (_, _, no_capture) =
            split.accepted.iter().find(|(k, _, _)| k.is_none()).expect("取れない分岐");
        let AttackExpansion::Groups(next) = expand_attack(no_capture) else { panic!() };
        assert_eq!(next.iter().map(|(_, g)| g.len()).sum::<usize>(), 2);
        // 6七玉と逃げれば王手がかからない
        let split = split_defense(&next[0].1, &king_move(sq(5, 8), sq(6, 7)));
        assert!(split.foul.is_none());
        for (_, _, g) in &split.accepted {
            assert!(matches!(expand_attack(g), AttackExpansion::NoCheck));
        }
    }

    #[test]
    fn 王手が無ければ即逃れ() {
        let mut pos = Position::new();
        pos.set_piece(sq(5, 9), Piece::new(Color::Sente, PieceKind::King));
        pos.set_piece(sq(1, 1), Piece::new(Color::Gote, PieceKind::Pawn));
        pos.side_to_move = Color::Gote;
        let analysis = analyze_escape(pos, 0, 3, 100_000, no_cancel());
        assert_eq!(analysis.verdict, Verdict::Escape);
        assert_eq!(analysis.min_fouls, Some(0));
        assert_eq!(analysis.checks, Some(0));
    }

    #[test]
    fn 詰む王手があれば捕まる() {
        // 頭金: 玉5九の周りは自駒で塞がっていて、後手の歩5七が支える 5八金打で詰む
        let mut pos = Position::new();
        pos.set_piece(sq(5, 9), Piece::new(Color::Sente, PieceKind::King));
        pos.set_piece(sq(5, 7), Piece::new(Color::Gote, PieceKind::Pawn));
        pos.set_piece(sq(4, 9), Piece::new(Color::Sente, PieceKind::Lance));
        pos.set_piece(sq(6, 9), Piece::new(Color::Sente, PieceKind::Lance));
        pos.set_piece(sq(4, 8), Piece::new(Color::Sente, PieceKind::Pawn));
        pos.set_piece(sq(6, 8), Piece::new(Color::Sente, PieceKind::Pawn));
        pos.gote_hand.add(PieceKind::Gold);
        pos.side_to_move = Color::Gote;
        let set = InfoSet::from_fresh(vec![pos.clone()]);
        assert!(matches!(expand_attack(&set), AttackExpansion::Mate));
        let analysis = analyze_escape(pos, 3, 4, 100_000, no_cancel());
        assert_eq!(analysis.verdict, Verdict::Caught);
        assert_eq!(analysis.min_fouls, None);
    }

    #[test]
    fn 王手し続けられると手数切れで捕まる() {
        // 盤に後手の竜だけ。受け方の玉は広いが、竜は王手を続けられる
        let mut pos = Position::new();
        pos.set_piece(sq(5, 5), Piece::new(Color::Sente, PieceKind::King));
        pos.set_piece(sq(1, 1), Piece::new(Color::Gote, PieceKind::PromotedRook));
        pos.side_to_move = Color::Gote;
        let analysis = analyze_escape(pos, 0, 2, 2_000_000, no_cancel());
        assert_ne!(analysis.verdict, Verdict::Escape);
    }

    #[test]
    fn ノード上限で不明になる() {
        let root = lone_king_vs_gold();
        let analysis = analyze_escape(root, 3, 6, 3, no_cancel());
        assert_eq!(analysis.verdict, Verdict::Unknown);
    }

    #[test]
    fn 集合のハッシュは並び順に依存しない() {
        let set = InfoSet::from_fresh(vec![lone_king_vs_gold()]);
        let AttackExpansion::Groups(groups) = expand_attack(&set) else { panic!() };
        let positions = groups[0].1.positions.clone();
        let mut reversed = positions.clone();
        reversed.reverse();
        assert_eq!(InfoSet::new(positions).hash(), InfoSet::new(reversed).hash());
    }
}
