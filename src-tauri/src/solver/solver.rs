use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::types::*;

/// 衝立詰将棋ソルバー
pub struct TsuitateSolver {
    /// 最大探索深さ
    max_depth: u32,
    /// 探索ノード数
    pub nodes_searched: u64,
}

impl TsuitateSolver {
    pub fn new(max_depth: u32) -> Self {
        Self {
            max_depth,
            nodes_searched: 0,
        }
    }

    /// 衝立詰将棋を解く（反復深化）
    pub fn solve(&mut self, meta: &MetaPosition) -> SolutionData {
        for depth in (1..=self.max_depth).step_by(2) {
            // 詰将棋は奇数手で詰む
            self.nodes_searched = 0;

            if let Some(tree) = self.solve_attack(meta, depth, 0) {
                return SolutionData {
                    found: true,
                    tree: Some(tree),
                    message: format!(
                        "{}手詰めが見つかりました (探索ノード数: {})",
                        depth, self.nodes_searched
                    ),
                };
            }
        }

        SolutionData {
            found: false,
            tree: None,
            message: format!(
                "{}手以内の詰みは見つかりませんでした (探索ノード数: {})",
                self.max_depth, self.nodes_searched
            ),
        }
    }

    /// 攻め方のターン（ORノード）
    /// 少なくとも1つの手が全てのメタポジションで詰みに導く
    fn solve_attack(
        &mut self,
        meta: &MetaPosition,
        remaining_depth: u32,
        current_depth: u32,
    ) -> Option<SolutionNode> {
        self.nodes_searched += 1;

        if meta.is_empty() {
            return None;
        }

        // 既に全て詰んでいるか
        if meta.all_checkmate() {
            return Some(SolutionNode::Checkmate {
                depth: current_depth,
            });
        }

        if remaining_depth == 0 {
            return None;
        }

        // 全盤面で共通して打てる王手の手を列挙
        // 衝立詰将棋では、攻め方は同じ手を全盤面に対して指す
        let candidate_moves = self.generate_attack_candidates(meta);

        for mv in candidate_moves {
            // 全盤面にこの手を適用
            let after_attack = meta.apply_attack_move(mv);

            if after_attack.is_empty() {
                continue; // この手はどの盤面でも指せない
            }

            // 玉方の応手を展開し、観測結果で分類
            let branches = after_attack.expand_defense_moves(mv);

            // 全ての分岐で詰みを証明できるか試す
            let mut solution_branches = Vec::new();
            let mut all_solved = true;

            for (observation, branch_meta) in &branches {
                match observation {
                    Observation::Checkmate => {
                        // この分岐は既に詰み
                        solution_branches.push(SolutionBranch {
                            observation: observation.clone(),
                            continuation: Box::new(SolutionNode::Checkmate {
                                depth: current_depth + 1,
                            }),
                        });
                    }
                    Observation::Captured | Observation::NoCapture => {
                        // 攻め方の次の手を探索（深さを2減らす: 攻め方+玉方で1往復）
                        if remaining_depth < 2 {
                            all_solved = false;
                            break;
                        }
                        if let Some(continuation) =
                            self.solve_attack(branch_meta, remaining_depth - 2, current_depth + 2)
                        {
                            solution_branches.push(SolutionBranch {
                                observation: observation.clone(),
                                continuation: Box::new(continuation),
                            });
                        } else {
                            all_solved = false;
                            break;
                        }
                    }
                    Observation::Illegal => {
                        // 不正な手は無視
                    }
                }
            }

            if all_solved && !solution_branches.is_empty() {
                return Some(SolutionNode::AttackMove {
                    mv: MoveData::from_move(mv, Color::Sente),
                    branches: solution_branches,
                });
            }
        }

        None
    }

    /// 攻め方の候補手を生成
    /// 全盤面で王手になる手、または全盤面で合法な手を優先
    fn generate_attack_candidates(&self, meta: &MetaPosition) -> Vec<Move> {
        if meta.positions.is_empty() {
            return Vec::new();
        }

        // 最初の盤面から王手の手を取得
        let first_pos = &meta.positions[0];
        let check_moves = first_pos.generate_check_moves();

        // 全盤面で合法な王手を優先
        let mut candidates: Vec<Move> = check_moves
            .into_iter()
            .filter(|mv| {
                meta.positions.iter().all(|pos| {
                    let legal = pos.generate_legal_moves();
                    legal.contains(mv)
                })
            })
            .collect();

        // 全盤面で共通ではなくても、最初の盤面で王手になる手も候補に
        // （一部の盤面で指せない手は apply_attack_move で自動除外される）
        let first_checks = first_pos.generate_check_moves();
        for mv in first_checks {
            if !candidates.contains(&mv) {
                candidates.push(mv);
            }
        }

        candidates
    }
}
