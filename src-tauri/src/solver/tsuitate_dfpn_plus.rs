use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::types::*;

// ============================================================================
// Constants
// ============================================================================

/// 証明数/反証数の上限（sum時のオーバーフロー防止）
const INF: u32 = u32::MAX / 2;

/// メタポジションのサイズ上限
const MAX_META_POSITIONS: usize = 5000;

/// 転置表のデフォルトサイズ（4Mエントリ）
const TT_DEFAULT_SIZE: usize = 1 << 22;

/// GC 発動の充填率
const GC_FILL_RATIO: f64 = 0.85;

/// GC で削除するエントリの割合
const GC_REMOVE_RATIO: f64 = 0.25;

/// TCA（Threshold Controlling Algorithm）の閾値倍率
const TCA_MULTIPLIER: f64 = 1.7;

/// 線形探索の最大ステップ数
const MAX_PROBE_STEPS: usize = 16;

// ============================================================================
// Transposition Table Entries
// ============================================================================

/// OR 転置表エントリ（MetaPosition ハッシュ → 証明/反証データ）
#[derive(Debug, Clone, Copy)]
struct OrEntry {
    key: u64,
    pn: u32,
    dn: u32,
    proof_hand: [u8; 7],
    generation: u8,
    amount: u32,
}

/// AND 転置表エントリ
#[derive(Debug, Clone, Copy)]
struct AndEntry {
    key: u64,
    pn: u32,
    dn: u32,
    proof_hand: [u8; 7],
    generation: u8,
    amount: u32,
    /// 二重カウント回避用ビットマスク
    /// bit i が 1 → 分岐 i は他の分岐と OR ハッシュが異なる → sum に加算
    /// bit i が 0 → 分岐 i は別の分岐と OR ハッシュが重複 → max で集約
    #[allow(dead_code)]
    sum_mask: u32,
}

// ============================================================================
// Fixed-Size Transposition Table with GC
// ============================================================================

/// 固定サイズ転置表（線形探索 + GC）
struct TransTable<T> {
    entries: Vec<Option<T>>,
    capacity: usize,
    count: usize,
    generation: u8,
}

/// TransTable の共通トレイト
trait TtEntry {
    fn key(&self) -> u64;
    fn amount(&self) -> u32;
}

impl TtEntry for OrEntry {
    fn key(&self) -> u64 { self.key }
    fn amount(&self) -> u32 { self.amount }
}

impl TtEntry for AndEntry {
    fn key(&self) -> u64 { self.key }
    fn amount(&self) -> u32 { self.amount }
}

impl<T: TtEntry + Clone + Copy> TransTable<T> {
    fn new(capacity: usize) -> Self {
        Self {
            entries: vec![None; capacity],
            capacity,
            count: 0,
            generation: 0,
        }
    }

    /// ハッシュキーからスロットインデックスを計算
    fn slot(&self, key: u64) -> usize {
        (key as usize) % self.capacity
    }

    /// キーで検索（線形探索、最大 MAX_PROBE_STEPS）
    fn lookup(&self, key: u64) -> Option<&T> {
        let start = self.slot(key);
        for step in 0..MAX_PROBE_STEPS {
            let idx = (start + step) % self.capacity;
            match &self.entries[idx] {
                Some(entry) if entry.key() == key => return Some(entry),
                None => return None,
                _ => continue,
            }
        }
        None
    }

    /// エントリを挿入（既存キーがあれば上書き、なければ空きスロットに挿入）
    fn insert(&mut self, entry: T) {
        let key = entry.key();
        let start = self.slot(key);

        // まず既存エントリを探す
        for step in 0..MAX_PROBE_STEPS {
            let idx = (start + step) % self.capacity;
            match &self.entries[idx] {
                Some(e) if e.key() == key => {
                    self.entries[idx] = Some(entry);
                    return;
                }
                None => {
                    self.entries[idx] = Some(entry);
                    self.count += 1;
                    self.maybe_gc();
                    return;
                }
                _ => continue,
            }
        }

        // MAX_PROBE_STEPS 以内に空きがない → 最も古い(amount最小の)エントリを上書き
        let mut min_amount = u32::MAX;
        let mut min_idx = start;
        for step in 0..MAX_PROBE_STEPS {
            let idx = (start + step) % self.capacity;
            if let Some(e) = &self.entries[idx] {
                if e.amount() < min_amount {
                    min_amount = e.amount();
                    min_idx = idx;
                }
            }
        }
        self.entries[min_idx] = Some(entry);
    }

    /// GC が必要かチェックし、必要なら実行
    fn maybe_gc(&mut self) {
        if self.count as f64 > self.capacity as f64 * GC_FILL_RATIO {
            self.collect_garbage();
        }
    }

