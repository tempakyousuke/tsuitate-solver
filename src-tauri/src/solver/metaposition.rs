use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};

use super::fasthash::{FxHashMap, FxHashSet, FxHasher};

use crate::shogi::position::Position;
use crate::shogi::types::*;

/// 展開処理の協調的キャンセル（CLI のタイムアウト/メモリ上限用）。
/// セットされている場合、expand_defense_moves は一定局面数ごとにフラグを確認し、
/// 立っていれば部分結果を捨てて空を返す。**空の戻り値を「応手なし＝詰み」と
/// 解釈しないよう、呼び出し側はキャンセル発火時に結果を使わず打ち切ること**
/// （tsuitate_dfpn.rs の各呼び出し箇所にガードあり。CLI 以外＝Tauri UI では
/// フラグ未設定のため挙動は変わらない）。
static EXPANSION_CANCEL: OnceLock<Arc<AtomicBool>> = OnceLock::new();

pub fn set_expansion_cancel(flag: Arc<AtomicBool>) {
    let _ = EXPANSION_CANCEL.set(flag);
}

pub fn expansion_cancelled() -> bool {
    EXPANSION_CANCEL
        .get()
        .map_or(false, |f| f.load(Ordering::Relaxed))
}

pub fn position_hash(pos: &Position) -> u64 {
    pos.zobrist_hash
}

