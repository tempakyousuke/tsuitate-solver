use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};

use crate::shogi::types::Move;

/// 攻め方が指した手に対する観測結果
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Observation {
    /// 駒が取られなかった
    NoCapture,
    /// 駒が取られた（どの地点の駒が取られたかは分かるが、取った駒の種類は分からない）
    Captured { file: u8, rank: u8 },
    /// 反則（打ち歩詰め等で手が無効だった場合）
    /// 衝立詰将棋では通常使わないが、不正局面の検出用
    Illegal,
    /// 詰み（手番側に合法手がない）
    Checkmate,
}

/// 解の手順木のノード
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SolutionNode {
    /// 詰み（この局面で詰んでいる）
    Checkmate {
        /// 詰みまでの手数
        depth: u32,
        /// 詰め上がり時に攻め方（先手）の持ち駒に残っている枚数
        /// （情報集合内の最大値。駒余り判定用。旧データは 0 扱い）
        #[serde(default)]
        hand_count: u8,
    },
    /// 攻め方の手
    AttackMove {
        /// 指し手
        mv: MoveData,
        /// 観測結果による分岐
        branches: Vec<SolutionBranch>,
        /// MetaPosition ハッシュ（余詰めチェック用、extract_solution で設定）
        #[serde(skip)]
        meta_hash: Option<u64>,
    },
}

/// 観測結果による分岐
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionBranch {
    /// 観測結果
    pub observation: Observation,
    /// この分岐に入った時点（攻め方の着手後）の攻め方の持ち駒枚数。
    /// 順序は [飛, 角, 金, 銀, 桂, 香, 歩]（PieceKind::HAND_PIECES と同順）。
    /// 情報集合内で枚数が一致しない場合（観測分岐のマージ後など）は保証枚数（最小値）。
    /// Illegal 分岐は着手が無効で持ち駒が変わらないため None。旧ソルバー出力にも無い。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sente_hand: Option<[u8; 7]>,
    /// この観測の後の継続
    pub continuation: Box<SolutionNode>,
}

/// フロントエンドに送る指し手データ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveData {
    pub from_file: Option<u8>,
    pub from_rank: Option<u8>,
    pub to_file: u8,
    pub to_rank: u8,
    pub promotion: bool,
    pub drop_piece: Option<String>,
    pub notation: String, // 日本語棋譜表記
}

impl MoveData {
    pub fn from_move(mv: Move, color: crate::shogi::types::Color) -> Self {
        Self {
            from_file: mv.from.map(|s| s.file),
            from_rank: mv.from.map(|s| s.rank),
            to_file: mv.to.file,
            to_rank: mv.to.rank,
            promotion: mv.promotion,
            drop_piece: mv.drop_piece.map(|k| k.to_kanji().to_string()),
            notation: mv.to_japanese(color),
        }
    }
}

impl SolutionNode {
    /// 最終手かどうかを判定する
    /// 全ての分岐の継続が直接 Checkmate である AttackMove は最終手
    pub fn is_final_move(&self) -> bool {
        match self {
            SolutionNode::Checkmate { .. } => false,
            SolutionNode::AttackMove { branches, .. } => {
                !branches.is_empty()
                    && branches
                        .iter()
                        .all(|b| matches!(*b.continuation, SolutionNode::Checkmate { .. }))
            }
        }
    }

    /// 手順木の構造ハッシュを計算する（Checkmate の depth は無視）
    fn structural_hash(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        self.hash_structure(&mut hasher);
        hasher.finish()
    }

    fn hash_structure(&self, hasher: &mut impl Hasher) {
        match self {
            SolutionNode::Checkmate { .. } => {
                0u8.hash(hasher);
            }
            SolutionNode::AttackMove { mv, branches, .. } => {
                1u8.hash(hasher);
                mv.from_file.hash(hasher);
                mv.from_rank.hash(hasher);
                mv.to_file.hash(hasher);
                mv.to_rank.hash(hasher);
                mv.promotion.hash(hasher);
                mv.drop_piece.hash(hasher);
                // ブランチを観測ハッシュでソートして順序非依存にする
                let mut branch_hashes: Vec<(u64, u64)> = branches
                    .iter()
                    .map(|b| {
                        let mut obs_hasher = DefaultHasher::new();
                        b.observation.hash(&mut obs_hasher);
                        let obs_hash = obs_hasher.finish();
                        let cont_hash = b.continuation.structural_hash();
                        (obs_hash, cont_hash)
                    })
                    .collect();
                branch_hashes.sort();
                branch_hashes.len().hash(hasher);
                for (obs, cont) in &branch_hashes {
                    obs.hash(hasher);
                    cont.hash(hasher);
                }
            }
        }
    }

