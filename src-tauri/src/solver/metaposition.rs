use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::shogi::position::Position;
use crate::shogi::types::*;

fn position_hash(pos: &Position) -> u64 {
    let mut h = DefaultHasher::new();
    pos.hash(&mut h);
    h.finish()
}

/// 攻め方の持ち駒のハッシュ（攻め方は自分の持ち駒を観測できる）
fn sente_hand_hash(pos: &Position) -> u64 {
    let mut h = DefaultHasher::new();
    pos.hand(Color::Sente).hash(&mut h);
    h.finish()
}

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
            let legal_moves = pos.generate_check_evasions();
            if legal_moves.is_empty() {
                continue; // 合法手なし = 詰み
            }
            // 合法手がある場合、全てが無駄合いかチェック（早期打ち切り）
            // 注意: 同じマスへの合駒でも、盤上の駒の移動と持ち駒の打ちでは
            // 結果が異なる場合がある（移動元マスが空くことで防御手が変わるため）。
            // そのため、打ち駒のみキャッシュし、盤上の駒の移動は個別にチェックする。
            let mut futile_drop_squares: HashSet<Square> = HashSet::new();
            for def_mv in &legal_moves {
                let is_king_move = if let Some(from) = def_mv.from {
                    pos.piece_at(from).map_or(false, |p| p.kind == PieceKind::King)
                } else {
                    false
                };
                if is_king_move {
                    return false; // 玉が逃げられる = 詰みではない
                }
                let is_drop = def_mv.drop_piece.is_some();
                if is_drop && futile_drop_squares.contains(&def_mv.to) {
                    continue; // 打ち駒の同一マスキャッシュヒット
                }
                if !pos.is_futile_interposition(def_mv) {
                    return false; // 有効な応手がある = 詰みではない
                }
                if is_drop {
                    futile_drop_squares.insert(def_mv.to);
                }
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

        let mut legal_seen: HashSet<u64> = HashSet::new();
        let mut illegal_seen: HashSet<u64> = HashSet::new();

        for (i, pos) in self.positions.iter().enumerate() {
            if legal_move_sets[i].contains(&mv) {
                let mut new_pos = pos.clone();
                new_pos.make_move(mv);
                if legal_seen.insert(position_hash(&new_pos)) {
                    legal_positions.push(new_pos);
                }
            } else {
                if illegal_seen.insert(position_hash(pos)) {
                    illegal_positions.push(pos.clone());
                }
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
    /// 攻め方は自分の持ち駒を観測できるため、攻め方の持ち駒が異なる盤面は
    /// 同じ観測結果でも区別される（別のメタポジションとして分離する）。
    /// これにより、攻め方の手で異なる駒を取った場合に正確な情報分割が行われる。
    ///
    /// 返り値: (observation, MetaPosition) のリスト
    ///   - 「駒取りあり」グループ: 攻め方の駒が取られた応手群
    ///   - 「駒取りなし」グループ: 攻め方の駒が取られなかった応手群
    ///   - 「詰み」: 玉方に合法手がない
    pub fn expand_defense_moves(&self, _attack_move: Move) -> Vec<(Observation, MetaPosition)> {
        let mut checkmate_positions = Vec::new();
        // グループキー: (地点, 攻め方持ち駒ハッシュ) → (重複排除用ハッシュ集合, 局面リスト)
        // 攻め方の持ち駒が異なる盤面は同じ観測でも区別する
        let mut capture_groups: HashMap<(Square, u64), (HashSet<u64>, Vec<Position>)> = HashMap::new();
        // NoCapture グループ: 攻め方持ち駒ハッシュ → (重複排除用ハッシュ集合, 局面リスト)
        let mut no_capture_groups: HashMap<u64, (HashSet<u64>, Vec<Position>)> = HashMap::new();

        for pos in &self.positions {
            // posは既に攻め方の手を指した後の状態（玉方手番）
            let defender_color = pos.side_to_move;
            let in_check = pos.is_in_check(defender_color);
            // 攻め方の持ち駒ハッシュ（玉方の応手では攻め方の持ち駒は変わらない）
            let hand_hash = sente_hand_hash(pos);

            if !in_check {
                // 王手でない場合（プローブ手など）: 玉方の全合法手を展開する。
                // 異なる応手は異なる盤面状態を生み出し、将来の手の合法性に影響するため、
                // 全応手を展開して正確なメタポジションを維持する必要がある。
                // （重複排除はハッシュで行い、同一局面は自動的に除外される）
                let legal_moves = pos.generate_legal_moves();
                if legal_moves.is_empty() {
                    // 合法手がない（ステイルメイト - 将棋では通常起きないが安全のため）
                    checkmate_positions.push(pos.clone());
                    continue;
                }

                let attacker_color = defender_color.opponent();
                for def_mv in &legal_moves {
                    let captured = pos.piece_at(def_mv.to)
                        .map_or(false, |p| p.color == attacker_color);
                    if captured {
                        let mut new_pos = pos.clone();
                        new_pos.make_move(*def_mv);
                        let (seen, positions) = capture_groups
                            .entry((def_mv.to, hand_hash))
                            .or_insert_with(|| (HashSet::new(), Vec::new()));
                        if seen.insert(position_hash(&new_pos)) {
                            positions.push(new_pos);
                        }
                    } else {
                        let mut new_pos = pos.clone();
                        new_pos.make_move(*def_mv);
                        let (seen, positions) = no_capture_groups
                            .entry(hand_hash)
                            .or_insert_with(|| (HashSet::new(), Vec::new()));
                        if seen.insert(position_hash(&new_pos)) {
                            positions.push(new_pos);
                        }
                    }
                }
                continue;
            }

            // 王手回避専用ジェネレータで高速に合法手を生成
            let legal_moves = pos.generate_check_evasions();

            if legal_moves.is_empty() {
                // 玉方に合法手がない = 詰み
                checkmate_positions.push(pos.clone());
                continue;
            }

            // 無駄合い判定のキャッシュ（打ち駒のみ、マス目ごと）
            // 盤上の駒の移動は移動元マスが空くことで防御手が変わる可能性があるため
            // キャッシュ対象外とする
            let mut futile_drop_squares: HashSet<Square> = HashSet::new();

            for def_mv in &legal_moves {
                // 無駄合い判定
                let is_king_move = if let Some(from) = def_mv.from {
                    pos.piece_at(from).map_or(false, |p| p.kind == PieceKind::King)
                } else {
                    false
                };

                let is_drop = def_mv.drop_piece.is_some();
                let is_futile = if is_king_move {
                    false
                } else if is_drop && futile_drop_squares.contains(&def_mv.to) {
                    true // 打ち駒の同一マスキャッシュヒット
                } else {
                    let result = pos.is_futile_interposition(def_mv);
                    if result && is_drop {
                        futile_drop_squares.insert(def_mv.to);
                    }
                    result
                };

                if is_futile {
                    continue;
                }

                let mut new_pos = pos.clone();
                new_pos.make_move(*def_mv);

                // 攻め方のどの駒でも取られたらCaptured観測（地点ごとに分類）
                let attacker_color = defender_color.opponent();
                let captured = pos.piece_at(def_mv.to)
                    .map_or(false, |p| p.color == attacker_color);

                if captured {
                    let (seen, positions) = capture_groups
                        .entry((def_mv.to, hand_hash))
                        .or_insert_with(|| (HashSet::new(), Vec::new()));
                    if seen.insert(position_hash(&new_pos)) {
                        positions.push(new_pos);
                    }
                } else {
                    let (seen, positions) = no_capture_groups
                        .entry(hand_hash)
                        .or_insert_with(|| (HashSet::new(), Vec::new()));
                    if seen.insert(position_hash(&new_pos)) {
                        positions.push(new_pos);
                    }
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

        // 地点 + 持ち駒ハッシュごとにCaptured分岐を追加
        let mut capture_keys: Vec<(Square, u64)> = capture_groups.keys().copied().collect();
        capture_keys.sort_by_key(|&(sq, hh)| (sq.file, sq.rank, hh));
        for key in capture_keys {
            if let Some((_, positions)) = capture_groups.remove(&key) {
                let sq = key.0;
                result.push((
                    Observation::Captured { file: sq.file, rank: sq.rank },
                    MetaPosition { positions },
                ));
            }
        }

        // 持ち駒ハッシュごとにNoCapture分岐を追加
        let mut nc_keys: Vec<u64> = no_capture_groups.keys().copied().collect();
        nc_keys.sort();
        for key in nc_keys {
            if let Some((_, positions)) = no_capture_groups.remove(&key) {
                result.push((
                    Observation::NoCapture,
                    MetaPosition { positions },
                ));
            }
        }

        result
    }

}

use super::solution::Observation;
