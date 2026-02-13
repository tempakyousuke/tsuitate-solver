use crate::shogi::position::Position;
use crate::shogi::types::*;

/// メタポジション: 観測情報と矛盾しない盤面状態の集合
/// 衝立詰将棋では、攻め方が相手の応手を直接観察できないため、
/// 可能性のある全局面を保持する
#[derive(Debug, Clone)]
pub struct MetaPosition {
    /// 矛盾しない盤面状態の集合
    pub positions: Vec<Position>,
}

impl MetaPosition {
    /// 初期状態（1つの盤面から開始）
    pub fn new(initial: Position) -> Self {
        Self {
            positions: vec![initial],
        }
    }

    /// メタポジションが空かどうか
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// 全ての盤面で詰んでいるか
    pub fn all_checkmate(&self) -> bool {
        !self.positions.is_empty() && self.positions.iter().all(|pos| pos.is_checkmate())
    }

    /// 攻め方の手を指す（全盤面に適用）
    /// 返り値: 手を指した後のメタポジション
    /// 不正な手（その盤面で指せない手）の盤面は除外される
    pub fn apply_attack_move(&self, mv: Move) -> MetaPosition {
        let mut new_positions = Vec::new();

        for pos in &self.positions {
            // この盤面で指し手が合法かチェック
            let legal_moves = pos.generate_legal_moves();
            if legal_moves.contains(&mv) {
                let mut new_pos = pos.clone();
                new_pos.make_move(mv);
                new_positions.push(new_pos);
            }
        }

        MetaPosition {
            positions: new_positions,
        }
    }

    /// 玉方の全応手を展開し、観測結果で分類する
    /// 攻め方が指した手のmvの結果について、各盤面で玉方の応手を列挙し、
    /// 駒が取られたか/取られなかったかで分類する
    ///
    /// 返り値: (observation, MetaPosition) のリスト
    ///   - 「駒取りあり」グループ: 攻め方の駒が取られた応手群
    ///   - 「駒取りなし」グループ: 攻め方の駒が取られなかった応手群
    ///   - 「詰み」: 玉方に合法手がない
    pub fn expand_defense_moves(&self, attack_move: Move) -> Vec<(Observation, MetaPosition)> {
        let mut checkmate_positions = Vec::new();
        let mut capture_positions = Vec::new();
        let mut no_capture_positions = Vec::new();

        for pos in &self.positions {
            // posは既に攻め方の手を指した後の状態（玉方手番）
            let legal_moves = pos.generate_legal_moves();

            if legal_moves.is_empty() {
                // 玉方に合法手がない = 詰み
                checkmate_positions.push(pos.clone());
                continue;
            }

            for def_mv in &legal_moves {
                let mut new_pos = pos.clone();
                new_pos.make_move(*def_mv);

                // 攻め方が指した手の駒が取られたかどうか
                // = 玉方がattack_move.toに移動してきたか
                let captured = def_mv.to == attack_move.to;

                if captured {
                    capture_positions.push(new_pos);
                } else {
                    no_capture_positions.push(new_pos);
                }
            }
        }

        let mut result = Vec::new();

        if !checkmate_positions.is_empty() {
            result.push((
                Observation::Checkmate,
                MetaPosition {
                    positions: checkmate_positions,
                },
            ));
        }

        if !capture_positions.is_empty() {
            result.push((
                Observation::Captured,
                MetaPosition {
                    positions: capture_positions,
                },
            ));
        }

        if !no_capture_positions.is_empty() {
            result.push((
                Observation::NoCapture,
                MetaPosition {
                    positions: no_capture_positions,
                },
            ));
        }

        result
    }
}

use super::solution::Observation;
