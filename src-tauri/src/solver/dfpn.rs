use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::shogi::position::Position;

/// 証明数/反証数の上限（sum時のオーバーフロー防止）
const INF: u32 = u32::MAX / 2;

/// df-pn の探索結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DfpnResult {
    /// 詰みが証明された
    Proven,
    /// 不詰が証明された
    Disproven,
    /// 探索が打ち切られた（ノード上限 or キャンセル）
    Unknown,
}

/// 転置表エントリ
#[derive(Debug, Clone, Copy)]
struct DfpnEntry {
    pn: u32,
    dn: u32,
}

/// df-pn (Depth-First Proof Number Search) による通常詰将棋ソルバー
///
/// 完全情報下での詰将棋を高速に判定する。
/// 衝立詰将棋ソルバーのフィルタとして使用し、個別局面の不詰を素早く判定する。
pub struct DfpnSolver {
    /// 転置表: 局面ハッシュ → (pn, dn)
    table: HashMap<u64, DfpnEntry>,
    /// 探索ノード数
    pub nodes_searched: u64,
    /// ノード数上限
    node_limit: u64,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
}

fn position_hash(pos: &Position) -> u64 {
    let mut h = DefaultHasher::new();
    pos.hash(&mut h);
    h.finish()
}

impl DfpnSolver {
    pub fn new(node_limit: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            table: HashMap::new(),
            nodes_searched: 0,
            node_limit,
            cancelled,
        }
    }

    /// 局面が詰むかどうかを df-pn で判定する
    ///
    /// pos は攻め方（先手）の手番であること。
    /// 攻め方が王手の連続で玉方を詰ませられるかを判定する。
    pub fn solve(&mut self, pos: &mut Position) -> DfpnResult {
        let hash = position_hash(pos);
        let mut path = Vec::new();
        path.push(hash);

        self.mid_or(pos, INF, INF, &mut path);

        let entry = self.table.get(&hash).copied().unwrap_or(DfpnEntry { pn: 1, dn: 1 });

        if entry.pn == 0 {
            DfpnResult::Proven
        } else if entry.dn == 0 {
            DfpnResult::Disproven
        } else {
            DfpnResult::Unknown
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn should_stop(&self) -> bool {
        self.nodes_searched >= self.node_limit || self.is_cancelled()
    }

    /// ORノード（攻め方）: 王手を掛ける手を生成し、1つでも詰めば成功
    fn mid_or(
        &mut self,
        pos: &mut Position,
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
    ) {
        // 子ノードの手を生成
        let check_moves = pos.generate_check_moves();

        let hash = position_hash(pos);

        if check_moves.is_empty() {
            // 王手の手がない → 詰ませられない（反証）
            self.table.insert(hash, DfpnEntry { pn: INF, dn: 0 });
            return;
        }

        // 子ノードの (pn, dn) を初期化
        let mut children: Vec<(crate::shogi::types::Move, u64, u32, u32)> = Vec::with_capacity(check_moves.len());
        for mv in &check_moves {
            let undo = pos.make_move(*mv);
            let child_hash = position_hash(pos);
            let (cpn, cdn) = self.lookup_and(pos, child_hash, path);
            pos.unmake_move(&undo);
            children.push((*mv, child_hash, cpn, cdn));
        }

        loop {
            if self.should_stop() {
                return;
            }

            // OR: pn = min{pn_c}, dn = sum{dn_c}
            let pn_n = children.iter().map(|c| c.2).min().unwrap_or(INF);
            let dn_n = children.iter().map(|c| c.3).fold(0u32, |acc, d| acc.saturating_add(d));

            // しきい値チェック
            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.table.insert(hash, DfpnEntry { pn: pn_n, dn: dn_n });
                return;
            }

            // 証明/反証が確定
            if pn_n == 0 || dn_n == 0 {
                self.table.insert(hash, DfpnEntry { pn: pn_n, dn: dn_n });
                return;
            }

            // 最善の子を選択（pn が最小の子）
            let (best_idx, _) = children
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| c.2)
                .unwrap();

            // 2番目に小さい pn を求める
            let pn_2nd = children
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best_idx)
                .map(|(_, c)| c.2)
                .min()
                .unwrap_or(INF);

            let best_dn = children[best_idx].3;

            // 子のしきい値を計算（1+ε trick）
            let child_pn_limit = pn_limit.min(pn_2nd.saturating_add(1).saturating_add(pn_2nd / 4));
            let child_dn_limit = dn_limit.saturating_sub(dn_n).saturating_add(best_dn);

            // 再帰探索
            let best_mv = children[best_idx].0;
            let undo = pos.make_move(best_mv);
            let child_hash = children[best_idx].1;

            path.push(child_hash);
            self.mid_and(pos, child_pn_limit, child_dn_limit, path);
            path.pop();

            pos.unmake_move(&undo);

            // 子の値を更新
            let entry = self.table.get(&child_hash).copied().unwrap_or(DfpnEntry { pn: 1, dn: 1 });
            children[best_idx].2 = entry.pn;
            children[best_idx].3 = entry.dn;
        }
    }

    /// ANDノード（玉方）: 王手回避手を生成し、全て詰めば証明
    fn mid_and(
        &mut self,
        pos: &mut Position,
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
    ) {
        self.nodes_searched += 1;

        let hash = position_hash(pos);

        // 王手回避手を生成（無駄合いをフィルタ）
        let evasions: Vec<_> = pos
            .generate_check_evasions()
            .into_iter()
            .filter(|mv| !pos.is_futile_interposition(mv))
            .collect();

        if evasions.is_empty() {
            // 合法手がない → 詰み（証明）
            self.table.insert(hash, DfpnEntry { pn: 0, dn: INF });
            return;
        }

        // 子ノードの (pn, dn) を初期化
        let mut children: Vec<(crate::shogi::types::Move, u64, u32, u32)> = Vec::with_capacity(evasions.len());
        for mv in &evasions {
            let undo = pos.make_move(*mv);
            let child_hash = position_hash(pos);
            let (cpn, cdn) = self.lookup_or(pos, child_hash, path);
            pos.unmake_move(&undo);
            children.push((*mv, child_hash, cpn, cdn));
        }

        loop {
            if self.should_stop() {
                return;
            }

            // AND: pn = sum{pn_c}, dn = min{dn_c}
            let pn_n = children.iter().map(|c| c.2).fold(0u32, |acc, p| acc.saturating_add(p));
            let dn_n = children.iter().map(|c| c.3).min().unwrap_or(INF);

            // しきい値チェック
            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.table.insert(hash, DfpnEntry { pn: pn_n, dn: dn_n });
                return;
            }

            // 証明/反証が確定
            if pn_n == 0 || dn_n == 0 {
                self.table.insert(hash, DfpnEntry { pn: pn_n, dn: dn_n });
                return;
            }

            // 最善の子を選択（dn が最小の子 = 最も反証しやすい = 弱点）
            let (best_idx, _) = children
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| c.3)
                .unwrap();

            // 2番目に小さい dn を求める
            let dn_2nd = children
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best_idx)
                .map(|(_, c)| c.3)
                .min()
                .unwrap_or(INF);

            let best_pn = children[best_idx].2;

            // 子のしきい値を計算（1+ε trick）
            let child_dn_limit = dn_limit.min(dn_2nd.saturating_add(1).saturating_add(dn_2nd / 4));
            let child_pn_limit = pn_limit.saturating_sub(pn_n).saturating_add(best_pn);

            // 再帰探索
            let best_mv = children[best_idx].0;
            let undo = pos.make_move(best_mv);
            let child_hash = children[best_idx].1;

            path.push(child_hash);
            self.mid_or(pos, child_pn_limit, child_dn_limit, path);
            path.pop();

            pos.unmake_move(&undo);

            // 子の値を更新
            let entry = self.table.get(&child_hash).copied().unwrap_or(DfpnEntry { pn: 1, dn: 1 });
            children[best_idx].2 = entry.pn;
            children[best_idx].3 = entry.dn;
        }
    }

    /// ORノードの初期 (pn, dn) を取得
    /// 転置表にあればそれを返し、なければ初期値を返す。
    /// 千日手（パス上の再訪）検出時は反証扱い。
    fn lookup_or(&self, _pos: &Position, hash: u64, path: &[u64]) -> (u32, u32) {
        // 千日手検出: 探索パス上に同じハッシュがあればループ → 反証
        if path.contains(&hash) {
            return (INF, 0);
        }

        if let Some(entry) = self.table.get(&hash) {
            (entry.pn, entry.dn)
        } else {
            (1, 1) // 未展開ノードの初期値
        }
    }

    /// ANDノードの初期 (pn, dn) を取得
    /// 千日手検出時は反証扱い（攻め方のループは失敗）。
    fn lookup_and(&self, _pos: &Position, hash: u64, path: &[u64]) -> (u32, u32) {
        // 千日手検出
        if path.contains(&hash) {
            return (INF, 0);
        }

        if let Some(entry) = self.table.get(&hash) {
            (entry.pn, entry.dn)
        } else {
            (1, 1) // 未展開ノードの初期値
        }
    }
}
