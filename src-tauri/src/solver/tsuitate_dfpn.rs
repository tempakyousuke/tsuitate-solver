use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::types::*;

/// 証明数/反証数の上限（sum時のオーバーフロー防止）
const INF: u32 = u32::MAX / 2;

/// メタポジションのサイズ上限
const MAX_META_POSITIONS: usize = 5000;

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
#[derive(Debug, Clone, Copy)]
struct PnDn {
    pn: u32,
    dn: u32,
}

fn meta_position_hash(meta: &MetaPosition) -> u64 {
    let mut xor_hash: u64 = 0;
    for pos in &meta.positions {
        let mut h = DefaultHasher::new();
        pos.hash(&mut h);
        xor_hash ^= h.finish();
    }
    xor_hash
}

/// MetaPosition の盤面セットハッシュ（sente_hand を除外）
/// 優越関係の判定用: 同じ盤面セットで sente_hand が異なるメタポジションを同一グループにまとめる
fn meta_board_set_hash(meta: &MetaPosition) -> u64 {
    let mut xor_hash: u64 = 0;
    for pos in &meta.positions {
        xor_hash ^= pos.hash_without_sente_hand();
    }
    xor_hash
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
    or_table: HashMap<u64, PnDn>,
    /// AND ノードの転置表: hash(meta_hash, move) → (pn, dn)
    and_table: HashMap<u64, PnDn>,
    /// 探索ノード数
    pub nodes_searched: u64,
    /// 優越関係ヒット数（診断用）
    pub dominance_hits: u64,
    /// ノード数上限
    node_limit: u64,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
    /// ルートで除外する手（余詰めチェック用）
    excluded_root_moves: Vec<Move>,
    /// ルートのメタポジションハッシュ（除外判定用）
    root_hash: Option<u64>,
    /// 深さ上限（None = 制限なし）
    depth_limit: Option<u32>,
    /// 優越関係テーブル: board_set_hash → Vec<[u8; 7]> (Pareto最小の proof_hand)
    dominance_table: HashMap<u64, Vec<[u8; 7]>>,
    /// OR ノードの証明駒: meta_hash → proof_hand (証明に必要な最小の攻め方持ち駒)
    or_proof_hands: HashMap<u64, [u8; 7]>,
    /// AND ノードの証明駒: and_key → proof_hand
    and_proof_hands: HashMap<u64, [u8; 7]>,
}