/// 攻め方の持ち駒のハッシュ（攻め方は自分の持ち駒を観測できる）
fn sente_hand_hash(pos: &Position) -> u64 {
    let mut h = FxHasher::default();
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
    pub fn new(mut initial: Position) -> Self {
        initial.init_zobrist_hash();
        Self {
            positions: vec![initial],
        }
    }

    /// メタポジションが空かどうか
    pub fn is_empty(&self) -> bool {
        self.positions.is_empty()
    }

    /// 情報集合内の攻め方（先手）持ち駒枚数の最大値（駒余り判定用）。
    /// 盤面によって「最終手で駒を取ったかどうか」が異なる場合があるため最大値を採る
    pub fn max_sente_hand_count(&self) -> u8 {
        self.positions
            .iter()
            .map(|pos| pos.sente_hand.total())
            .max()
            .unwrap_or(0)
    }

    /// 情報集合内で保証される攻め方（先手）の持ち駒枚数（種類別の最小値）。
    /// 順序は PieceKind::HAND_PIECES と同じ [飛, 角, 金, 銀, 桂, 香, 歩]。
    /// 「最終手で取った駒の種類」が盤面によって異なる場合は共通部分だけが残る
    pub fn min_sente_hand_counts(&self) -> [u8; 7] {
        let mut iter = self.positions.iter();
        let Some(first) = iter.next() else {
            return [0; 7];
        };
        let mut result = first.hand(Color::Sente).counts_array();
        for pos in iter {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].min(h[j]);
            }
        }
        result
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
            let mut futile_drop_squares: FxHashSet<Square> = FxHashSet::default();
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
    /// S は Vec<Move> と Rc<Vec<Move>> の両方を受け付ける（Borrow 経由）
    pub fn apply_attack_move_split_with_sets<S: std::borrow::Borrow<Vec<Move>>>(
        &self,
        mv: Move,
        legal_move_sets: &[S],
    ) -> (MetaPosition, MetaPosition) {
        let mut legal_positions = Vec::new();
        let mut illegal_positions = Vec::new();

        let mut legal_seen: FxHashSet<u64> = FxHashSet::default();
        let mut illegal_seen: FxHashSet<u64> = FxHashSet::default();

        for (i, pos) in self.positions.iter().enumerate() {
            if legal_move_sets[i].borrow().contains(&mv) {
                let mut new_pos = pos.clone();
                new_pos.make_move(mv);
                if legal_seen.insert(new_pos.zobrist_hash) {
                    legal_positions.push(new_pos);
                }
            } else {
                if illegal_seen.insert(pos.zobrist_hash) {
                    illegal_positions.push(pos.clone());
                }
            }
        }

        (
            MetaPosition { positions: legal_positions },
            MetaPosition { positions: illegal_positions },
        )
    }

    /// 攻め方の手を指す（合法/不正に分割、特定の手の合法性のみをチェック）
    /// 証明木リプレイ用: generate_legal_moves の代わりに特定手の合法性のみ確認
    pub fn apply_attack_move_split_fast(&self, mv: Move) -> (MetaPosition, MetaPosition) {
        let mut legal_positions = Vec::new();
        let mut illegal_positions = Vec::new();
        let mut legal_seen: FxHashSet<u64> = FxHashSet::default();
        let mut illegal_seen: FxHashSet<u64> = FxHashSet::default();

        for pos in &self.positions {
            let color = pos.side_to_move;

            // 手の適用可能性を事前チェック（make_move のパニック防止）
            let can_apply = if let Some(drop_kind) = mv.drop_piece {
                pos.hand(color).count(drop_kind) > 0 && pos.piece_at(mv.to).is_none()
            } else if let Some(from) = mv.from {
                pos.piece_at(from).map_or(false, |p| p.color == color)
            } else {
                false
            };

            if !can_apply {
                if illegal_seen.insert(pos.zobrist_hash) {
                    illegal_positions.push(pos.clone());
                }
                continue;
            }

            // 合法性チェック: 手を指して自玉が王手されていないか
            let mut test_pos = pos.clone();
            let undo = test_pos.make_move(mv);
            let is_legal = !test_pos.is_in_check(color);

            if is_legal {
                if legal_seen.insert(test_pos.zobrist_hash) {
                    legal_positions.push(test_pos);
                }
            } else {
                test_pos.unmake_move(&undo);
                if illegal_seen.insert(pos.zobrist_hash) {
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
        let mut capture_groups: FxHashMap<(Square, u64), (FxHashSet<u64>, Vec<Position>)> = FxHashMap::default();
        // NoCapture グループ: 攻め方持ち駒ハッシュ → (重複排除用ハッシュ集合, 局面リスト)
        let mut no_capture_groups: FxHashMap<u64, (FxHashSet<u64>, Vec<Position>)> = FxHashMap::default();

        for (pos_idx, pos) in self.positions.iter().enumerate() {
            // 巨大な情報集合の展開が数GB規模になり得るため、協調キャンセルを確認
            // （呼び出し側はキャンセル発火時にこの戻り値を使わない契約）
            if pos_idx % 256 == 0 && expansion_cancelled() {
                return Vec::new();
            }
            // posは既に攻め方の手を指した後の状態（玉方手番）
            let defender_color = pos.side_to_move;
            let in_check = pos.is_in_check(defender_color);
            // 攻め方の持ち駒ハッシュ（玉方の応手では攻め方の持ち駒は変わらない）
            let hand_hash = sente_hand_hash(pos);

            if !in_check {
                // 王手でない場合（プローブ手など）: 玉方の全合法手を展開する。
                let legal_moves = pos.generate_legal_moves();
                if legal_moves.is_empty() {
                    checkmate_positions.push(pos.clone());
                    continue;
                }

                let attacker_color = defender_color.opponent();
                // 遅延クローン: scratch で make_move/unmake_move し、ユニーク時のみクローン
                let mut scratch = pos.clone();
                for def_mv in &legal_moves {
                    let captured = pos.piece_at(def_mv.to)
                        .map_or(false, |p| p.color == attacker_color);

                    let undo = scratch.make_move(*def_mv);

                    if captured {
                        let (seen, positions) = capture_groups
                            .entry((def_mv.to, hand_hash))
                            .or_insert_with(|| (FxHashSet::default(), Vec::new()));
                        if seen.insert(scratch.zobrist_hash) {
                            positions.push(scratch.clone());
                        }
                    } else {
                        let (seen, positions) = no_capture_groups
                            .entry(hand_hash)
                            .or_insert_with(|| (FxHashSet::default(), Vec::new()));
                        if seen.insert(scratch.zobrist_hash) {
                            positions.push(scratch.clone());
                        }
                    }

                    scratch.unmake_move(&undo);
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

            // 衝立詰将棋では、無駄合い判定を個別局面レベルで行わない。
            // 攻め方は個々の局面を区別できないため、一部の局面で無駄合いでも
            // 他の局面で有効な応手であれば、その応手は観測分岐に含まれるべき。
            // さらに、全局面で無駄合いの手であっても、攻め方がその筋を追わない
            // 手順では合駒した駒が盤上に残り、以降の観測結果に影響するため、
            // expand_defense_moves での除外は不正。
            // MetaPosition全体が実質詰みかどうかは all_effectively_checkmate() で
            // expand_defense_moves() の呼び出し前に判定済み。
            let attacker_color = defender_color.opponent();

            // 遅延クローン: scratch で make_move/unmake_move し、ユニーク時のみクローン
            let mut scratch = pos.clone();

            for def_mv in &legal_moves {
                let undo = scratch.make_move(*def_mv);

                let captured = pos.piece_at(def_mv.to)
                    .map_or(false, |p| p.color == attacker_color);

                if captured {
                    let (seen, positions) = capture_groups
                        .entry((def_mv.to, hand_hash))
                        .or_insert_with(|| (FxHashSet::default(), Vec::new()));
                    if seen.insert(scratch.zobrist_hash) {
                        positions.push(scratch.clone());
                    }
                } else {
                    let (seen, positions) = no_capture_groups
                        .entry(hand_hash)
                        .or_insert_with(|| (FxHashSet::default(), Vec::new()));
                    if seen.insert(scratch.zobrist_hash) {
                        positions.push(scratch.clone());
                    }
                }

                scratch.unmake_move(&undo);
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
