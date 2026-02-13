use std::collections::HashSet;

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

    /// 全ての盤面で実質的に詰んでいるか（詰みまたは全応手が無駄合い）
    /// expand_defense_moves より高速: 非詰み局面が見つかった時点で即座にfalseを返す
    pub fn all_effectively_checkmate(&self) -> bool {
        if self.positions.is_empty() {
            return false;
        }
        for pos in &self.positions {
            let defender_color = pos.side_to_move;
            if !pos.is_in_check(defender_color) {
                return false; // 王手でなければ詰みではない
            }
            let legal_moves = pos.generate_legal_moves();
            if legal_moves.is_empty() {
                continue; // 合法手なし = 詰み
            }
            // 合法手がある場合、全てが無駄合いかチェック（早期打ち切り）
            let mut futile_squares: HashSet<Square> = HashSet::new();
            for def_mv in &legal_moves {
                let is_king_move = if let Some(from) = def_mv.from {
                    pos.piece_at(from).map_or(false, |p| p.kind == PieceKind::King)
                } else {
                    false
                };
                if is_king_move {
                    return false; // 玉が逃げられる = 詰みではない
                }
                if futile_squares.contains(&def_mv.to) {
                    continue; // キャッシュヒット
                }
                if !pos.is_futile_interposition(def_mv) {
                    return false; // 有効な応手がある = 詰みではない
                }
                futile_squares.insert(def_mv.to);
            }
        }
        true
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

    /// 攻め方の手を指す（合法/不正に分割）
    /// 返り値: (合法な盤面(手を指した後), 不正な盤面(手を指す前の状態))
    /// 衝立詰将棋では、不正な手(反則)も情報を持つため分割して返す
    pub fn apply_attack_move_split(&self, mv: Move) -> (MetaPosition, MetaPosition) {
        let mut legal_positions = Vec::new();
        let mut illegal_positions = Vec::new();

        for pos in &self.positions {
            let legal_moves = pos.generate_legal_moves();
            if legal_moves.contains(&mv) {
                let mut new_pos = pos.clone();
                new_pos.make_move(mv);
                legal_positions.push(new_pos);
            } else {
                illegal_positions.push(pos.clone());
            }
        }

        (
            MetaPosition { positions: legal_positions },
            MetaPosition { positions: illegal_positions },
        )
    }

    /// 攻め方の手を指す（合法/不正に分割、事前計算された合法手セットを使用）
    /// generate_attack_candidates で既に計算された合法手セットを再利用し、
    /// 重複する generate_legal_moves 呼び出しを回避する
    pub fn apply_attack_move_split_with_sets(
        &self,
        mv: Move,
        legal_move_sets: &[HashSet<Move>],
    ) -> (MetaPosition, MetaPosition) {
        let mut legal_positions = Vec::new();
        let mut illegal_positions = Vec::new();

        for (i, pos) in self.positions.iter().enumerate() {
            if legal_move_sets[i].contains(&mv) {
                let mut new_pos = pos.clone();
                new_pos.make_move(mv);
                legal_positions.push(new_pos);
            } else {
                illegal_positions.push(pos.clone());
            }
        }

        (
            MetaPosition { positions: legal_positions },
            MetaPosition { positions: illegal_positions },
        )
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

            // 王手されている場合のみ無駄合い判定が必要
            let defender_color = pos.side_to_move;
            let in_check = pos.is_in_check(defender_color);

            // 無駄合い判定のキャッシュ（マス目ごと：同じマスへの合駒は同様に無駄）
            let mut futile_squares: HashSet<Square> = HashSet::new();

            for def_mv in &legal_moves {
                // 無駄合い判定（最適化済み）
                let is_futile = if !in_check {
                    false // 王手でなければ合駒ではない
                } else {
                    // 玉の移動は合駒ではない
                    let is_king_move = if let Some(from) = def_mv.from {
                        pos.piece_at(from).map_or(false, |p| p.kind == PieceKind::King)
                    } else {
                        false
                    };

                    if is_king_move {
                        false
                    } else if futile_squares.contains(&def_mv.to) {
                        true // キャッシュヒット: このマスへの合駒は無駄
                    } else {
                        let result = pos.is_futile_interposition(def_mv);
                        if result {
                            futile_squares.insert(def_mv.to);
                        }
                        result
                    }
                };

                if is_futile {
                    continue;
                }

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
