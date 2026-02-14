use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::dfpn::{DfpnResult, DfpnSolver};
use super::metaposition::MetaPosition;
use super::solution::Observation;
use crate::shogi::position::Position;
use crate::shogi::types::*;

/// MetaPosition のハッシュキー（転置表用）
/// 各 Position のハッシュを XOR で結合し、順序に依存しないキーを生成
fn meta_position_hash(meta: &MetaPosition) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut xor_hash: u64 = 0;
    for pos in &meta.positions {
        let mut h = DefaultHasher::new();
        pos.hash(&mut h);
        xor_hash ^= h.finish();
    }
    xor_hash
}

/// メタポジションのサイズ上限
const MAX_META_POSITIONS: usize = 5000;

/// 不詰証明の結果
#[derive(Debug, Clone)]
pub struct RefutationResult {
    /// 不詰が証明されたか
    pub proven: bool,
    /// 何手まで確認して証明したか（proven=trueの場合のみ有効）
    pub proven_depth: u32,
    /// 探索ノード数
    pub nodes_searched: u64,
}

/// 不詰証明ソルバー
///
/// OR/AND構造の反転:
/// - 詰みソルバー: 候補手=OR（1つ成功でOK）、観測分岐=AND（全部成功が必要）
/// - 不詰ソルバー: 候補手=AND（全部失敗を証明）、観測分岐=OR（1つ失敗で十分）
pub struct RefutationSolver {
    /// 最大探索深さ
    max_depth: u32,
    /// 探索ノード数
    pub nodes_searched: u64,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
    /// 不詰が証明されたメタポジション: hash → 証明済みの最大remaining_depth
    refutation_table: HashMap<u64, u32>,
    /// df-pn のノード上限
    dfpn_node_limit: u64,
    /// df-pn の累計ノード数
    pub dfpn_nodes_total: u64,
}