    /// 手順木から全ての AttackMove サブツリーの構造ハッシュを収集する
    pub fn collect_attack_subtree_hashes(&self) -> HashSet<u64> {
        let mut hashes = HashSet::new();
        self.collect_hashes_recursive(&mut hashes);
        hashes
    }

    fn collect_hashes_recursive(&self, hashes: &mut HashSet<u64>) {
        match self {
            SolutionNode::Checkmate { .. } => {}
            SolutionNode::AttackMove { branches, .. } => {
                hashes.insert(self.structural_hash());
                for branch in branches {
                    branch.continuation.collect_hashes_recursive(hashes);
                }
            }
        }
    }

    /// 2つ目の解が1つ目の解に包含されているかチェックする
    ///
    /// 2つ目の解の全分岐が、最終的に1つ目の解のいずれかのサブツリーに収束する場合、
    /// 2つ目の解は無駄合い等による変形に過ぎず、実質的な余詰めではないと判断する。
    ///
    /// ルート直下の Checkmate 観測（初手で即詰み）は独立した詰み手段とみなし、
    /// 包含とは判定しない。内部ノードの Checkmate 観測は、1つ目の解でも同じ局面が
    /// 詰んでいるはずなので包含扱いとする。
    pub fn is_subsumed_by(&self, first_subtree_hashes: &HashSet<u64>) -> bool {
        match self {
            SolutionNode::Checkmate { .. } => false,
            SolutionNode::AttackMove { branches, .. } => {
                // 最終手余詰は余詰と見なさない
                // （ルートの手が最終手＝1手詰で別の詰め方がある場合も含む）
                if self.is_final_move() {
                    return true;
                }
                branches.iter().all(|b| match b.observation {
                    // ルート直下の Checkmate 観測は独立した詰み手段
                    Observation::Checkmate => false,
                    _ => Self::is_continuation_subsumed(
                        &b.continuation,
                        first_subtree_hashes,
                    ),
                })
            }
        }
    }

    /// 内部ノードが1つ目の解のサブツリーに包含されているかチェック
    fn is_continuation_subsumed(
        node: &SolutionNode,
        first_subtree_hashes: &HashSet<u64>,
    ) -> bool {
        match node {
            SolutionNode::Checkmate { .. } => false,
            SolutionNode::AttackMove { branches, .. } => {
                // このサブツリー全体が1つ目の解のサブツリーに一致するか
                if first_subtree_hashes.contains(&node.structural_hash()) {
                    return true;
                }
                // 非 Checkmate ブランチのみチェック
                // (Checkmate 観測は1つ目の解でも詰んでいるはずなので自動的に包含扱い)
                let non_checkmate: Vec<_> = branches
                    .iter()
                    .filter(|b| b.observation != Observation::Checkmate)
                    .collect();
                // 非 Checkmate ブランチがない場合、このノードは最終詰み手のみ
                // → サブツリー全体の一致が必要（上で不一致だったので false）
                if non_checkmate.is_empty() {
                    return false;
                }
                non_checkmate.iter().all(|b| {
                    Self::is_continuation_subsumed(
                        &b.continuation,
                        first_subtree_hashes,
                    )
                })
            }
        }
    }

    /// 解の最大手数を計算（攻め方と玉方の手数を含む）
    pub fn max_moves(&self) -> u32 {
        match self {
            SolutionNode::Checkmate { .. } => 0,
            SolutionNode::AttackMove { branches, .. } => {
                branches
                    .iter()
                    .map(|b| match b.observation {
                        // 攻め方の手が有効 → 即詰み: 1手
                        Observation::Checkmate => 1,
                        // 反則: 攻め方の手は実行されていない → サブ解法の手数のみ
                        Observation::Illegal => b.continuation.max_moves(),
                        // 攻め方の手(1手) + 玉方の応手(1手) + 続きの手数
                        Observation::Captured { .. } | Observation::NoCapture => {
                            2 + b.continuation.max_moves()
                        }
                    })
                    .max()
                    .unwrap_or(0)
            }
        }
    }