    /// GC: amount ベースで古いエントリを削除
    fn collect_garbage(&mut self) {
        let target_remove = (self.count as f64 * GC_REMOVE_RATIO) as usize;
        if target_remove == 0 {
            return;
        }

        // サンプリングで閾値を決定（全エントリの amount を収集してソート）
        let mut amounts: Vec<u32> = self.entries.iter()
            .filter_map(|e| e.as_ref().map(|x| x.amount()))
            .collect();
        amounts.sort_unstable();

        let threshold = if target_remove < amounts.len() {
            amounts[target_remove]
        } else {
            u32::MAX
        };

        // 閾値以下のエントリを削除
        let mut removed = 0;
        for i in 0..self.capacity {
            if removed >= target_remove {
                break;
            }
            if let Some(e) = &self.entries[i] {
                if e.amount() <= threshold {
                    self.entries[i] = None;
                    removed += 1;
                }
            }
        }
        self.count -= removed;
        self.generation = self.generation.wrapping_add(1);
    }

    /// 全クリア
    fn clear(&mut self) {
        for entry in self.entries.iter_mut() {
            *entry = None;
        }
        self.count = 0;
        self.generation = self.generation.wrapping_add(1);
    }
}

// ============================================================================
// Hash Functions (same as existing solver)
// ============================================================================

fn meta_position_hash(meta: &MetaPosition) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|p| p.zobrist_hash)
        .collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

fn meta_board_set_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|pos| pos.hash_without_sente_hand())
        .collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

fn meta_board_only_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|pos| pos.hash_board_only())
        .collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

fn and_key(meta_hash: u64, mv: &Move) -> u64 {
    let mut h = DefaultHasher::new();
    h.write_u64(meta_hash);
    mv.hash(&mut h);
    h.finish()
}

// ============================================================================
// Proof Tree Types (for replay cache)
// ============================================================================

/// コンパクトな証明木ノード（リプレイキャッシュ用）
/// SolutionNode と異なり String を持たず、Move を直接格納
#[derive(Debug, Clone)]
enum ProofNode {
    Checkmate,
    Attack {
        mv: Move,
        #[allow(dead_code)]
        branches: Vec<(ProofObs, ProofNode)>,
    },
}

/// 証明木の観測タイプ（Observation の軽量版）
#[derive(Debug, Clone, PartialEq, Eq)]
enum ProofObs {
    Checkmate,
    Captured(u8, u8),
    NoCapture,
    Illegal,
}

// ============================================================================
// AND Expansion Cache
// ============================================================================

/// AND ノードの展開結果キャッシュ
/// mid_and が同一 AND ノードを再訪問する際の expand_defense_moves 再計算を回避
#[derive(Clone)]
struct CachedAndExpansion {
    obs_terminal: Vec<bool>,
    obs_hash: Vec<u64>,
    obs_metas: Vec<MetaPosition>,
    obs_depth_inc: Vec<u32>,
    obs_hands: Vec<[u8; 7]>,
    obs_types: Vec<ProofObs>,
    parent_hand: [u8; 7],
}

// ============================================================================
// TsuitateDfpnPlusSolver
// ============================================================================

/// 衝立詰将棋 df-pn+ ソルバー
///
/// KomoringHeights 参考の改良版:
/// - 固定サイズ転置表 + GC
/// - df-pn+ ヒューリスティック初期値
/// - TCA (Threshold Controlling Algorithm)
/// - 二重カウント回避
/// - proof_hand を TT エントリに統合格納
pub struct TsuitateDfpnPlusSolver {
    or_table: TransTable<OrEntry>,
    and_table: TransTable<AndEntry>,
    dominance_table: HashMap<u64, Vec<[u8; 7]>>,
    move_hints: HashMap<u64, Move>,
    /// 盤面のみのハッシュ → 証明木リスト（リプレイキャッシュ、複数手順対応）
    proof_cache: HashMap<u64, Vec<ProofNode>>,
    /// AND ノードの展開結果キャッシュ: and_key → 構造データ
    and_expansion_cache: HashMap<u64, CachedAndExpansion>,
    pub nodes_searched: u64,
    pub dominance_hits: u64,
    node_limit: u64,
    cancelled: Arc<AtomicBool>,
    excluded_root_moves: Vec<Move>,
    root_hash: Option<u64>,
    depth_limit: Option<u32>,
    // diagnostics
    pub gc_count: u64,
    pub tca_activations: u64,
    pub expand_defense_calls: u64,
    pub mid_and_calls: u64,
}

/// 探索結果（既存ソルバーと同じ）
pub use super::tsuitate_dfpn::TsuitateDfpnResult;

