use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::Observation;
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
    /// ノード数上限
    node_limit: u64,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
}

impl TsuitateDfpnSolver {
    pub fn new(node_limit: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            or_table: HashMap::new(),
            and_table: HashMap::new(),
            nodes_searched: 0,
            node_limit,
            cancelled,
        }
    }

    /// メタポジションが詰むかどうかを判定する
    pub fn solve(&mut self, meta: &MetaPosition) -> TsuitateDfpnResult {
        let hash = meta_position_hash(meta);
        let mut path = vec![hash];

        self.mid_or(meta, INF, INF, &mut path);

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
            return;
        }
        if meta.positions.len() > MAX_META_POSITIONS {
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            return;
        }

        // 候補手を生成
        let (candidates, legal_move_sets) = self.generate_attack_candidates(meta);
        if self.should_stop() {
            return;
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

        // Illegal 分岐
        if !illegal_meta.is_empty() {
            let bh = meta_position_hash(&illegal_meta);
            let (cpn, cdn) = self.lookup_or(bh, path);
            obs_pn.push(cpn);
            obs_dn.push(cdn);
            obs_terminal.push(false);
            obs_hash.push(bh);
            obs_metas.push(illegal_meta);
        }

        // 合法分岐
        if !legal_meta.is_empty() {
            if legal_meta.all_effectively_checkmate() {
                // 全合法盤面で実質詰み
                obs_pn.push(0);
                obs_dn.push(INF);
                obs_terminal.push(true);
                obs_hash.push(0);
                obs_metas.push(MetaPosition {
                    positions: Vec::new(),
                });
            } else {
                let branches = legal_meta.expand_defense_moves(mv);
                for (obs, branch_meta) in branches {
                    match obs {
                        Observation::Checkmate => {
                            obs_pn.push(0);
                            obs_dn.push(INF);
                            obs_terminal.push(true);
                            obs_hash.push(0);
                            obs_metas.push(branch_meta);
                        }
                        Observation::Captured | Observation::NoCapture => {
                            if branch_meta.positions.len() > MAX_META_POSITIONS {
                                self.and_table
                                    .insert(and_key, PnDn { pn: INF, dn: 0 });
                                return;
                            }
                            let bh = meta_position_hash(&branch_meta);
                            let (cpn, cdn) = self.lookup_or(bh, path);
                            obs_pn.push(cpn);
                            obs_dn.push(cdn);
                            obs_terminal.push(false);
                            obs_hash.push(bh);
                            obs_metas.push(branch_meta);
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
            path.push(child_hash);
            self.mid_or(
                &obs_metas[best_idx],
                child_pn_limit,
                child_dn_limit,
                path,
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
}