    /// 駒余り判定: 最長手順（最深の詰み）の全ての詰め上がりで攻め方の持ち駒が余るか。
    /// 変化（短い分岐）で余るのは通常の詰将棋の慣例に合わせて不問とする。
    /// 同じ最長手数でも観測分岐（反則分岐など）によって余らない詰め上がりが
    /// 1つでもあれば不問（余る側の駒は別分岐で使われる必要駒とみなす）。
    pub fn has_piece_surplus(&self) -> bool {
        let mut leaves: Vec<(u32, u8)> = Vec::new();
        self.collect_checkmate_leaves(&mut leaves);
        let Some(max_depth) = leaves.iter().map(|&(d, _)| d).max() else {
            return false;
        };
        leaves
            .iter()
            .filter(|&&(d, _)| d == max_depth)
            .all(|&(_, h)| h > 0)
    }

    /// 全 Checkmate 葉の (詰みまでの手数, 残り持ち駒枚数) を収集
    fn collect_checkmate_leaves(&self, out: &mut Vec<(u32, u8)>) {
        match self {
            SolutionNode::Checkmate { depth, hand_count } => out.push((*depth, *hand_count)),
            SolutionNode::AttackMove { branches, .. } => {
                for branch in branches {
                    branch.continuation.collect_checkmate_leaves(out);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ヘルパー: MoveData を手軽に作成
    fn make_move_data(notation: &str, to_file: u8, to_rank: u8, drop_piece: Option<&str>) -> MoveData {
        MoveData {
            from_file: if drop_piece.is_some() { None } else { Some(to_file) },
            from_rank: if drop_piece.is_some() { None } else { Some(to_rank + 1) },
            to_file,
            to_rank,
            promotion: notation.contains('成'),
            drop_piece: drop_piece.map(|s| s.to_string()),
            notation: notation.to_string(),
        }
    }

    /// ユーザ報告のケース:
    /// 解1: ▲２一飛打 → Captured(2,1) → ▲２二金打 → Checkmate
    /// 解2: ▲１一飛打 → NoCapture → ▲２一飛成 → Captured(2,1) → ▲２二金打 → Checkmate
    /// → 解2は無駄合い変形なので包含される
    #[test]
    fn test_subsumed_futile_interposition() {
        let solution1 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２一飛打", 2, 1, Some("飛")),
            branches: vec![
                SolutionBranch { sente_hand: None,
                    observation: Observation::Captured { file: 2, rank: 1 },
                    continuation: Box::new(SolutionNode::AttackMove {
                        meta_hash: None,
                        mv: make_move_data("▲２二金打", 2, 2, Some("金")),
                        branches: vec![SolutionBranch { sente_hand: None,
                            observation: Observation::Checkmate,
                            continuation: Box::new(SolutionNode::Checkmate { depth: 3, hand_count: 0 }),
                        }],
                    }),
                },
            ],
        };

        let solution2 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲１一飛打", 1, 1, Some("飛")),
            branches: vec![
                SolutionBranch { sente_hand: None,
                    observation: Observation::NoCapture,
                    continuation: Box::new(SolutionNode::AttackMove {
                        meta_hash: None,
                        mv: make_move_data("▲２一飛成", 2, 1, None),
                        branches: vec![
                            SolutionBranch { sente_hand: None,
                                observation: Observation::Captured { file: 2, rank: 1 },
                                continuation: Box::new(SolutionNode::AttackMove {
                                    meta_hash: None,
                                    mv: make_move_data("▲２二金打", 2, 2, Some("金")),
                                    branches: vec![SolutionBranch { sente_hand: None,
                                        observation: Observation::Checkmate,
                                        continuation: Box::new(SolutionNode::Checkmate { depth: 5, hand_count: 0 }),
                                    }],
                                }),
                            },
                        ],
                    }),
                },
            ],
        };

        let first_hashes = solution1.collect_attack_subtree_hashes();
        assert!(
            solution2.is_subsumed_by(&first_hashes),
            "無駄合い変形は包含と判定されるべき"
        );
    }

    /// 最終手余詰: 異なる初手で即詰み（1手詰め）
    /// 解1: ▲１一金打 → Checkmate
    /// 解2: ▲２二銀打 → Checkmate
    /// → 最終手での分岐なので余詰めと見なさない
    #[test]
    fn test_final_move_yodume_1move() {
        let solution1 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲１一金打", 1, 1, Some("金")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: 1, hand_count: 0 }),
            }],
        };

