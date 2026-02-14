use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::types::*;

/// MetaPosition のハッシュキー（転置表用）
/// Position のベクタをソートしてハッシュ化することで、順序に依存しないキーを生成
fn meta_position_hash(meta: &MetaPosition) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut hashes: Vec<u64> = meta.positions.iter().map(|pos| {
        let mut h = DefaultHasher::new();
        pos.hash(&mut h);
        h.finish()
    }).collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

/// メタポジションのサイズ上限（これを超える分岐は枝刈り）
const MAX_META_POSITIONS: usize = 5000;

/// 衝立詰将棋ソルバー
pub struct TsuitateSolver {
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
    /// 転置表: 失敗したメタポジション（ハッシュ, remaining_depth）→ 失敗済み
    /// 同じメタポジションを同じ深さ以下で再探索しても失敗することが保証される
    fail_table: HashSet<(u64, u32)>,
}

impl TsuitateSolver {
    pub fn new(max_depth: u32, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            max_depth,
            nodes_searched: 0,
            trace: Vec::new(),
            trace_enabled: true,
            cancelled,
            find_second_solution: false,
            last_root_move: None,
            fail_table: HashSet::new(),
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

    /// 衝立詰将棋を解く（指数的反復深化: 1, 3, 7, 15, ...）
    pub fn solve(&mut self, meta: &MetaPosition) -> SolutionData {
        self.log(0, format!("開始: メタポジション数={}", meta.positions.len()));

        // 初期盤面の駒を記録
        for (i, pos) in meta.positions.iter().enumerate() {
            self.log(0, format!("  盤面[{}]:", i));
            for file in 1..=9u8 {
                for rank in 1..=9u8 {
                    let sq = Square::new(file, rank);
                    if let Some(piece) = pos.piece_at(sq) {
                        let color_str = match piece.color {
                            Color::Sente => "先手",
                            Color::Gote => "後手",
                        };
                        self.log(0, format!(
                            "    {}{}:{}{}", file, rank, color_str, piece.kind.to_kanji()
                        ));
                    }
                }
            }
            // 持ち駒
            for kind in PieceKind::HAND_PIECES {
                let c = pos.hand(Color::Sente).count(kind);
                if c > 0 {
                    self.log(0, format!("    先手持ち駒: {}x{}", kind.to_kanji(), c));
                }
            }
            for kind in PieceKind::HAND_PIECES {
                let c = pos.hand(Color::Gote).count(kind);
                if c > 0 {
                    self.log(0, format!("    後手持ち駒: {}x{}", kind.to_kanji(), c));
                }
            }
        }

        let depths = self.build_depth_sequence();
        self.log(0, format!("探索深さ系列: {:?}", depths));

        // 第1フェーズ: 最初の解を探す
        let first_tree = self.solve_iterative(meta, &depths, &[]);

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
            let max_search_depth = *depths.last().unwrap_or(&1);
            let msg = format!(
                "{}手以内の詰みは見つかりませんでした (探索ノード数: {})",
                max_search_depth, self.nodes_searched
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
                self.log(0, format!(
                    "--- 2つ目の解を探索（初手 {} を除外）---",
                    first_mv.to_japanese(Color::Sente)
                ));
                let second_tree = self.solve_iterative(meta, &depths, &[first_mv]);

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

    /// 探索深さの系列を構築
    fn build_depth_sequence(&self) -> Vec<u32> {
        let max_search_depth = if self.max_depth % 2 == 0 {
            self.max_depth - 1
        } else {
            self.max_depth
        };
        let mut depths = Vec::new();
        let mut d = 1u32;
        while d < max_search_depth {
            depths.push(d);
            d = d * 2 + 1;
        }
        depths.push(max_search_depth);
        depths.dedup();
        depths
    }

    /// 反復深化で解を探す（excluded_first_movesに含まれる初手は除外）
    fn solve_iterative(
        &mut self,
        meta: &MetaPosition,
        depths: &[u32],
        excluded_first_moves: &[Move],
    ) -> Option<SolutionNode> {
        for &search_depth in depths {
            if self.is_cancelled() {
                return None;
            }
            // 各反復で転置表をクリア（より深い探索では浅い探索の失敗が無効になる場合がある）
            self.fail_table.clear();
            self.log(0, format!("--- 深さ{}手で探索開始 ---", search_depth));

            if let Some(tree) = self.solve_attack(meta, search_depth, 0, excluded_first_moves) {
                return Some(tree);
            }
        }
        None
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    /// 攻め方のターン（ORノード）
    /// 少なくとも1つの手が全てのメタポジションで詰みに導く
    fn solve_attack(
        &mut self,
        meta: &MetaPosition,
        remaining_depth: u32,
        current_depth: u32,
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

        if remaining_depth == 0 {
            self.log(current_depth, format!(
                "深さ上限到達 (メタポジション数={})", meta.positions.len()
            ));
            return None;
        }

        // 転置表チェック: 同じメタポジションを同じ深さ以下で失敗済みならスキップ
        let meta_hash = meta_position_hash(meta);
        if self.fail_table.contains(&(meta_hash, remaining_depth)) {
            self.log(current_depth, format!(
                "転置表ヒット: 深さ{}で失敗済み", remaining_depth
            ));
            return None;
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
        // TODO: 最適値を調整
        let max_candidates: usize = if remaining_depth >= 7 {
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

        for mv in candidate_moves {
            let node_before = self.nodes_searched;

            // 全盤面にこの手を適用し、合法/不正に分割（事前計算済みの合法手セットを再利用）
            let (legal_meta, illegal_meta) = meta.apply_attack_move_split_with_sets(mv, &legal_move_sets);

            if legal_meta.is_empty() {
                continue; // この手はどの盤面でも指せない
            }

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
                    self.solve_attack(&illegal_meta, remaining_depth, current_depth, &[])
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
                let branches = legal_meta.expand_defense_moves(mv);
                self.log(current_depth, format!(
                    "  合法分岐: 観測パターン数={}",
                    branches.len()
                ));
                for (obs, bm) in &branches {
                    self.log(current_depth, format!(
                        "    {:?}: {}盤面", obs, bm.positions.len()
                    ));
                }

                for (observation, branch_meta) in &branches {
                    match observation {
                        Observation::Checkmate => {
                            self.log(current_depth, "  → 詰み分岐: 成功".to_string());
                            solution_branches.push(SolutionBranch {
                                observation: observation.clone(),
                                continuation: Box::new(SolutionNode::Checkmate {
                                    depth: current_depth + 1,
                                }),
                            });
                        }
                        Observation::Captured | Observation::NoCapture => {
                            // メタポジションが大きすぎる場合は枝刈り
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
                                self.solve_attack(branch_meta, remaining_depth - 2, current_depth + 2, &[])
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
                        Observation::Illegal => {
                            // expand_defense_moves からは Illegal は来ない
                        }
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
            self.fail_table.insert((meta_hash, remaining_depth));
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
            .map(|pos| pos.generate_legal_moves().into_iter().collect())
            .collect();

        let mut seen = HashSet::new();
        let mut check_moves = Vec::new();

        // 全盤面から王手の手を収集（union）
        for (i, pos) in meta.positions.iter().enumerate() {
            let opponent = pos.side_to_move.opponent();
            for mv in &legal_move_sets[i] {
                if seen.contains(mv) {
                    continue;
                }
                let mut test_pos = pos.clone();
                test_pos.make_move(*mv);
                if test_pos.is_in_check(opponent) {
                    seen.insert(*mv);
                    check_moves.push(*mv);
                }
            }
        }

        // 候補手のソート（安定した手順で探索するため）
        // ヒューリスティック: 打ち駒優先、玉に近い手優先、接触王手優先、長距離利き駒優先、座標で決定的順序
        let king_sq = meta.positions[0].find_king(Color::Gote);
        let sort_key = |mv: &Move| -> (u8, u8, u8, u8, u8, u8) {
            // 第1キー: 打ち駒を優先（0=打ち, 1=移動）
            let is_drop = if mv.drop_piece.is_some() { 0u8 } else { 1u8 };
            // 第2キー: 玉との距離（小さいほど優先）
            let dist = if let Some(ksq) = king_sq {
                let df = (mv.to.file as i8 - ksq.file as i8).unsigned_abs();
                let dr = (mv.to.rank as i8 - ksq.rank as i8).unsigned_abs();
                df.max(dr)
            } else {
                0
            };
            // 第3キー: 合駒可能マス数（接触王手=0を優先、長距離ほど合駒爆発しやすい）
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
            // 第4キー: 駒種優先度（長距離利き駒を優先）
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
            // 第5,6キー: 座標で決定的順序を保証
            (is_drop, dist, interposition, piece_priority, mv.to.file, mv.to.rank)
        };
        check_moves.sort_by_key(|mv| sort_key(mv));

        // メタポジションが1つの場合、プローブは不要
        if n <= 1 {
            return (check_moves, legal_move_sets);
        }

        // プローブ手: 一部の盤面でのみ合法な手（メタポジションを分割する手）
        let mut probe_moves = Vec::new();
        let mut all_moves: HashSet<Move> = HashSet::new();
        for legal_set in &legal_move_sets {
            for mv in legal_set {
                all_moves.insert(*mv);
            }
        }

        for mv in &all_moves {
            if seen.contains(mv) {
                continue;
            }
            let legal_count = legal_move_sets.iter().filter(|s| s.contains(mv)).count();
            if legal_count > 0 && legal_count < n {
                seen.insert(*mv);
                probe_moves.push(*mv);
            }
        }

        check_moves.extend(probe_moves);
        (check_moves, legal_move_sets)
    }
}
