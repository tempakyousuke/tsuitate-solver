use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::types::*;

/// MetaPosition のハッシュキー（転置表用）
/// 各 Position のハッシュを XOR で結合し、順序に依存しないキーを生成（O(N)）
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

/// メタポジションのサイズ上限（これを超える分岐は枝刈り）
const MAX_META_POSITIONS: usize = 5000;

/// ハイブリッドIDS+DFS型衝立詰将棋ソルバー
/// ルートレベルでは反復深化（IDS）で浅い解を素早く発見し、
/// サブツリーではDFSで再探索を避ける
pub struct DfsTsuitateSolver {
    /// 最大探索深さ
    max_depth: u32,
    /// 探索ノード数
    pub nodes_searched: u64,
    /// 探索ログ
    pub trace: Vec<String>,
    /// トレース有効フラグ
    trace_enabled: bool,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
    /// 2つ目の解を探すかどうか
    find_second_solution: bool,
    /// 直前の探索で見つかったルートレベルの手（除外用）
    last_root_move: Option<Move>,
    /// 転置表: 失敗したメタポジションのハッシュ → 失敗した最大remaining_depth
    /// remaining_depth ≤ stored_depth なら再探索不要
    fail_table: HashMap<u64, u32>,
}

impl DfsTsuitateSolver {
    pub fn new(max_depth: u32, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            max_depth,
            nodes_searched: 0,
            trace: Vec::new(),
            trace_enabled: true,
            cancelled,
            find_second_solution: false,
            last_root_move: None,
            fail_table: HashMap::new(),
        }
    }

    pub fn set_find_second_solution(&mut self, find: bool) {
        self.find_second_solution = find;
    }

    pub fn set_trace_enabled(&mut self, enabled: bool) {
        self.trace_enabled = enabled;
    }

    fn log(&mut self, depth: u32, msg: String) {
        if !self.trace_enabled && depth > 2 {
            return;
        }
        let indent = "  ".repeat(depth as usize);
        self.trace.push(format!("{}{}", indent, msg));
    }

    /// ルートレベルのみログ出力
    fn log_root(&mut self, msg: String) {
        self.trace.push(msg);
    }

    /// 探索深さのシーケンスを生成（奇数のみ: 1, 3, 5, 7, ...）
    fn build_depth_sequence(max_depth: u32) -> Vec<u32> {
        let mut depths = Vec::new();
        let mut d = 1;
        while d <= max_depth {
            depths.push(d);
            d += 2;
        }
        // 最後が max_depth でなければ追加
        if depths.last().copied() != Some(max_depth) && max_depth % 2 == 1 {
            depths.push(max_depth);
        }
        depths
    }

    /// 衝立詰将棋を解く（ハイブリッドIDS+DFS）
    /// ルートレベルでは反復深化で浅い解を素早く発見
    pub fn solve(&mut self, meta: &MetaPosition) -> SolutionData {
        self.log(0, format!("開始(IDS+DFS): メタポジション数={}", meta.positions.len()));

        // 最大深さを奇数に正規化（攻め方の手で終わる必要があるため）
        let effective_max = if self.max_depth % 2 == 0 {
            self.max_depth - 1
        } else {
            self.max_depth
        };

        let depths = Self::build_depth_sequence(effective_max);
        self.log(0, format!("IDS+DFS探索: 深さシーケンス={:?}", depths));

        // 第1フェーズ: 反復深化で最初の解を探す
        // 転置表は反復間で保持（remaining_depth ベースで安全に再利用可能）
        let mut first_tree = None;
        for &search_depth in &depths {
            if self.is_cancelled() {
                break;
            }
            self.log(0, format!(
                "--- 深さ{}手で探索開始 (転置表サイズ: {}) ---",
                search_depth, self.fail_table.len()
            ));
            if let Some(tree) = self.dfs_solve_attack(meta, 0, search_depth, &[]) {
                first_tree = Some(tree);
                break;
            }
        }

        if self.is_cancelled() {
            let msg = format!(
                "探索を中止しました (探索ノード数: {})",
                self.nodes_searched
            );
            self.log(0, format!("結果: {}", msg));
            return SolutionData {
                found: false,
                tree: None,
                second_tree: None,
                message: msg,
                trace: std::mem::take(&mut self.trace),
            };
        }

        let Some(tree) = first_tree else {
            let msg = format!(
                "{}手以内の詰みは見つかりませんでした (探索ノード数: {})",
                effective_max, self.nodes_searched
            );
            self.log(0, format!("結果: {}", msg));
            return SolutionData {
                found: false,
                tree: None,
                second_tree: None,
                message: msg,
                trace: std::mem::take(&mut self.trace),
            };
        };

        let actual_depth = tree.max_moves();

        // 第2フェーズ: 2つ目の解を探す（オプション）
        if self.find_second_solution {
            if let Some(first_mv) = self.last_root_move {
                // 成/不成の違いだけの手順は余詰として扱わない
                let mut excluded = vec![first_mv];
                if first_mv.from.is_some() {
                    let mut counterpart = first_mv;
                    counterpart.promotion = !counterpart.promotion;
                    excluded.push(counterpart);
                }
                self.log(0, format!(
                    "--- 2つ目の解を探索（初手 {} および成/不成を除外）---",
                    first_mv.to_japanese(Color::Sente)
                ));
                // 転置表をクリアして再探索（IDS）
                self.fail_table.clear();
                let mut second_tree = None;
                for &search_depth in &depths {
                    if self.is_cancelled() {
                        break;
                    }
                    if let Some(tree) = self.dfs_solve_attack(meta, 0, search_depth, &excluded) {
                        second_tree = Some(tree);
                        break;
                    }
                }
                let second_tree = second_tree;

                if self.is_cancelled() {
                    let msg = format!(
                        "{}手詰めが見つかりました（余詰めチェック中に中止、探索ノード数: {}）",
                        actual_depth, self.nodes_searched
                    );
                    self.log(0, format!("結果: {}", msg));
                    return SolutionData {
                        found: true,
                        tree: Some(tree),
                        second_tree: None,
                        message: msg,
                        trace: std::mem::take(&mut self.trace),
                    };
                }

                if let Some(second) = second_tree {
                    let second_depth = second.max_moves();
                    let msg = format!(
                        "余詰めあり: {}手詰めと{}手詰めが見つかりました (探索ノード数: {})",
                        actual_depth, second_depth, self.nodes_searched
                    );
                    self.log(0, format!("結果: {}", msg));
                    return SolutionData {
                        found: true,
                        tree: Some(tree),
                        second_tree: Some(second),
                        message: msg,
                        trace: std::mem::take(&mut self.trace),
                    };
                } else {
                    let msg = format!(
                        "完全作: {}手詰め、余詰めなし (探索ノード数: {})",
                        actual_depth, self.nodes_searched
                    );
                    self.log(0, format!("結果: {}", msg));
                    return SolutionData {
                        found: true,
                        tree: Some(tree),
                        second_tree: None,
                        message: msg,
                        trace: std::mem::take(&mut self.trace),
                    };
                }
            }
        }

        let msg = format!(
            "{}手詰めが見つかりました (探索ノード数: {})",
            actual_depth, self.nodes_searched
        );
        self.log(0, format!("結果: {}", msg));
        SolutionData {
            found: true,
            tree: Some(tree),
            second_tree: None,
            message: msg,
            trace: std::mem::take(&mut self.trace),
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// DFS型の攻め方ターン（ORノード）
    /// current_depth から effective_max まで一気に探索
    fn dfs_solve_attack(
        &mut self,
        meta: &MetaPosition,
        current_depth: u32,
        effective_max: u32,
        excluded_first_moves: &[Move],
    ) -> Option<SolutionNode> {
        self.nodes_searched += 1;

        if self.is_cancelled() {
            return None;
        }

        if meta.is_empty() {
            self.log(current_depth, "空のメタポジション → 失敗".to_string());
            return None;
        }

        // 既に全て詰んでいるか
        if meta.all_checkmate() {
            self.log(current_depth, format!(
                "全{}盤面で詰み！", meta.positions.len()
            ));
            return Some(SolutionNode::Checkmate {
                depth: current_depth,
            });
        }

        // remaining_depth を計算（既存の枝刈りロジックとの互換性のため）
        let remaining_depth = effective_max.saturating_sub(current_depth);

        if remaining_depth == 0 {
            self.log(current_depth, format!(
                "深さ上限到達 (メタポジション数={})", meta.positions.len()
            ));
            return None;
        }

        // 転置表チェック: 同じメタポジションがremaining_depth以上の深さで失敗済みならスキップ
        let meta_hash = meta_position_hash(meta);
        if let Some(&max_failed_depth) = self.fail_table.get(&meta_hash) {
            if remaining_depth <= max_failed_depth {
                self.log(current_depth, format!(
                    "転置表ヒット: 深さ{}で失敗済み（記録深さ={}）", remaining_depth, max_failed_depth
                ));
                return None;
            }
        }

        // 候補手を列挙（王手の手 + 情報収集用の手）
        let (mut candidate_moves, legal_move_sets) = self.generate_attack_candidates(meta);

        // ルートレベルで除外指定された初手をフィルタ
        if current_depth == 0 && !excluded_first_moves.is_empty() {
            candidate_moves.retain(|mv| !excluded_first_moves.contains(mv));
        }

        // 合駒可能マスが多い長距離王手を除外（合駒爆発を回避）
        // 残り深さが浅いほど厳しくフィルタ
        let max_interposition: u8 = if remaining_depth >= 7 { 4 } else { 2 };
        let king_sq_for_filter = meta.positions[0].find_king(Color::Gote);
        candidate_moves.retain(|mv| {
            let piece_kind = if let Some(drop_kind) = mv.drop_piece {
                drop_kind
            } else if let Some(from) = mv.from {
                meta.positions.iter()
                    .find_map(|pos| pos.piece_at(from).map(|p| {
                        if mv.promotion { p.kind.promoted().unwrap_or(p.kind) } else { p.kind }
                    }))
                    .unwrap_or(PieceKind::Pawn)
            } else {
                PieceKind::Pawn
            };
            let is_slider = matches!(piece_kind,
                PieceKind::Rook | PieceKind::PromotedRook |
                PieceKind::Bishop | PieceKind::PromotedBishop |
                PieceKind::Lance);
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

        // 候補手数の上限（ソート順で上位候補に絞る）
        let max_candidates: usize = if meta.positions.len() <= 5 {
            if remaining_depth >= 7 { 30 } else if remaining_depth >= 5 { 25 } else { 20 }
        } else if remaining_depth >= 7 {
            20
        } else if remaining_depth >= 5 {
            15
        } else if remaining_depth >= 3 {
            12
        } else {
            10
        };
        if candidate_moves.len() > max_candidates {
            candidate_moves.truncate(max_candidates);
        }

        self.log(current_depth, format!(
            "攻め方ターン: 残り深さ={}, メタポジション数={}, 候補手数={}",
            remaining_depth, meta.positions.len(), candidate_moves.len()
        ));
        for mv in &candidate_moves {
            self.log(current_depth, format!("  候補: {}", mv.to_japanese(Color::Sente)));
        }

        // 成/不成の重複候補をスキップするためのフィンガープリント
        let mut branch_fingerprints: HashMap<(Option<Square>, Square), u64> = HashMap::new();

        for mv in candidate_moves {
            if self.is_cancelled() {
                return None;
            }

            let node_before = self.nodes_searched;

            // 全盤面にこの手を適用し、合法/不正に分割
            let (legal_meta, illegal_meta) = meta.apply_attack_move_split_with_sets(mv, &legal_move_sets);

            if legal_meta.is_empty() {
                continue;
            }

            // 全合法盤面で王手がかかっているか確認
            if !legal_meta.positions.iter().all(|pos| pos.is_in_check(pos.side_to_move)) {
                self.log(current_depth, format!(
                    "  {} → 一部の盤面で王手にならない → 打ち切り",
                    mv.to_japanese(Color::Sente)
                ));
                continue;
            }

            // 成/不成の重複検出
            let precomputed_branches = if remaining_depth > 1 && mv.from.is_some() {
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
                        self.log(current_depth, format!(
                            "  {} → 成/不成による同一分岐 → スキップ",
                            mv.to_japanese(Color::Sente)
                        ));
                        continue;
                    }
                }
                branch_fingerprints.insert(key, fp);
                Some(branches)
            } else {
                None
            };

            self.log(current_depth, format!(
                "試行: {} (合法={}, 不正={})",
                mv.to_japanese(Color::Sente),
                legal_meta.positions.len(),
                illegal_meta.positions.len()
            ));

            let mut solution_branches = Vec::new();
            let mut all_solved = true;

            // 反則（不正）分岐の処理
            if !illegal_meta.is_empty() {
                self.log(current_depth, format!(
                    "  反則分岐: {}盤面で不正 → 同じ深さで再探索",
                    illegal_meta.positions.len()
                ));
                if let Some(continuation) =
                    self.dfs_solve_attack(&illegal_meta, current_depth, effective_max, &[])
                {
                    self.log(current_depth, "  反則分岐: 解決！".to_string());
                    solution_branches.push(SolutionBranch {
                        observation: Observation::Illegal,
                        continuation: Box::new(continuation),
                    });
                } else {
                    self.log(current_depth, "  反則分岐: 解決できず → 次の候補へ".to_string());
                    continue;
                }
            }

            // 最適化: remaining_depth==1 では expand_defense_moves を呼ばず、
            // 全盤面が実質的に詰みかどうかだけを高速チェックする
            if remaining_depth == 1 {
                if legal_meta.all_effectively_checkmate() {
                    self.log(current_depth, "  合法分岐: 全盤面で実質詰み！".to_string());
                    solution_branches.push(SolutionBranch {
                        observation: Observation::Checkmate,
                        continuation: Box::new(SolutionNode::Checkmate {
                            depth: current_depth + 1,
                        }),
                    });
                } else {
                    self.log(current_depth, "  合法分岐: 残り深さ1で詰みでない → 次の候補へ".to_string());
                    continue;
                }
            } else {
                // 合法分岐の処理: 玉方の応手を展開し、観測結果で分類
                let branches = precomputed_branches.unwrap_or_else(|| legal_meta.expand_defense_moves(mv));
                if self.is_cancelled() {
                    return None;
                }
                self.log(current_depth, format!(
                    "  合法分岐: 観測パターン数={}",
                    branches.len()
                ));

                // Phase 1: Checkmate分岐は即座に処理、Captured/NoCapture分岐を収集
                let mut non_checkmate_indices: Vec<usize> = Vec::new();
                for (i, (obs, _bm)) in branches.iter().enumerate() {
                    match obs {
                        Observation::Checkmate => {
                            self.log(current_depth, "  → 詰み分岐: 成功".to_string());
                            solution_branches.push(SolutionBranch {
                                observation: obs.clone(),
                                continuation: Box::new(SolutionNode::Checkmate {
                                    depth: current_depth + 1,
                                }),
                            });
                        }
                        Observation::Captured | Observation::NoCapture => {
                            non_checkmate_indices.push(i);
                        }
                        Observation::Illegal => {}
                    }
                }

                for (obs, bm) in &branches {
                    self.log(current_depth, format!(
                        "    {:?}: {}盤面", obs, bm.positions.len()
                    ));
                }

                // Phase 2: 各分岐を探索（自然順序で）
                for &idx in &non_checkmate_indices {
                    let (observation, branch_meta) = &branches[idx];
                    if branch_meta.positions.len() > MAX_META_POSITIONS {
                        self.log(current_depth, format!(
                            "  → {:?}分岐: {}盤面 > 上限{} → 枝刈り",
                            observation, branch_meta.positions.len(), MAX_META_POSITIONS
                        ));
                        all_solved = false;
                        break;
                    }
                    self.log(current_depth, format!(
                        "  → {:?}分岐: {}盤面を深さ{}で探索",
                        observation, branch_meta.positions.len(), remaining_depth - 2
                    ));
                    if let Some(continuation) =
                        self.dfs_solve_attack(branch_meta, current_depth + 2, effective_max, &[])
                    {
                        self.log(current_depth, format!(
                            "  → {:?}分岐: 解決！", observation
                        ));
                        solution_branches.push(SolutionBranch {
                            observation: observation.clone(),
                            continuation: Box::new(continuation),
                        });
                    } else {
                        self.log(current_depth, format!(
                            "  → {:?}分岐: 解決できず → 次の候補へ", observation
                        ));
                        all_solved = false;
                        break;
                    }
                }
            }

            if all_solved && !solution_branches.is_empty() {
                self.log(current_depth, format!(
                    "成功: {} で全分岐解決！", mv.to_japanese(Color::Sente)
                ));
                if current_depth == 0 {
                    self.last_root_move = Some(mv);
                }
                if current_depth <= 2 {
                    self.log_root(format!(
                        "[D{}] 成功: {} (nodes={})",
                        current_depth,
                        mv.to_japanese(Color::Sente),
                        self.nodes_searched - node_before
                    ));
                }
                return Some(SolutionNode::AttackMove {
                    mv: MoveData::from_move(mv, Color::Sente),
                    branches: solution_branches,
                });
            }

            if current_depth <= 2 {
                self.log_root(format!(
                    "[D{}] 失敗: {} (nodes={})",
                    current_depth,
                    mv.to_japanese(Color::Sente),
                    self.nodes_searched - node_before
                ));
            }
        }

        self.log(current_depth, "全候補手で失敗".to_string());
        // 転置表に失敗を記録（キャンセルによる中断は記録しない）
        if !self.is_cancelled() {
            let entry = self.fail_table.entry(meta_hash).or_insert(0);
            if remaining_depth > *entry {
                *entry = remaining_depth;
            }
        }
        None
    }

    /// 攻め方の候補手を生成
    /// 全盤面から王手の手を収集し、さらに情報収集用のプローブ手を追加
    /// 返り値: (候補手リスト, 各盤面の合法手セット) - 合法手セットは再利用可能
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
                meta.positions.iter()
                    .find_map(|pos| pos.piece_at(from).map(|p| {
                        if mv.promotion { p.kind.promoted().unwrap_or(p.kind) } else { p.kind }
                    }))
                    .unwrap_or(PieceKind::Pawn)
            } else {
                PieceKind::Pawn
            };
            let is_slider = matches!(piece_kind,
                PieceKind::Rook | PieceKind::PromotedRook |
                PieceKind::Bishop | PieceKind::PromotedBishop |
                PieceKind::Lance);
            let interposition = if is_slider && dist > 1 { dist - 1 } else { 0 };
            let piece_priority = match piece_kind {
                PieceKind::Rook | PieceKind::PromotedRook => 0,
                PieceKind::Bishop | PieceKind::PromotedBishop => 1,
                PieceKind::Gold | PieceKind::PromotedSilver | PieceKind::PromotedKnight
                | PieceKind::PromotedLance | PieceKind::PromotedPawn => 2,
                PieceKind::Silver => 3,
                PieceKind::Knight => 4,
                PieceKind::Lance => 5,
                PieceKind::Pawn => 6,
                _ => 7,
            };
            (is_drop, dist, interposition, piece_priority, mv.to.file, mv.to.rank)
        };
        check_moves.sort_by_key(|mv| sort_key(mv));

        // メタポジションが1つの場合、プローブは不要
        if n <= 1 {
            return (check_moves, legal_move_sets);
        }

        // プローブ手: 一部の盤面でのみ合法な手（メタポジションを分割する手）
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