impl TsuitateDfpnSolver {
    pub fn new(node_limit: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            or_table: HashMap::new(),
            and_table: HashMap::new(),
            nodes_searched: 0,
            dominance_hits: 0,
            node_limit,
            cancelled,
            excluded_root_moves: Vec::new(),
            root_hash: None,
            depth_limit: None,
            dominance_table: HashMap::new(),
            or_proof_hands: HashMap::new(),
            and_proof_hands: HashMap::new(),
        }
    }

    /// メタポジションが詰むかどうかを判定する
    pub fn solve(&mut self, meta: &MetaPosition) -> TsuitateDfpnResult {
        let hash = meta_position_hash(meta);
        self.root_hash = Some(hash);
        let mut path = vec![hash];

        self.mid_or(meta, INF, INF, &mut path, 0);

        let entry = self.or_table.get(&hash).copied().unwrap_or(PnDn { pn: 1, dn: 1 });
        if entry.pn == 0 {
            TsuitateDfpnResult::Proven
        } else if entry.dn == 0 {
            TsuitateDfpnResult::Disproven
        } else {
            TsuitateDfpnResult::Unknown
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn should_stop(&self) -> bool {
        self.nodes_searched >= self.node_limit || self.is_cancelled()
    }

    fn and_key(meta_hash: u64, mv: &Move) -> u64 {
        let mut h = DefaultHasher::new();
        h.write_u64(meta_hash);
        mv.hash(&mut h);
        h.finish()
    }

    /// OR ノード（攻め方）: 候補手から1つでも詰みに導ければ成功
    ///
    /// pn = min{pn_c}, dn = sum{dn_c}
    fn mid_or(
        &mut self,
        meta: &MetaPosition,
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
        depth: u32,
    ) {
        self.nodes_searched += 1;
        let hash = meta_position_hash(meta);

        // 終端チェック
        if meta.is_empty() {
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            return;
        }
        if meta.all_effectively_checkmate() {
            self.or_table.insert(hash, PnDn { pn: 0, dn: INF });
            let proof_hand = [0u8; 7];
            self.or_proof_hands.insert(hash, proof_hand);
            if !meta.positions.is_empty() {
                let board_hash = meta_board_set_hash(meta);
                self.add_dominance_entry(board_hash, proof_hand);
            }
            return;
        }
        if meta.positions.len() > MAX_META_POSITIONS {
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            return;
        }

        // 深さ制限チェック
        if let Some(limit) = self.depth_limit {
            if depth >= limit {
                self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
                return;
            }
        }

        // 優越関係チェック
        if !meta.positions.is_empty() {
            let board_hash = meta_board_set_hash(meta);
            let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
            if let Some(entries) = self.dominance_table.get(&board_hash) {
                if let Some(matched_ph) = entries
                    .iter()
                    .find(|proven| proven.iter().zip(sente_hand.iter()).all(|(p, s)| p <= s))
                {
                    self.or_table.insert(hash, PnDn { pn: 0, dn: INF });
                    self.or_proof_hands.insert(hash, *matched_ph);
                    self.dominance_hits += 1;
                    return;
                }
            }
        }

        // 候補手を生成
        let (mut candidates, legal_move_sets) = self.generate_attack_candidates(meta);
        if self.should_stop() {
            return;
        }

        // ルートノードでは除外手をフィルタ
        if self.root_hash == Some(hash) && !self.excluded_root_moves.is_empty() {
            candidates.retain(|mv| {
                !self.excluded_root_moves.iter().any(|exc| {
                    exc.from == mv.from
                        && exc.to == mv.to
                        && exc.promotion == mv.promotion
                        && exc.drop_piece == mv.drop_piece
                })
            });
        }

        if candidates.is_empty() {
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            return;
        }

        // 子ノード（AND）の初期化
        let mut children: Vec<(Move, u64, u32, u32)> = Vec::with_capacity(candidates.len());
        for mv in &candidates {
            let ak = Self::and_key(hash, mv);
            let (cpn, cdn) = if let Some(e) = self.and_table.get(&ak) {
                (e.pn, e.dn)
            } else {
                (1, 1)
            };
            children.push((*mv, ak, cpn, cdn));
        }

        loop {
            if self.should_stop() {
                return;
            }

            // OR 集約: pn = min{pn_c}, dn = sum{dn_c}
            let pn_n = children.iter().map(|c| c.2).min().unwrap_or(INF);
            let dn_n = children.iter().map(|c| c.3).fold(0u32, |a, d| a.saturating_add(d));

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.or_table.insert(hash, PnDn { pn: pn_n, dn: dn_n });
                return;
            }
            if pn_n == 0 || dn_n == 0 {
                self.or_table.insert(hash, PnDn { pn: pn_n, dn: dn_n });
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
                    self.record_proven_dominance(meta, or_ph);
                }
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
                best_mv,
                &legal_move_sets,
                child_pn_limit,
                child_dn_limit,
                path,
                depth,
            );

            // AND テーブルから値を読み戻す
            let ak = children[best_idx].1;
            let e = self.and_table.get(&ak).copied().unwrap_or(PnDn { pn: 1, dn: 1 });
            children[best_idx].2 = e.pn;
            children[best_idx].3 = e.dn;
        }
    }

    /// AND ノード（観測分岐）: 全分岐で詰めば証明
    ///
    /// pn = sum{pn_c}, dn = min{dn_c}
    /// 分岐は最大4つ: Illegal, Checkmate, Captured, NoCapture
    fn mid_and(
        &mut self,
        meta: &MetaPosition,
        mv: Move,
        legal_move_sets: &[HashSet<Move>],
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
        depth: u32,
    ) {
        let meta_hash = meta_position_hash(meta);
        let and_key = Self::and_key(meta_hash, &mv);

        // 手を適用して合法/不正に分割
        let (legal_meta, illegal_meta) =
            meta.apply_attack_move_split_with_sets(mv, legal_move_sets);

        if legal_meta.is_empty() && illegal_meta.is_empty() {
            self.and_table.insert(and_key, PnDn { pn: INF, dn: 0 });
            return;
        }

        // 全合法盤面で王手がかかっているか（衝立詰将棋: 毎手王手が必要）
        if !legal_meta.is_empty()
            && !legal_meta
                .positions
                .iter()
                .all(|pos| pos.is_in_check(pos.side_to_move))
        {
            self.and_table.insert(and_key, PnDn { pn: INF, dn: 0 });
            return;
        }

        // 観測分岐の子ノードを構築（並列配列）
        let mut obs_pn: Vec<u32> = Vec::new();
        let mut obs_dn: Vec<u32> = Vec::new();
        let mut obs_terminal: Vec<bool> = Vec::new();
        let mut obs_hash: Vec<u64> = Vec::new();
        let mut obs_metas: Vec<MetaPosition> = Vec::new();
        let mut obs_depth_inc: Vec<u32> = Vec::new();
        let mut obs_hands: Vec<[u8; 7]> = Vec::new(); // 各分岐の sente_hand（proof_hand 計算用）
        let parent_hand = meta.positions.first()
            .map(|p| p.hand(Color::Sente).counts_array())
            .unwrap_or([0; 7]);

        // Illegal 分岐
        if !illegal_meta.is_empty() {
            let bh = meta_position_hash(&illegal_meta);
            let (cpn, cdn) = self.lookup_or(bh, path);
            obs_pn.push(cpn);
            obs_dn.push(cdn);
            obs_terminal.push(false);
            obs_hash.push(bh);
            obs_hands.push(parent_hand); // Illegal: 手が実行されていないので親と同じ
            obs_metas.push(illegal_meta);
            obs_depth_inc.push(0);
        }

        // 合法分岐
        if !legal_meta.is_empty() {
            if legal_meta.all_effectively_checkmate() {
                // 全合法盤面で実質詰み → 空の proof_hand を dominance に記録
                if !legal_meta.positions.is_empty() {
                    let board_hash = meta_board_set_hash(&legal_meta);
                    self.add_dominance_entry(board_hash, [0; 7]);
                }
                let legal_hand = legal_meta.positions.first()
                    .map(|p| p.hand(Color::Sente).counts_array())
                    .unwrap_or(parent_hand);
                obs_pn.push(0);
                obs_dn.push(INF);
                obs_terminal.push(true);
                obs_hash.push(0);
                obs_hands.push(legal_hand);
                obs_metas.push(MetaPosition {
                    positions: Vec::new(),
                });
                obs_depth_inc.push(0); // 終端: 再帰しない
            } else {
                let branches = legal_meta.expand_defense_moves(mv);
                for (obs, branch_meta) in branches {
                    match obs {
                        Observation::Checkmate => {
                            let branch_hand = branch_meta.positions.first()
                                .map(|p| p.hand(Color::Sente).counts_array())
                                .unwrap_or(parent_hand);
                            obs_pn.push(0);
                            obs_dn.push(INF);
                            obs_terminal.push(true);
                            obs_hash.push(0);
                            obs_hands.push(branch_hand);
                            obs_metas.push(branch_meta);
                            obs_depth_inc.push(0); // 終端: 再帰しない
                        }
                        Observation::Captured { .. } | Observation::NoCapture => {
                            if branch_meta.positions.len() > MAX_META_POSITIONS {
                                self.and_table
                                    .insert(and_key, PnDn { pn: INF, dn: 0 });
                                return;
                            }
                            let branch_hand = branch_meta.positions.first()
                                .map(|p| p.hand(Color::Sente).counts_array())
                                .unwrap_or(parent_hand);
                            let bh = meta_position_hash(&branch_meta);
                            let (cpn, cdn) = self.lookup_or(bh, path);
                            obs_pn.push(cpn);
                            obs_dn.push(cdn);
                            obs_terminal.push(false);
                            obs_hash.push(bh);
                            obs_hands.push(branch_hand);
                            obs_metas.push(branch_meta);
                            obs_depth_inc.push(2); // 攻め方の手 + 玉方の応手
                        }
                        Observation::Illegal => {}
                    }
                }
            }
        }

        if obs_pn.is_empty() {
            self.and_table.insert(and_key, PnDn { pn: INF, dn: 0 });
            return;
        }

        let n = obs_pn.len();

        loop {
            if self.should_stop() {
                return;
            }

            // AND 集約: pn = sum{pn_c}, dn = min{dn_c}
            let pn_n = obs_pn.iter().fold(0u32, |a, &p| a.saturating_add(p));
            let dn_n = obs_dn.iter().copied().min().unwrap_or(INF);

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.and_table.insert(and_key, PnDn { pn: pn_n, dn: dn_n });
                return;
            }
            if pn_n == 0 || dn_n == 0 {
                self.and_table.insert(and_key, PnDn { pn: pn_n, dn: dn_n });
                if pn_n == 0 {
                    // AND 証明駒の計算: max over branches of max(P_b[j] + H[j] - H_b[j], 0)
                    let mut and_ph = [0u8; 7];
                    for i in 0..n {
                        let child_ph = if obs_terminal[i] {
                            [0u8; 7]
                        } else {
                            self.or_proof_hands.get(&obs_hash[i])
                                .copied().unwrap_or(parent_hand)
                        };
                        let child_hand = obs_hands[i];
                        for j in 0..7 {
                            let eff = (child_ph[j] as i16) + (parent_hand[j] as i16)
                                - (child_hand[j] as i16);
                            let eff = eff.max(0) as u8;
                            and_ph[j] = and_ph[j].max(eff);
                        }
                    }
                    self.and_proof_hands.insert(and_key, and_ph);
                }
                return;
            }

            // 最弱の非終端子（dn 最小）を選択
            let best = (0..n)
                .filter(|&i| !obs_terminal[i])
                .min_by_key(|&i| obs_dn[i]);
            let best_idx = match best {
                Some(i) => i,
                None => {
                    // 全て終端（ありえないはずだが安全のため）
                    self.and_table.insert(and_key, PnDn { pn: pn_n, dn: dn_n });
                    return;
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
            let child_hash = obs_hash[best_idx];
            let child_depth = depth + obs_depth_inc[best_idx];
            path.push(child_hash);
            self.mid_or(
                &obs_metas[best_idx],
                child_pn_limit,
                child_dn_limit,
                path,
                child_depth,
            );
            path.pop();

            // OR テーブルから値を読み戻す
            let e = self
                .or_table
                .get(&child_hash)
                .copied()
                .unwrap_or(PnDn { pn: 1, dn: 1 });
            obs_pn[best_idx] = e.pn;
            obs_dn[best_idx] = e.dn;
        }
    }

    /// OR ノードの初期値を取得（転置表 or デフォルト）
    /// ループ検出時は反証扱い
    fn lookup_or(&self, hash: u64, path: &[u64]) -> (u32, u32) {
        if path.contains(&hash) {
            return (INF, 0);
        }
        if let Some(e) = self.or_table.get(&hash) {
            (e.pn, e.dn)
        } else {
            (1, 1)
        }
    }

    /// 優越関係テーブルに証明駒で記録する
    fn record_proven_dominance(&mut self, meta: &MetaPosition, proof_hand: [u8; 7]) {
        if meta.positions.is_empty() {
            return;
        }
        let board_hash = meta_board_set_hash(meta);
        self.add_dominance_entry(board_hash, proof_hand);
    }

    /// dominance_table にエントリを追加（Pareto 最小を維持）
    /// 追加された場合 true を返す
    fn add_dominance_entry(&mut self, board_hash: u64, hand: [u8; 7]) -> bool {
        let entries = self.dominance_table.entry(board_hash).or_default();
        if entries
            .iter()
            .any(|e| e.iter().zip(hand.iter()).all(|(ei, hi)| ei <= hi))
        {
            return false;
        }
        entries.retain(|e| !e.iter().zip(hand.iter()).all(|(ei, hi)| hi <= ei));
        entries.push(hand);
        true
    }

    /// 攻め方の候補手を生成
    /// 全盤面から王手の手を収集し、プローブ手を追加
    fn generate_attack_candidates(
        &self,
        meta: &MetaPosition,
    ) -> (Vec<Move>, Vec<HashSet<Move>>) {
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

        // 合駒可能マスが多い長距離王手を除外
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
                interp <= 4
            } else {
                true
            }
        });

        // メタポジションが1つならプローブ不要
        if n <= 1 {
            return (check_moves, legal_move_sets);
        }

        // プローブ手: 一部の盤面でのみ合法な手
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
                let (tree, depth, shorten_nodes) = if find_shortest && depth > 1 {
                    self.shorten_solution(meta, tree, depth)
                } else {
                    (tree, depth, 0)
                };
                let nodes_before_second = initial_nodes + shorten_nodes;

                // 余詰めチェック
                // find_second_solution は self.nodes_searched を first_phase_nodes として使うため、
                // shorten 分を含めた累計値を設定しておく
                self.nodes_searched = nodes_before_second;
                let (second_tree, total_nodes, second_msg) = if find_second {
                    if let Some(ref t) = tree {
                        self.find_second_solution(meta, t)
                    } else {
                        (None, nodes_before_second, String::new())
                    }
                } else {
                    (None, nodes_before_second, String::new())
                };

                let message = if second_tree.is_some() {
                    let second_depth = second_tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
                    format!(
                        "余詰めあり: {}手詰めと{}手詰めが見つかりました (探索ノード数: {})",
                        depth, second_depth, total_nodes
                    )
                } else if find_second && !second_msg.is_empty() {
                    format!(
                        "{}手詰めが見つかりました（余詰めなし、探索ノード数: {}）",
                        depth, total_nodes
                    )
                } else {
                    format!(
                        "{}手詰めが見つかりました (探索ノード数: {})",
                        depth, total_nodes
                    )
                };

                SolutionData {
                    found: true,
                    tree,
                    second_tree,
                    message,
                    trace: Vec::new(),
                }
            }
            TsuitateDfpnResult::Disproven => SolutionData {
                found: false,
                tree: None,
                second_tree: None,
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
                message: format!(
                    "探索を打ち切りました (探索ノード数: {})",
                    self.nodes_searched
                ),
                trace: Vec::new(),
            },
        }
    }

    /// 二分探索で最短の深さを求める
    fn shorten_solution(
        &mut self,
        meta: &MetaPosition,
        initial_tree: Option<SolutionNode>,
        initial_depth: u32,
    ) -> (Option<SolutionNode>, u32, u64) {
        let mut low: u32 = 1;
        let mut high: u32 = initial_depth - 1;
        let mut best_tree = initial_tree;
        let mut best_depth = initial_depth;
        let mut total_nodes: u64 = 0;

        while low <= high {
            if self.should_stop() {
                break;
            }

            let mid = low + (high - low) / 2;

            // 転置表クリア、深さ制限付きで再探索
            self.or_table.clear();
            self.and_table.clear();
            self.dominance_table.clear();
            self.or_proof_hands.clear();
            self.and_proof_hands.clear();
            self.nodes_searched = 0;
            self.depth_limit = Some(mid);

            let result = self.solve(meta);
            total_nodes += self.nodes_searched;

            match result {
                TsuitateDfpnResult::Proven => {
                    let tree = self.extract_solution(meta);
                    let actual_depth = tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
                    if actual_depth < best_depth {
                        best_tree = tree;
                        best_depth = actual_depth;
                    }
                    if actual_depth <= 1 {
                        break;
                    }
                    high = actual_depth - 1;
                }
                _ => {
                    low = mid + 1;
                }
            }
        }

        self.depth_limit = None;
        (best_tree, best_depth, total_nodes)
    }

    /// 1つ目の解の初手を除外して2つ目の解を探す
    fn find_second_solution(
        &mut self,
        meta: &MetaPosition,
        first_tree: &SolutionNode,
    ) -> (Option<SolutionNode>, u64, String) {
        // 1つ目の解の初手を取得
        let first_mv = match first_tree {
            SolutionNode::AttackMove { mv, .. } => {
                let to = Square::new(mv.to_file, mv.to_rank);
                let from = match (mv.from_file, mv.from_rank) {
                    (Some(f), Some(r)) => Some(Square::new(f, r)),
                    _ => None,
                };
                let drop_piece = mv.drop_piece.as_ref().and_then(|s| {
                    match s.as_str() {
                        "飛" => Some(PieceKind::Rook),
                        "角" => Some(PieceKind::Bishop),
                        "金" => Some(PieceKind::Gold),
                        "銀" => Some(PieceKind::Silver),
                        "桂" => Some(PieceKind::Knight),
                        "香" => Some(PieceKind::Lance),
                        "歩" => Some(PieceKind::Pawn),
                        _ => None,
                    }
                });
                Move {
                    from,
                    to,
                    promotion: mv.promotion,
                    drop_piece,
                    moved_piece_kind: None,
                }
            }
            SolutionNode::Checkmate { .. } => {
                return (None, self.nodes_searched, String::new());
            }
        };

        // 初手と成/不成のバリエーションを除外リストに追加
        let mut excluded = vec![first_mv];
        if first_mv.from.is_some() {
            let mut counterpart = first_mv;
            counterpart.promotion = !counterpart.promotion;
            excluded.push(counterpart);
        }

        let first_phase_nodes = self.nodes_searched;

        // 転置表をクリアして再探索
        self.or_table.clear();
        self.and_table.clear();
        self.dominance_table.clear();
        self.or_proof_hands.clear();
        self.and_proof_hands.clear();
        self.nodes_searched = 0;
        self.excluded_root_moves = excluded;

        let result = self.solve(meta);
        let second_phase_nodes = self.nodes_searched;
        let total_nodes = first_phase_nodes + second_phase_nodes;

        self.excluded_root_moves.clear();

        match result {
            TsuitateDfpnResult::Proven => {
                let second_tree = self.extract_solution(meta);
                (second_tree, total_nodes, "found".to_string())
            }
            _ => (None, total_nodes, "not_found".to_string()),
        }
    }

    /// 証明木を抽出する（solve() で Proven になった後に呼ぶ）
    pub fn extract_solution(&self, meta: &MetaPosition) -> Option<SolutionNode> {
        self.extract_or(meta, 0)
    }

    /// OR ノードから証明木を抽出する
    fn extract_or(&self, meta: &MetaPosition, depth: u32) -> Option<SolutionNode> {
        if meta.is_empty() {
            return None;
        }
        if meta.all_effectively_checkmate() {
            return Some(SolutionNode::Checkmate { depth });
        }

        let hash = meta_position_hash(meta);
        let entry = self.or_table.get(&hash)?;
        if entry.pn != 0 {
            return None; // 証明されていない
        }

        // 全盤面の合法手を収集
        let legal_move_sets: Vec<HashSet<Move>> = meta
            .positions
            .iter()
            .map(|pos| pos.generate_legal_moves().into_iter().collect())
            .collect();

        let mut seen = HashSet::new();
        let mut all_moves = Vec::new();
        for set in &legal_move_sets {
            for mv in set {
                if seen.insert(*mv) {
                    all_moves.push(*mv);
                }
            }
        }

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

        // AND テーブルで pn=0 の手を探す
        for mv in &all_moves {
            let ak = Self::and_key(hash, mv);
            if let Some(and_entry) = self.and_table.get(&ak) {
                if and_entry.pn == 0 {
                    if let Some(branches) =
                        self.extract_and(meta, *mv, &legal_move_sets, depth)
                    {
                        return Some(SolutionNode::AttackMove {
                            mv: MoveData::from_move(*mv, Color::Sente),
                            branches,
                        });
                    }
                }
            }
        }

        // AND テーブルで見つからない場合（優越関係で証明されたノード）
        // 各候補手で直接 extract_and を試す
        for mv in &all_moves {
            if let Some(branches) = self.extract_and(meta, *mv, &legal_move_sets, depth) {
                return Some(SolutionNode::AttackMove {
                    mv: MoveData::from_move(*mv, Color::Sente),
                    branches,
                });
            }
        }
        None
    }

    /// AND ノード（観測分岐）から証明木を抽出する
    fn extract_and(
        &self,
        meta: &MetaPosition,
        mv: Move,
        legal_move_sets: &[HashSet<Move>],
        depth: u32,
    ) -> Option<Vec<SolutionBranch>> {
        let (legal_meta, illegal_meta) =
            meta.apply_attack_move_split_with_sets(mv, legal_move_sets);

        let mut branches = Vec::new();

        // Illegal 分岐
        if !illegal_meta.is_empty() {
            let continuation = self.extract_or(&illegal_meta, depth)?;
            branches.push(SolutionBranch {
                observation: Observation::Illegal,
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
                continuation: Box::new(SolutionNode::Checkmate { depth: depth + 1 }),
            });
            return Some(branches);
        }

        // 玉方の応手を展開
        let obs_branches = legal_meta.expand_defense_moves(mv);
        for (obs, branch_meta) in obs_branches {
            match obs {
                Observation::Checkmate => {
                    branches.push(SolutionBranch {
                        observation: Observation::Checkmate,
                        continuation: Box::new(SolutionNode::Checkmate {
                            depth: depth + 1,
                        }),
                    });
                }
                Observation::Captured { .. } | Observation::NoCapture => {
                    // depth + 2: 攻め方の手(+1) + 玉方の応手(+1)
                    let continuation = self.extract_or(&branch_meta, depth + 2)?;
                    branches.push(SolutionBranch {
                        observation: obs,
                        continuation: Box::new(continuation),
                    });
                }
                Observation::Illegal => {}
            }
        }

        Some(branches)
    }
}