impl RefutationSolver {
    pub fn new(max_depth: u32, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            max_depth,
            nodes_searched: 0,
            cancelled,
            refutation_table: HashMap::new(),
            dfpn_node_limit: 500_000,
            dfpn_nodes_total: 0,
        }
    }

    /// 不詰を証明する（指数的反復深化: 1, 3, 5, ... max_depth）
    ///
    /// IDS の各反復で refutation_table を蓄積し、深い探索を高速化する。
    /// refute_all_attacks(meta, D) は「D手以内の詰みがない」ことを証明する。
    /// 深さDで詰みが見つかった場合（refute失敗）、より深い探索でも同じ詰みが
    /// 存在するため、早期に「不詰証明失敗」として返せる。
    pub fn prove_no_checkmate(&mut self, meta: &MetaPosition) -> RefutationResult {
        let depths = self.build_depth_sequence();
        let mut last_depth = 0;

        for &search_depth in &depths {
            if self.is_cancelled() {
                break;
            }
            last_depth = search_depth;
            if !self.refute_all_attacks(meta, search_depth) {
                // この深さで詰みが存在する → 不詰証明失敗（早期リターン）
                return RefutationResult {
                    proven: false,
                    proven_depth: 0,
                    nodes_searched: self.nodes_searched,
                };
            }
        }

        // 全深さで詰みが見つからなかった → 不詰証明成功
        RefutationResult {
            proven: !self.is_cancelled() && last_depth > 0,
            proven_depth: last_depth,
            nodes_searched: self.nodes_searched,
        }
    }

    /// 特定の初手に対して不詰を証明する
    /// メタポジションに初手を適用した後の状態で不詰証明を行う
    pub fn prove_no_checkmate_for_move(
        &mut self,
        meta: &MetaPosition,
        first_move: Move,
    ) -> RefutationResult {
        // 初手を適用して合法/不正に分割
        let (legal_meta, illegal_meta) = meta.apply_attack_move_split(first_move);

        if legal_meta.is_empty() {
            // この手はどの盤面でも指せない → 不詰（指せない手は詰ませられない）
            return RefutationResult {
                proven: true,
                proven_depth: 0,
                nodes_searched: self.nodes_searched,
            };
        }

        // 全合法盤面で王手がかかっているか確認
        if !legal_meta
            .positions
            .iter()
            .all(|pos| pos.is_in_check(pos.side_to_move))
        {
            // 一部の盤面で王手にならない → 衝立詰将棋では不正解手
            return RefutationResult {
                proven: true,
                proven_depth: 0,
                nodes_searched: self.nodes_searched,
            };
        }

        let depths = self.build_depth_sequence();
        let mut last_depth = 0;

        for &search_depth in &depths {
            if self.is_cancelled() {
                break;
            }
            last_depth = search_depth;
            if !self.can_refute_move(first_move, &legal_meta, &illegal_meta, search_depth) {
                // この深さでこの手が詰みに導ける → 不詰証明失敗（早期リターン）
                return RefutationResult {
                    proven: false,
                    proven_depth: 0,
                    nodes_searched: self.nodes_searched,
                };
            }
        }

        // 全深さでこの手は詰みに導けなかった → 不詰証明成功
        RefutationResult {
            proven: !self.is_cancelled() && last_depth > 0,
            proven_depth: last_depth,
            nodes_searched: self.nodes_searched,
        }
    }

    /// 探索深さの系列を構築（奇数のみ: 1, 3, 5, 7, ...）
    fn build_depth_sequence(&self) -> Vec<u32> {
        let max_search_depth = if self.max_depth % 2 == 0 {
            self.max_depth - 1
        } else {
            self.max_depth
        };
        let mut depths = Vec::new();
        let mut d = 1u32;
        while d <= max_search_depth {
            depths.push(d);
            d += 2;
        }
        depths
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// 通常詰将棋（完全情報）として不詰かどうかを df-pn で判定する
    /// 通常詰将棋で詰まなければ衝立詰将棋でも詰まない
    fn is_regular_tsume_unsolvable(&mut self, pos: &Position) -> bool {
        let mut dfpn = DfpnSolver::new(self.dfpn_node_limit, self.cancelled.clone());
        let mut pos_clone = pos.clone();
        let result = dfpn.solve(&mut pos_clone);
        self.dfpn_nodes_total += dfpn.nodes_searched;
        result == DfpnResult::Disproven
    }

    /// 全ての攻め手が不詰であることを証明する（ANDノード: 全候補手が失敗必要）
    fn refute_all_attacks(&mut self, meta: &MetaPosition, remaining_depth: u32) -> bool {
        self.nodes_searched += 1;

        if self.is_cancelled() {
            return false;
        }

        if meta.is_empty() {
            return true; // 空のメタポジション → 不詰
        }

        // 既に全て詰んでいるなら不詰証明は失敗
        if meta.all_checkmate() {
            return false;
        }

        // 深さ0で詰んでいない → 不詰証明成功
        if remaining_depth == 0 {
            return true;
        }

        // 転置表チェック: 同じメタポジションがremaining_depth以下で不詰証明済みならスキップ
        let meta_hash = meta_position_hash(meta);
        if let Some(&proven_depth) = self.refutation_table.get(&meta_hash) {
            if remaining_depth <= proven_depth {
                return true;
            }
        }

        // df-pn フィルタ: メタポジションが1局面の場合、通常詰将棋として不詰判定
        if meta.positions.len() == 1 {
            if self.is_regular_tsume_unsolvable(&meta.positions[0]) {
                let entry = self.refutation_table.entry(meta_hash).or_insert(0);
                if remaining_depth > *entry {
                    *entry = remaining_depth;
                }
                return true; // 通常詰将棋で詰まない → 衝立でも詰まない
            }
        }

        // 候補手を列挙
        let (candidate_moves, legal_move_sets) = self.generate_attack_candidates(meta);

        if candidate_moves.is_empty() {
            // 候補手がない → 攻め方は詰ませられない → 不詰証明成功
            let entry = self.refutation_table.entry(meta_hash).or_insert(0);
            if remaining_depth > *entry {
                *entry = remaining_depth;
            }
            return true;
        }

        // 成/不成の重複候補をスキップするためのフィンガープリント
        let mut branch_fingerprints: HashMap<(Option<Square>, Square), u64> = HashMap::new();

        // AND: 全候補手が失敗（不詰）であることを証明する
        for mv in candidate_moves {
            if self.is_cancelled() {
                return false;
            }

            // 全盤面にこの手を適用し、合法/不正に分割
            let (legal_meta, illegal_meta) =
                meta.apply_attack_move_split_with_sets(mv, &legal_move_sets);

            if legal_meta.is_empty() {
                continue; // この手はどの盤面でも指せない → 次の候補手へ
            }

            // 全合法盤面で王手がかかっているか確認
            if !legal_meta
                .positions
                .iter()
                .all(|pos| pos.is_in_check(pos.side_to_move))
            {
                continue; // 王手にならない → この候補手は不正解 → 次へ
            }

            // 成/不成の重複検出
            if mv.from.is_some() {
                let branches = legal_meta.expand_defense_moves(mv);
                let fp = {
                    use std::collections::hash_map::DefaultHasher;
                    let mut h = DefaultHasher::new();
                    h.write_u64(meta_position_hash(&illegal_meta));
                    h.write_usize(branches.len());
                    for (obs, bm) in &branches {
                        obs.hash(&mut h);
                        h.write_u64(meta_position_hash(bm));
                    }
                    h.finish()
                };
                let key = (mv.from, mv.to);
                if let Some(&prev) = branch_fingerprints.get(&key) {
                    if prev == fp {
                        continue; // 同一分岐 → スキップ
                    }
                }
                branch_fingerprints.insert(key, fp);
            }

            // この手が不詰であることを証明する
            if !self.can_refute_move(mv, &legal_meta, &illegal_meta, remaining_depth) {
                // この手で詰ませられる可能性がある → 不詰証明失敗
                return false;
            }
        }

        // 全候補手で不詰 → 証明成功
        if !self.is_cancelled() {
            let entry = self.refutation_table.entry(meta_hash).or_insert(0);
            if remaining_depth > *entry {
                *entry = remaining_depth;
            }
        }
        true
    }

    /// 特定の手が不詰であることを証明する（ORノード: 1つの分岐で失敗すれば十分）
    fn can_refute_move(
        &mut self,
        mv: Move,
        legal_meta: &MetaPosition,
        illegal_meta: &MetaPosition,
        remaining_depth: u32,
    ) -> bool {
        // 反則分岐: 反則した盤面で不詰なら即リターン
        if !illegal_meta.is_empty() {
            if self.refute_all_attacks(illegal_meta, remaining_depth) {
                return true; // 反則分岐で不詰証明成功
            }
        }

        // 残り深さ1の場合の高速チェック
        if remaining_depth <= 1 {
            // 全盤面が実質的に詰みでないなら、不詰
            if !legal_meta.all_effectively_checkmate() {
                return true;
            }
            return false; // 全盤面で実質詰み → この手は有効
        }

        // 玉方の応手を展開
        let branches = legal_meta.expand_defense_moves(mv);

        // 分岐を「攻め方にとって厳しい順」にソート（OR: 1つ見つければ終了）
        // Captured > NoCapture（攻め駒を失った方が厳しい）
        // メタポジション数が多い方が先（全盤面に通用する手を見つけにくい）
        let mut sorted_branches: Vec<&(Observation, MetaPosition)> = branches.iter().collect();
        sorted_branches.sort_by(|a, b| {
            let obs_ord = |obs: &Observation| -> u8 {
                match obs {
                    Observation::Captured => 0,   // 最優先: 攻め駒が取られた
                    Observation::NoCapture => 1,   // 次: 取られてない
                    Observation::Checkmate => 2,   // 詰みは最後（使えない）
                    Observation::Illegal => 3,     // ここには来ない
                }
            };
            let ord_a = obs_ord(&a.0);
            let ord_b = obs_ord(&b.0);
            if ord_a != ord_b {
                return ord_a.cmp(&ord_b);
            }
            // 同じ観測種別ならメタポジション数が多い方を先に
            b.1.positions.len().cmp(&a.1.positions.len())
        });

        // OR: 1つの分岐で不詰を証明できれば十分
        for (obs, branch_meta) in sorted_branches {
            if self.is_cancelled() {
                return false;
            }

            match obs {
                Observation::Checkmate => {
                    continue; // 詰み分岐は攻め方に有利 → 不詰証明に使えない
                }
                Observation::Captured | Observation::NoCapture => {
                    // メタポジションが大きすぎる場合 → 不詰の可能性が高い（枝刈り）
                    if branch_meta.positions.len() > MAX_META_POSITIONS {
                        return true;
                    }
                    // df-pn フィルタ: 1局面なら通常詰将棋として不詰判定
                    if branch_meta.positions.len() == 1 {
                        if self.is_regular_tsume_unsolvable(&branch_meta.positions[0]) {
                            return true; // この分岐は不詰
                        }
                    }
                    if self.refute_all_attacks(branch_meta, remaining_depth - 2) {
                        return true; // この分岐で不詰証明成功
                    }
                }
                Observation::Illegal => {
                    // expand_defense_moves からは来ない
                }
            }
        }

        false // 全分岐で攻め方が詰ませられる → 不詰証明失敗
    }

    /// 攻め方の候補手を生成（solver.rs の generate_attack_candidates と同等）
    fn generate_attack_candidates(&self, meta: &MetaPosition) -> (Vec<Move>, Vec<HashSet<Move>>) {
        if meta.positions.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let n = meta.positions.len();

        // 各盤面の合法手セットを事前計算
        let legal_move_sets: Vec<HashSet<Move>> = meta
            .positions
            .iter()
            .map(|pos| {
                if self.is_cancelled() {
                    return HashSet::new();
                }
                pos.generate_legal_moves().into_iter().collect()
            })
            .collect();

        if self.is_cancelled() {
            return (Vec::new(), legal_move_sets);
        }

        let mut seen = HashSet::new();
        let mut check_moves = Vec::new();

        // 全盤面から王手の手を収集（union）
        for (i, pos) in meta.positions.iter().enumerate() {
            let opponent = pos.side_to_move.opponent();
            let mut test_pos = pos.clone();
            for mv in &legal_move_sets[i] {
                if seen.contains(mv) {
                    continue;
                }
                let undo = test_pos.make_move(*mv);
                if test_pos.is_in_check(opponent) {
                    seen.insert(*mv);
                    check_moves.push(*mv);
                }
                test_pos.unmake_move(&undo);
            }
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
            (is_drop, dist, interposition, piece_priority, mv.to.file, mv.to.rank)
        };
        check_moves.sort_by_key(|mv| sort_key(mv));

        // 合駒可能マスが多い長距離王手を除外
        let max_interposition: u8 = 4; // 不詰証明では緩めに
        let king_sq_for_filter = meta.positions[0].find_king(Color::Gote);
        check_moves.retain(|mv| {
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
            if !is_slider {
                return true;
            }
            if let Some(ksq) = king_sq_for_filter {
                let dist = ((mv.to.file as i8 - ksq.file as i8).unsigned_abs())
                    .max((mv.to.rank as i8 - ksq.rank as i8).unsigned_abs());
                let interp = if dist > 1 { dist - 1 } else { 0 };
                interp <= max_interposition
            } else {
                true
            }
        });

        // メタポジションが1つの場合、プローブは不要
        if n <= 1 {
            return (check_moves, legal_move_sets);
        }

        // プローブ手
        let mut move_counts: HashMap<Move, usize> = HashMap::new();
        for legal_set in &legal_move_sets {
            for mv in legal_set {
                *move_counts.entry(*mv).or_insert(0) += 1;
            }
        }

        let mut probe_moves = Vec::new();
        for (mv, count) in &move_counts {
            if seen.contains(mv) {
                continue;
            }
            if *count > 0 && *count < n {
                seen.insert(*mv);
                probe_moves.push(*mv);
            }
        }

        check_moves.extend(probe_moves);
        (check_moves, legal_move_sets)
    }
}