        let solution2 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２二銀打", 2, 2, Some("銀")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: 1, hand_count: 0 }),
            }],
        };

        let first_hashes = solution1.collect_attack_subtree_hashes();
        assert!(
            solution2.is_subsumed_by(&first_hashes),
            "最終手での分岐は余詰めと見なさない"
        );
    }

    /// is_final_move() の正確性テスト
    #[test]
    fn test_is_final_move() {
        // 全分岐の継続が直接 Checkmate である AttackMove は最終手
        let final_node = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２二金打", 2, 2, Some("金")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: 3, hand_count: 0 }),
            }],
        };
        assert!(final_node.is_final_move(), "全分岐がCheckmateに至るNodeは最終手");

        // 複数の Checkmate 分岐でも最終手
        let multi_checkmate = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２二金打", 2, 2, Some("金")),
            branches: vec![
                SolutionBranch { sente_hand: None,
                    observation: Observation::Checkmate,
                    continuation: Box::new(SolutionNode::Checkmate { depth: 3, hand_count: 0 }),
                },
                SolutionBranch { sente_hand: None,
                    observation: Observation::Illegal,
                    continuation: Box::new(SolutionNode::Checkmate { depth: 3, hand_count: 0 }),
                },
            ],
        };
        assert!(multi_checkmate.is_final_move(), "全分岐がCheckmateなら最終手");

        // 非 Checkmate 継続がある場合は最終手ではない
        let non_final = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２一飛打", 2, 1, Some("飛")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::NoCapture,
                continuation: Box::new(SolutionNode::AttackMove {
                    meta_hash: None,
                    mv: make_move_data("▲２二金打", 2, 2, Some("金")),
                    branches: vec![SolutionBranch { sente_hand: None,
                        observation: Observation::Checkmate,
                        continuation: Box::new(SolutionNode::Checkmate { depth: 3, hand_count: 0 }),
                    }],
                }),
            }],
        };
        assert!(!non_final.is_final_move(), "継続にAttackMoveがあれば最終手ではない");

        // Checkmate ノード自体は最終手ではない
        let checkmate = SolutionNode::Checkmate { depth: 1, hand_count: 0 };
        assert!(!checkmate.is_final_move(), "Checkmateノードは最終手ではない");
    }

    /// Illegal プローブ後に1つ目の解と同じ手順に合流するケース
    /// 解1: ▲１一金打 → Checkmate
    /// 解2: ▲３三桂打 → Illegal → ▲１一金打 → Checkmate
    /// → 包含される
    #[test]
    fn test_subsumed_illegal_probe() {
        let gold_drop = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲１一金打", 1, 1, Some("金")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: 1, hand_count: 0 }),
            }],
        };

        let solution2 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲３三桂打", 3, 3, Some("桂")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Illegal,
                continuation: Box::new(gold_drop.clone()),
            }],
        };

        let first_hashes = gold_drop.collect_attack_subtree_hashes();
        assert!(
            solution2.is_subsumed_by(&first_hashes),
            "Illegal プローブ後の合流は包含されるべき"
        );
    }

    /// 駒余り判定: 最長手数の詰め上がりが全て駒余りなら駒余り
    #[test]
    fn test_piece_surplus_all_max_depth_leaves() {
        let tree = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２二金打", 2, 2, Some("金")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: 1, hand_count: 1 }),
            }],
        };
        assert!(tree.has_piece_surplus(), "唯一の最長詰め上がりで余るなら駒余り");
    }

    /// ユーザ報告のケース（「同？確認」）:
    /// ▲１五歩 → Captured(1,5) → ▲１三角成 の分岐:
    ///   - Checkmate: 角成で詰み（香が余る、hand_count=1）
    ///   - Illegal: 角成が反則 → ▲１六香打 → Checkmate（余りなし、hand_count=0）
    /// 反則分岐は depth を進めないので両葉は同じ最長手数。
    /// 余らない詰め上がりがあるので駒余りではない。
    #[test]
    fn test_no_piece_surplus_when_illegal_branch_uses_piece() {
        let tree = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲１五歩", 1, 5, Some("歩")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Captured { file: 1, rank: 5 },
                continuation: Box::new(SolutionNode::AttackMove {
                    meta_hash: None,
                    mv: make_move_data("▲１三角成", 1, 3, None),
                    branches: vec![
                        SolutionBranch { sente_hand: None,
                            observation: Observation::Checkmate,
                            continuation: Box::new(SolutionNode::Checkmate {
                                depth: 3,
                                hand_count: 1,
                            }),
                        },
                        SolutionBranch { sente_hand: None,
                            observation: Observation::Illegal,
                            continuation: Box::new(SolutionNode::AttackMove {
                                meta_hash: None,
                                mv: make_move_data("▲１六香打", 1, 6, Some("香")),
                                branches: vec![SolutionBranch { sente_hand: None,
                                    observation: Observation::Checkmate,
                                    continuation: Box::new(SolutionNode::Checkmate {
                                        depth: 3,
                                        hand_count: 0,
                                    }),
                                }],
                            }),
                        },
                    ],
                }),
            }],
        };
        assert!(
            !tree.has_piece_surplus(),
            "同じ最長手数に余らない詰め上がりがあれば駒余りではない"
        );
    }

    /// 変化（短い分岐）で余らなくても最長手順が全て余るなら駒余り
    #[test]
    fn test_piece_surplus_ignores_shorter_leaves() {
        let tree = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２一飛打", 2, 1, Some("飛")),
            branches: vec![
                SolutionBranch { sente_hand: None,
                    observation: Observation::Checkmate,
                    continuation: Box::new(SolutionNode::Checkmate { depth: 1, hand_count: 0 }),
                },
                SolutionBranch { sente_hand: None,
                    observation: Observation::Captured { file: 2, rank: 1 },
                    continuation: Box::new(SolutionNode::AttackMove {
                        meta_hash: None,
                        mv: make_move_data("▲２二金打", 2, 2, Some("金")),
                        branches: vec![SolutionBranch { sente_hand: None,
                            observation: Observation::Checkmate,
                            continuation: Box::new(SolutionNode::Checkmate {
                                depth: 3,
                                hand_count: 1,
                            }),
                        }],
                    }),
                },
            ],
        };
        assert!(
            tree.has_piece_surplus(),
            "短い変化で余らなくても最長手順が全て余るなら駒余り"
        );
    }

    /// 解2の一部分岐が1つ目の解に収束しないケース → 余詰め
    #[test]
    fn test_not_subsumed_partial_convergence() {
        let solution1 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲２一飛打", 2, 1, Some("飛")),
            branches: vec![SolutionBranch { sente_hand: None,
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: 1, hand_count: 0 }),
            }],
        };

        // 解2: NoCapture は1つ目の解のサブツリーに収束するが、
        // Captured は異なる手順で詰む → 余詰め
        let solution2 = SolutionNode::AttackMove {
            meta_hash: None,
            mv: make_move_data("▲１一飛打", 1, 1, Some("飛")),
            branches: vec![
                SolutionBranch { sente_hand: None,
                    observation: Observation::NoCapture,
                    continuation: Box::new(solution1.clone()),
                },
                SolutionBranch { sente_hand: None,
                    observation: Observation::Captured { file: 1, rank: 1 },
                    continuation: Box::new(SolutionNode::AttackMove {
                        meta_hash: None,
                        mv: make_move_data("▲３三角打", 3, 3, Some("角")),
                        branches: vec![SolutionBranch { sente_hand: None,
                            observation: Observation::Checkmate,
                            continuation: Box::new(SolutionNode::Checkmate { depth: 3, hand_count: 0 }),
                        }],
                    }),
                },
            ],
        };

        let first_hashes = solution1.collect_attack_subtree_hashes();
        assert!(
            !solution2.is_subsumed_by(&first_hashes),
            "一部のみ収束する場合は余詰めとして報告すべき"
        );
    }
}

/// 解のデータ（フロントエンドへの送信用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionData {
    /// 解が見つかったか
    pub found: bool,
    /// 解の手順木
    pub tree: Option<SolutionNode>,
    /// 2つ目の解の手順木（余詰めチェック用）
    #[serde(default)]
    pub second_tree: Option<SolutionNode>,
    /// キズの手順木（プローブ代替手による許容余詰め）
    #[serde(default)]
    pub kizu_trees: Vec<SolutionNode>,
    /// メッセージ
    pub message: String,
    /// 探索ログ
    #[serde(default)]
    pub trace: Vec<String>,
}