impl TsuitateDfpnPlusSolver {
    pub fn new(node_limit: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            or_table: TransTable::new(TT_DEFAULT_SIZE),
            and_table: TransTable::new(TT_DEFAULT_SIZE),
            dominance_table: HashMap::new(),
            move_hints: HashMap::new(),
            proof_cache: HashMap::new(),
            and_expansion_cache: HashMap::new(),
            nodes_searched: 0,
            dominance_hits: 0,
            node_limit,
            cancelled,
            excluded_root_moves: Vec::new(),
            root_hash: None,
            depth_limit: None,
            gc_count: 0,
            tca_activations: 0,
            expand_defense_calls: 0,
            mid_and_calls: 0,
        }
    }

    // ========================================================================
    // Public API
    // ========================================================================

    /// メタポジションが詰むかどうかを判定する
    pub fn solve(&mut self, meta: &MetaPosition) -> TsuitateDfpnResult {
        let hash = meta_position_hash(meta);
        self.root_hash = Some(hash);
        let mut path = vec![hash];

        self.mid_or(meta, INF, INF, &mut path, 0);

        let entry = self.or_table.lookup(hash);
        let (pn, dn) = entry.map_or((1, 1), |e| (e.pn, e.dn));
        if pn == 0 {
            TsuitateDfpnResult::Proven
        } else if dn == 0 {
            TsuitateDfpnResult::Disproven
        } else {
            TsuitateDfpnResult::Unknown
        }
    }

    pub fn solve_to_solution(&mut self, meta: &MetaPosition, find_shortest: bool) -> SolutionData {
        self.solve_to_solution_inner(meta, false, find_shortest)
    }

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

                let (tree, depth, shorten_nodes) = if find_shortest && depth > 1 {
                    self.shorten_solution(meta, tree, depth)
                } else {
                    (tree, depth, 0)
                };
                let nodes_before_second = initial_nodes + shorten_nodes;

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

    // ========================================================================
    // Core: mid_or (OR node search)
    // ========================================================================

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

        // Terminal checks
        if meta.is_empty() {
            self.or_table.insert(OrEntry {
                key: hash, pn: INF, dn: 0, proof_hand: [0; 7],
                generation: self.or_table.generation, amount: self.nodes_searched as u32,
            });
            return;
        }
        if meta.all_effectively_checkmate() {
            let proof_hand = [0u8; 7];
            self.or_table.insert(OrEntry {
                key: hash, pn: 0, dn: INF, proof_hand,
                generation: self.or_table.generation, amount: self.nodes_searched as u32,
            });
            if !meta.positions.is_empty() {
                let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
                let board_hash = meta_board_set_hash(meta);
                self.add_dominance_entry(board_hash, sente_hand);
            }
            return;
        }
        if meta.positions.len() > MAX_META_POSITIONS {
            self.or_table.insert(OrEntry {
                key: hash, pn: INF, dn: 0, proof_hand: [0; 7],
                generation: self.or_table.generation, amount: self.nodes_searched as u32,
            });
            return;
        }

        // Depth limit
        if let Some(limit) = self.depth_limit {
            if depth >= limit {
                self.or_table.insert(OrEntry {
                    key: hash, pn: INF, dn: 0, proof_hand: [0; 7],
                    generation: self.or_table.generation, amount: self.nodes_searched as u32,
                });
                return;
            }
        }

        // Dominance check
        if !meta.positions.is_empty() {
            let board_hash = meta_board_set_hash(meta);
            let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
            if let Some(entries) = self.dominance_table.get(&board_hash) {
                if let Some(matched_ph) = entries
                    .iter()
                    .find(|ph| ph.iter().zip(sente_hand.iter()).all(|(p, s)| p <= s))
                {
                    self.or_table.insert(OrEntry {
                        key: hash, pn: 0, dn: INF, proof_hand: *matched_ph,
                        generation: self.or_table.generation, amount: self.nodes_searched as u32,
                    });
                    self.dominance_hits += 1;
                    return;
                }
            }
        }

        // Generate candidates
        let (mut candidates, legal_move_sets) = self.generate_attack_candidates(meta);
        if self.should_stop() {
            return;
        }

        // Filter excluded root moves
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
            self.or_table.insert(OrEntry {
                key: hash, pn: INF, dn: 0, proof_hand: [0; 7],
                generation: self.or_table.generation, amount: self.nodes_searched as u32,
            });
            return;
        }

        // Move hint reordering
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

        // TCA: check generation mismatch
        let mut inc_flag = 0u32;
        if let Some(existing) = self.or_table.lookup(hash) {
            if existing.generation != self.or_table.generation {
                inc_flag += 1;
            }
        }

        // Initialize children from AND table
        let mut children: Vec<(Move, u64, u32, u32)> = Vec::with_capacity(candidates.len());
        for mv in &candidates {
            let ak = and_key(hash, mv);
            let (cpn, cdn) = if let Some(e) = self.and_table.lookup(ak) {
                if e.generation != self.and_table.generation {
                    inc_flag += 1;
                }
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

            // OR aggregation: pn = min{pn_c}, dn = sum{dn_c}
            let pn_n = children.iter().map(|c| c.2).min().unwrap_or(INF);
            let dn_n = children.iter().map(|c| c.3).fold(0u32, |a, d| a.saturating_add(d));

            if pn_n == 0 || dn_n == 0 {
                let proof_hand = if pn_n == 0 {
                    self.compute_or_proof_hand(meta, &children, hash)
                } else {
                    [0; 7]
                };
                self.or_table.insert(OrEntry {
                    key: hash, pn: pn_n, dn: dn_n, proof_hand,
                    generation: self.or_table.generation, amount: self.nodes_searched as u32,
                });
                if pn_n == 0 {
                    self.record_proven_dominance(meta, proof_hand);
                    // Record move hint and proof cache
                    if !meta.positions.is_empty() {
                        let board_only_hash = meta_board_only_hash(meta);
                        if let Some(proven_child) = children.iter().find(|c| c.2 == 0) {
                            self.move_hints.insert(board_only_hash, proven_child.0);
                        }
                        // 証明木をキャッシュ（リプレイ用、複数手順対応）
                        // extract_compact_or/and は and_expansion_cache を使用するため高速
                        {
                            let max_proofs = 20;
                            let should_add = self.proof_cache
                                .get(&board_only_hash)
                                .map_or(true, |v| v.len() < max_proofs);
                            if should_add {
                                if let Some(proof) = self.extract_compact_or(meta) {
                                    self.proof_cache.entry(board_only_hash)
                                        .or_default()
                                        .push(proof);
                                }
                            }
                        }
                    }
                }
                return;
            }

            // TCA: expand thresholds if stale entries detected
            let (eff_pn_limit, eff_dn_limit) = if inc_flag > 0 {
                self.tca_activations += 1;
                let expanded_pn = ((pn_limit as f64) * TCA_MULTIPLIER) as u32;
                let expanded_dn = ((dn_limit as f64) * TCA_MULTIPLIER) as u32;
                (expanded_pn.min(INF), expanded_dn.min(INF))
            } else {
                (pn_limit, dn_limit)
            };

            if pn_n >= eff_pn_limit || dn_n >= eff_dn_limit {
                self.or_table.insert(OrEntry {
                    key: hash, pn: pn_n, dn: dn_n, proof_hand: [0; 7],
                    generation: self.or_table.generation, amount: self.nodes_searched as u32,
                });
                return;
            }

            // Select best child (min pn)
            let (best_idx, _) = children.iter().enumerate().min_by_key(|(_, c)| c.2).unwrap();
            let pn_2nd = children
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best_idx)
                .map(|(_, c)| c.2)
                .min()
                .unwrap_or(INF);
            let best_dn = children[best_idx].3;

            // Child thresholds (1+epsilon trick)
            let child_pn_limit =
                eff_pn_limit.min(pn_2nd.saturating_add(1).saturating_add(pn_2nd / 4));
            let child_dn_limit = eff_dn_limit.saturating_sub(dn_n).saturating_add(best_dn);

            // Recurse into AND node
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

            // Read back from AND table
            let ak = children[best_idx].1;
            let e = self.and_table.lookup(ak);
            let (cpn, cdn) = e.map_or((1, 1), |e| (e.pn, e.dn));
            children[best_idx].2 = cpn;
            children[best_idx].3 = cdn;

            // Reset inc_flag after first iteration
            inc_flag = 0;
        }
    }

    // ========================================================================
    // Core: mid_and (AND node search)
    // ========================================================================

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
        self.mid_and_calls += 1;
        let meta_hash = meta_position_hash(meta);
        let ak = and_key(meta_hash, &mv);

        // Phase 1: キャッシュから展開結果を取得、なければ計算
        // remove で所有権を取り、ループ後に再挿入（borrow checker 回避）
        let expansion = if let Some(cached) = self.and_expansion_cache.remove(&ak) {
            cached
        } else {
            // Apply move and split into legal/illegal
            let (legal_meta, illegal_meta) =
                meta.apply_attack_move_split_with_sets(mv, legal_move_sets);

            if legal_meta.is_empty() && illegal_meta.is_empty() {
                self.and_table.insert(AndEntry {
                    key: ak, pn: INF, dn: 0, proof_hand: [0; 7],
                    generation: self.and_table.generation, amount: self.nodes_searched as u32,
                    sum_mask: 0,
                });
                return;
            }

            // Check all legal positions are in check
            if !legal_meta.is_empty()
                && !legal_meta.positions.iter().all(|pos| pos.is_in_check(pos.side_to_move))
            {
                self.and_table.insert(AndEntry {
                    key: ak, pn: INF, dn: 0, proof_hand: [0; 7],
                    generation: self.and_table.generation, amount: self.nodes_searched as u32,
                    sum_mask: 0,
                });
                return;
            }

            let parent_hand = Self::parent_sente_hand(meta, [0; 7]);

            let mut obs_terminal: Vec<bool> = Vec::new();
            let mut obs_hash: Vec<u64> = Vec::new();
            let mut obs_metas: Vec<MetaPosition> = Vec::new();
            let mut obs_depth_inc: Vec<u32> = Vec::new();
            let mut obs_hands: Vec<[u8; 7]> = Vec::new();
            let mut obs_types: Vec<ProofObs> = Vec::new();

            // Illegal branch
            if !illegal_meta.is_empty() {
                let bh = meta_position_hash(&illegal_meta);
                obs_terminal.push(false);
                obs_hash.push(bh);
                obs_hands.push(parent_hand);
                obs_metas.push(illegal_meta);
                obs_depth_inc.push(0);
                obs_types.push(ProofObs::Illegal);
            }

            // Legal branches
            if !legal_meta.is_empty() {
                if legal_meta.all_effectively_checkmate() {
                    if !legal_meta.positions.is_empty() {
                        let board_hash = meta_board_set_hash(&legal_meta);
                        self.add_dominance_entry(board_hash, [0; 7]);
                    }
                    let legal_hand = legal_meta.positions.first()
                        .map(|p| p.hand(Color::Sente).counts_array())
                        .unwrap_or(parent_hand);
                    obs_terminal.push(true);
                    obs_hash.push(0);
                    obs_hands.push(legal_hand);
                    obs_metas.push(MetaPosition { positions: Vec::new() });
                    obs_depth_inc.push(0);
                    obs_types.push(ProofObs::Checkmate);
                } else {
                    self.expand_defense_calls += 1;
                    let raw_branches = legal_meta.expand_defense_moves(mv);
                    let branches = Self::merge_observation_branches(raw_branches);

                    for (obs, branch_meta) in branches {
                        match obs {
                            Observation::Checkmate => {
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                obs_terminal.push(true);
                                obs_hash.push(0);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(0);
                                obs_types.push(ProofObs::Checkmate);
                            }
                            Observation::Captured { file, rank } => {
                                if branch_meta.positions.len() > MAX_META_POSITIONS {
                                    self.and_table.insert(AndEntry {
                                        key: ak, pn: INF, dn: 0, proof_hand: [0; 7],
                                        generation: self.and_table.generation,
                                        amount: self.nodes_searched as u32, sum_mask: 0,
                                    });
                                    return;
                                }
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                let bh = meta_position_hash(&branch_meta);
                                obs_terminal.push(false);
                                obs_hash.push(bh);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(2);
                                obs_types.push(ProofObs::Captured(file, rank));
                            }
                            Observation::NoCapture => {
                                if branch_meta.positions.len() > MAX_META_POSITIONS {
                                    self.and_table.insert(AndEntry {
                                        key: ak, pn: INF, dn: 0, proof_hand: [0; 7],
                                        generation: self.and_table.generation,
                                        amount: self.nodes_searched as u32, sum_mask: 0,
                                    });
                                    return;
                                }
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                let bh = meta_position_hash(&branch_meta);
                                obs_terminal.push(false);
                                obs_hash.push(bh);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(2);
                                obs_types.push(ProofObs::NoCapture);
                            }
                            Observation::Illegal => {}
                        }
                    }
                }
            }

            if obs_terminal.is_empty() {
                self.and_table.insert(AndEntry {
                    key: ak, pn: INF, dn: 0, proof_hand: [0; 7],
                    generation: self.and_table.generation, amount: self.nodes_searched as u32,
                    sum_mask: 0,
                });
                return;
            }

            CachedAndExpansion {
                obs_terminal,
                obs_hash,
                obs_metas,
                obs_depth_inc,
                obs_hands,
                obs_types,
                parent_hand,
            }
        };

        let n = expansion.obs_terminal.len();

        // Compute sum_mask for double-counting avoidance
        let sum_mask = self.compute_sum_mask(&expansion.obs_terminal, &expansion.obs_hash);

        // Phase 2: 転置表から最新の pn/dn を取得
        let mut obs_pn: Vec<u32> = Vec::with_capacity(n);
        let mut obs_dn: Vec<u32> = Vec::with_capacity(n);
        for i in 0..n {
            if expansion.obs_terminal[i] {
                obs_pn.push(0);
                obs_dn.push(INF);
            } else {
                let (cpn, cdn) = self.lookup_or(expansion.obs_hash[i], path);
                obs_pn.push(cpn);
                obs_dn.push(cdn);
            }
        }

        // Phase 3: メインループ（break で統一して最後にキャッシュ再挿入）
        loop {
            if self.should_stop() {
                break;
            }

            // AND aggregation with double-counting avoidance
            let pn_n = Self::compute_and_pn(&obs_pn, sum_mask);
            let dn_n = obs_dn.iter().copied().min().unwrap_or(INF);

            if pn_n == 0 || dn_n == 0 {
                let proof_hand = if pn_n == 0 {
                    self.compute_and_proof_hand(
                        &expansion.obs_terminal,
                        &expansion.obs_hash,
                        &expansion.obs_hands,
                        expansion.parent_hand,
                    )
                } else {
                    [0; 7]
                };
                self.and_table.insert(AndEntry {
                    key: ak, pn: pn_n, dn: dn_n, proof_hand,
                    generation: self.and_table.generation, amount: self.nodes_searched as u32,
                    sum_mask,
                });
                break;
            }

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.and_table.insert(AndEntry {
                    key: ak, pn: pn_n, dn: dn_n, proof_hand: [0; 7],
                    generation: self.and_table.generation, amount: self.nodes_searched as u32,
                    sum_mask,
                });
                break;
            }

            // Select worst child (min dn)
            let best = (0..n)
                .filter(|&i| !expansion.obs_terminal[i])
                .min_by_key(|&i| obs_dn[i]);
            let best_idx = match best {
                Some(i) => i,
                None => {
                    self.and_table.insert(AndEntry {
                        key: ak, pn: pn_n, dn: dn_n, proof_hand: [0; 7],
                        generation: self.and_table.generation,
                        amount: self.nodes_searched as u32, sum_mask,
                    });
                    break;
                }
            };

            let dn_2nd = (0..n)
                .filter(|&i| i != best_idx)
                .map(|i| obs_dn[i])
                .min()
                .unwrap_or(INF);
            let best_pn = obs_pn[best_idx];

            // Child thresholds (1+epsilon)
            let child_dn_limit =
                dn_limit.min(dn_2nd.saturating_add(1).saturating_add(dn_2nd / 4));
            let child_pn_limit = pn_limit.saturating_sub(pn_n).saturating_add(best_pn);

            // Recurse into OR node
            let child_hash = expansion.obs_hash[best_idx];
            let child_depth = depth + expansion.obs_depth_inc[best_idx];
            path.push(child_hash);
            self.mid_or(
                &expansion.obs_metas[best_idx],
                child_pn_limit,
                child_dn_limit,
                path,
                child_depth,
            );
            path.pop();

            // Read back
            let e = self.or_table.lookup(child_hash);
            let (cpn, cdn) = e.map_or((1, 1), |e| (e.pn, e.dn));
            obs_pn[best_idx] = cpn;
            obs_dn[best_idx] = cdn;
        }

        // Phase 4: キャッシュに再挿入
        self.and_expansion_cache.insert(ak, expansion);
    }

    // ========================================================================
    // Double-counting avoidance
    // ========================================================================

    /// Compute sum_mask: bit i = 1 if branch i has a unique OR hash
    /// When multiple branches converge to the same OR node, we use max instead of sum
    fn compute_sum_mask(&self, obs_terminal: &[bool], obs_hash: &[u64]) -> u32 {
        let n = obs_terminal.len();
        if n >= 32 {
            // Too many branches for bitmask; treat all as unique (no dedup)
            return u32::MAX;
        }
        let mut mask: u32 = (1u32 << n) - 1; // start with all bits set

        for i in 0..n {
            if obs_terminal[i] {
                continue; // terminal branches always contribute (pn=0)
            }
            for j in (i + 1)..n {
                if obs_terminal[j] {
                    continue;
                }
                if obs_hash[i] == obs_hash[j] {
                    // Duplicate: clear the higher-indexed bit
                    mask &= !(1u32 << j);
                }
            }
        }
        mask
    }

    /// AND pn aggregation with sum_mask
    /// For branches with sum_mask bit set: sum their pn
    /// For branches with sum_mask bit clear: take max with the group representative
    fn compute_and_pn(obs_pn: &[u32], sum_mask: u32) -> u32 {
        let n = obs_pn.len();
        let mut result: u32 = 0;
        for i in 0..n {
            if (sum_mask >> i) & 1 == 1 {
                result = result.saturating_add(obs_pn[i]);
            }
            // Branches with cleared bits are duplicates; their pn is already
            // counted via the representative branch (the one with bit set).
            // We take max to be safe:
            // If the duplicate has higher pn, we should use it.
        }

        // For correctness with max semantics on duplicates:
        // Find groups of duplicate hashes and ensure we use max pn within each group
        // This is implicitly handled because cleared-bit branches have the same OR hash,
        // so their TT lookup returns the same pn value.
        result
    }

    // ========================================================================
    // Proof hand computation
    // ========================================================================

    fn compute_or_proof_hand(
        &self,
        meta: &MetaPosition,
        children: &[(Move, u64, u32, u32)],
        _hash: u64,
    ) -> [u8; 7] {
        let fallback_ph = meta.positions.first()
            .map(|p| p.hand(Color::Sente).counts_array())
            .unwrap_or([0; 7]);
        let mut or_ph = [u8::MAX; 7];
        for child in children {
            if child.2 == 0 {
                let and_ph = self.and_table.lookup(child.1)
                    .map(|e| e.proof_hand)
                    .unwrap_or(fallback_ph);
                for j in 0..7 {
                    or_ph[j] = or_ph[j].min(and_ph[j]);
                }
            }
        }
        if or_ph[0] == u8::MAX {
            or_ph = fallback_ph;
        }
        or_ph
    }

    fn compute_and_proof_hand(
        &self,
        obs_terminal: &[bool],
        obs_hash: &[u64],
        obs_hands: &[[u8; 7]],
        parent_hand: [u8; 7],
    ) -> [u8; 7] {
        let n = obs_terminal.len();
        let mut and_ph = [0u8; 7];
        for i in 0..n {
            let child_ph = if obs_terminal[i] {
                [0u8; 7]
            } else {
                self.or_table.lookup(obs_hash[i])
                    .map(|e| e.proof_hand)
                    .unwrap_or(parent_hand)
            };
            let child_hand = obs_hands[i];
            for j in 0..7 {
                let eff = (child_ph[j] as i16) + (parent_hand[j] as i16)
                    - (child_hand[j] as i16);
                let eff = eff.max(0) as u8;
                and_ph[j] = and_ph[j].max(eff);
            }
        }
        and_ph
    }

    // ========================================================================
    // OR lookup with loop detection
    // ========================================================================

    fn lookup_or(&self, hash: u64, path: &[u64]) -> (u32, u32) {
        if path.contains(&hash) {
            return (INF, 0);
        }
        if let Some(e) = self.or_table.lookup(hash) {
            (e.pn, e.dn)
        } else {
            (1, 1)
        }
    }

    // ========================================================================
    // Helpers (same logic as existing solver)
    // ========================================================================

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn should_stop(&self) -> bool {
        self.nodes_searched >= self.node_limit || self.is_cancelled()
    }

    /// Observation branch merging (same as existing)
    const MAX_MERGE_POSITIONS: usize = 7;

    fn merge_observation_branches(
        branches: Vec<(Observation, MetaPosition)>,
    ) -> Vec<(Observation, MetaPosition)> {
        use super::metaposition::position_hash;

        let mut obs_counts: HashMap<Observation, usize> = HashMap::new();
        let mut obs_total_positions: HashMap<Observation, usize> = HashMap::new();
        for (obs, meta) in &branches {
            *obs_counts.entry(obs.clone()).or_default() += 1;
            *obs_total_positions.entry(obs.clone()).or_default() += meta.positions.len();
        }

        let mut result: Vec<(Observation, MetaPosition)> = Vec::new();
        for (obs, meta) in branches {
            let count = obs_counts.get(&obs).copied().unwrap_or(0);
            let total_pos = obs_total_positions.get(&obs).copied().unwrap_or(0);
            if count >= 3 && total_pos <= Self::MAX_MERGE_POSITIONS {
                if let Some(existing) = result.iter_mut().find(|(o, _)| *o == obs) {
                    let mut seen: HashSet<u64> = existing.1.positions.iter()
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
                result.push((obs, meta));
            }
        }
        result
    }

    fn child_sente_hand(meta: &MetaPosition, default: [u8; 7]) -> [u8; 7] {
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
        let mut result = first;
        for pos in &meta.positions[1..] {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].min(h[j]);
            }
        }
        result
    }

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
        let mut result = first;
        for pos in &meta.positions {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].max(h[j]);
            }
        }
        result
    }

    fn record_proven_dominance(&mut self, meta: &MetaPosition, proof_hand: [u8; 7]) {
        if meta.positions.is_empty() {
            return;
        }
        let board_hash = meta_board_set_hash(meta);
        self.add_dominance_entry(board_hash, proof_hand);
    }

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

    fn generate_attack_candidates(
        &self,
        meta: &MetaPosition,
    ) -> (Vec<Move>, Vec<HashSet<Move>>) {
        if meta.positions.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let n = meta.positions.len();

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

        // Sort candidates
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

        // Filter out long-range checks with too many interposition squares
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

        if n <= 1 {
            return (check_moves, legal_move_sets);
        }

        // Probe moves
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
    // Solution extraction
    // ========================================================================

    pub fn extract_solution(&self, meta: &MetaPosition) -> Option<SolutionNode> {
        self.extract_or(meta, 0)
    }

    fn extract_or(&self, meta: &MetaPosition, depth: u32) -> Option<SolutionNode> {
        if meta.is_empty() {
            return None;
        }
        if meta.all_effectively_checkmate() {
            return Some(SolutionNode::Checkmate { depth });
        }
        // 深さガード（指数爆発防止）
        if depth > 200 {
            return None;
        }

        let hash = meta_position_hash(meta);
        let entry = self.or_table.lookup(hash);
        if entry.map_or(true, |e| e.pn != 0) {
            return None;
        }

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

        // Phase 1: TT ベース（AND pn=0 を探す）
        for mv in &all_moves {
            let ak = and_key(hash, mv);
            if let Some(and_entry) = self.and_table.lookup(ak) {
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

        // Phase 2: move_hints で直接参照
        if !meta.positions.is_empty() {
            let board_only_hash = meta_board_only_hash(meta);
            if let Some(hint_mv) = self.move_hints.get(&board_only_hash).copied() {
                if all_moves.contains(&hint_mv) {
                    if let Some(branches) = self.extract_and(meta, hint_mv, &legal_move_sets, depth) {
                        return Some(SolutionNode::AttackMove {
                            mv: MoveData::from_move(hint_mv, Color::Sente),
                            branches,
                        });
                    }
                }
            }

            // Phase 3: proof_cache で証明手を参照
            if let Some(proofs) = self.proof_cache.get(&board_only_hash) {
                for proof in proofs {
                    if let ProofNode::Attack { mv, .. } = proof {
                        if all_moves.contains(mv) {
                            if let Some(branches) = self.extract_and(meta, *mv, &legal_move_sets, depth) {
                                return Some(SolutionNode::AttackMove {
                                    mv: MoveData::from_move(*mv, Color::Sente),
                                    branches,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Phase 4: 全候補手フォールバック
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

    fn proof_obs_to_observation(po: &ProofObs) -> Observation {
        match po {
            ProofObs::Checkmate => Observation::Checkmate,
            ProofObs::Captured(f, r) => Observation::Captured { file: *f, rank: *r },
            ProofObs::NoCapture => Observation::NoCapture,
            ProofObs::Illegal => Observation::Illegal,
        }
    }

    fn extract_and(
        &self,
        meta: &MetaPosition,
        mv: Move,
        legal_move_sets: &[HashSet<Move>],
        depth: u32,
    ) -> Option<Vec<SolutionBranch>> {
        let meta_hash = meta_position_hash(meta);
        let ak = and_key(meta_hash, &mv);

        // キャッシュヒット: expand_defense_moves を回避
        if let Some(expansion) = self.and_expansion_cache.get(&ak) {
            let mut branches = Vec::new();
            for i in 0..expansion.obs_terminal.len() {
                let obs = Self::proof_obs_to_observation(&expansion.obs_types[i]);
                if expansion.obs_terminal[i] {
                    branches.push(SolutionBranch {
                        observation: obs,
                        continuation: Box::new(SolutionNode::Checkmate { depth: depth + 1 }),
                    });
                } else {
                    let d_inc = expansion.obs_depth_inc[i];
                    let continuation = self.extract_or(&expansion.obs_metas[i], depth + d_inc)?;
                    branches.push(SolutionBranch {
                        observation: obs,
                        continuation: Box::new(continuation),
                    });
                }
            }
            return if branches.is_empty() { None } else { Some(branches) };
        }

        // キャッシュミス: 既存ロジック（expand_defense_moves 使用）
        let (legal_meta, illegal_meta) =
            meta.apply_attack_move_split_with_sets(mv, legal_move_sets);

        let mut branches = Vec::new();

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

        if legal_meta.all_effectively_checkmate() {
            branches.push(SolutionBranch {
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: depth + 1 }),
            });
            return Some(branches);
        }

        let raw_obs_branches = legal_meta.expand_defense_moves(mv);
        let obs_branches = Self::merge_observation_branches(raw_obs_branches);
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

    // ========================================================================
    // Shorten / Second solution
    // ========================================================================

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

            self.or_table.clear();
            self.and_table.clear();
            self.dominance_table.clear();
            self.move_hints.clear();
            self.proof_cache.clear();
            self.and_expansion_cache.clear();

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

    fn find_second_solution(
        &mut self,
        meta: &MetaPosition,
        first_tree: &SolutionNode,
    ) -> (Option<SolutionNode>, u64, String) {
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

        let mut excluded = vec![first_mv];
        if first_mv.from.is_some() {
            let mut counterpart = first_mv;
            counterpart.promotion = !counterpart.promotion;
            excluded.push(counterpart);
        }

        let first_phase_nodes = self.nodes_searched;

        self.or_table.clear();
        self.and_table.clear();
        self.dominance_table.clear();
        self.move_hints.clear();
        self.proof_cache.clear();
        self.and_expansion_cache.clear();
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

    // ========================================================================
    // Compact proof tree extraction (for replay cache)
    // ========================================================================

    /// コンパクト証明木を抽出（リプレイキャッシュ用）
    fn extract_compact_or(&self, meta: &MetaPosition) -> Option<ProofNode> {
        if meta.is_empty() { return None; }
        if meta.all_effectively_checkmate() { return Some(ProofNode::Checkmate); }

        let hash = meta_position_hash(meta);
        let entry = self.or_table.lookup(hash);
        let is_proven = entry.map_or(false, |e| e.pn == 0);
        if !is_proven { return None; }

        // 全合法手を収集
        let legal_move_sets: Vec<HashSet<Move>> = meta.positions.iter()
            .map(|pos| pos.generate_legal_moves().into_iter().collect())
            .collect();
        let mut seen = HashSet::new();
        let mut all_moves = Vec::new();
        for set in &legal_move_sets {
            for mv in set {
                if seen.insert(*mv) { all_moves.push(*mv); }
            }
        }

        // pn=0 の手を探す
        for mv in &all_moves {
            let ak = and_key(hash, mv);
            if let Some(e) = self.and_table.lookup(ak) {
                if e.pn == 0 {
                    if let Some(branches) = self.extract_compact_and(meta, *mv, &legal_move_sets) {
                        return Some(ProofNode::Attack { mv: *mv, branches });
                    }
                }
            }
        }

        // move_hints で直接参照
        if !meta.positions.is_empty() {
            let board_only_hash = meta_board_only_hash(meta);
            if let Some(hint_mv) = self.move_hints.get(&board_only_hash).copied() {
                if all_moves.contains(&hint_mv) {
                    if let Some(branches) = self.extract_compact_and(meta, hint_mv, &legal_move_sets) {
                        return Some(ProofNode::Attack { mv: hint_mv, branches });
                    }
                }
            }
        }

        // dominance で証明されたノード: 各候補手で直接試す
        for mv in &all_moves {
            if let Some(branches) = self.extract_compact_and(meta, *mv, &legal_move_sets) {
                return Some(ProofNode::Attack { mv: *mv, branches });
            }
        }
        None
    }

    fn extract_compact_and(
        &self,
        meta: &MetaPosition,
        mv: Move,
        _legal_move_sets: &[HashSet<Move>],
    ) -> Option<Vec<(ProofObs, ProofNode)>> {
        // and_expansion_cache を使って expand_defense_moves の再呼び出しを回避
        let meta_hash = meta_position_hash(meta);
        let ak = and_key(meta_hash, &mv);

        let expansion = self.and_expansion_cache.get(&ak)?;
        let n = expansion.obs_terminal.len();
        let mut branches = Vec::new();

        for i in 0..n {
            let obs_type = expansion.obs_types[i].clone();
            if expansion.obs_terminal[i] {
                branches.push((obs_type, ProofNode::Checkmate));
            } else {
                let node = self.extract_compact_or(&expansion.obs_metas[i])?;
                branches.push((obs_type, node));
            }
        }

        if branches.is_empty() { None } else { Some(branches) }
    }

}
