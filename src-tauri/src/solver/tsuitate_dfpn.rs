use std::collections::{HashMap, HashSet};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::solution::*;
use crate::shogi::position::Position;
use crate::shogi::types::*;

/// 証明数/反証数の上限（sum時のオーバーフロー防止）
const INF: u32 = u32::MAX / 2;

/// メタポジションのサイズ上限
const MAX_META_POSITIONS: usize = 5000;

/// 衝立df-pnの探索結果
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TsuitateDfpnResult {
    /// 詰みが証明された（攻め方勝ち）
    Proven,
    /// 不詰が証明された（玉方勝ち）
    Disproven,
    /// 探索が打ち切られた（ノード上限 or キャンセル）
    Unknown,
}

/// 転置表エントリ
#[derive(Debug, Clone, Copy)]
struct PnDn {
    pn: u32,
    dn: u32,
}

/// コンパクトな証明木ノード（リプレイキャッシュ用）
/// SolutionNode と異なり String を持たず、Move を直接格納
#[derive(Debug, Clone)]
enum ProofNode {
    Checkmate,
    Attack {
        mv: Move,
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

impl ProofObs {
    fn matches(&self, obs: &Observation) -> bool {
        match (self, obs) {
            (ProofObs::Checkmate, Observation::Checkmate) => true,
            (ProofObs::Captured(f, r), Observation::Captured { file, rank }) => f == file && r == rank,
            (ProofObs::NoCapture, Observation::NoCapture) => true,
            (ProofObs::Illegal, Observation::Illegal) => true,
            _ => false,
        }
    }
}

fn meta_position_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|p| p.zobrist_hash)
        .collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

/// MetaPosition の盤面セットハッシュ（sente_hand を除外）
/// 優越関係の判定用: 同じ盤面配置で sente_hand のみ異なるメタポジションを同一グループにまとめる
/// ソートしてから結合することで XOR より衝突に強いハッシュを生成
fn meta_board_set_hash(meta: &MetaPosition) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|pos| pos.hash_without_sente_hand())
        .collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

/// MetaPosition の盤面のみのハッシュ（両方の持ち駒を除外）
/// 手順ヒント用: 盤面配置が同じメタポジション間で証明手順を共有する
fn meta_board_only_hash(meta: &MetaPosition) -> u64 {
    let mut hashes: Vec<u64> = meta.positions.iter()
        .map(|pos| pos.hash_board_only())
        .collect();
    hashes.sort();
    let mut h = DefaultHasher::new();
    hashes.hash(&mut h);
    h.finish()
}

// ========================================================================
// 王手候補の幾何学的事前フィルタ
// ========================================================================

/// 駒種が to から target を大まかに攻撃可能か（遮蔽物無視）
/// 偽陰性なし（王手になりうる手を漏らさない）、偽陽性あり
fn can_attack_rough(kind: PieceKind, to: Square, target: Square, color: Color) -> bool {
    let df = target.file as i8 - to.file as i8;
    let dr = target.rank as i8 - to.rank as i8;
    if df == 0 && dr == 0 {
        return false;
    }
    match kind {
        PieceKind::King => df.abs() <= 1 && dr.abs() <= 1,
        PieceKind::Gold | PieceKind::PromotedSilver | PieceKind::PromotedKnight
        | PieceKind::PromotedLance | PieceKind::PromotedPawn => {
            // 金の動き（色依存）
            if df.abs() <= 1 && dr.abs() <= 1 {
                if color == Color::Sente {
                    // 後ろ斜めは不可: (-1,1),(1,1) が不可
                    !(dr == 1 && df.abs() == 1)
                } else {
                    // 前斜めは不可: (-1,-1),(1,-1) が不可
                    !(dr == -1 && df.abs() == 1)
                }
            } else {
                false
            }
        }
        PieceKind::Silver => {
            if df.abs() <= 1 && dr.abs() <= 1 {
                if color == Color::Sente {
                    // 銀: 前3 + 後斜め2 = (-1,-1),(0,-1),(1,-1),(-1,1),(1,1)
                    !(df == 0 && dr == 1) && !(df.abs() == 1 && dr == 0)
                } else {
                    !(df == 0 && dr == -1) && !(df.abs() == 1 && dr == 0)
                }
            } else {
                false
            }
        }
        PieceKind::Knight => {
            if color == Color::Sente {
                (df == -1 || df == 1) && dr == -2
            } else {
                (df == -1 || df == 1) && dr == 2
            }
        }
        PieceKind::Pawn => {
            if color == Color::Sente {
                df == 0 && dr == -1
            } else {
                df == 0 && dr == 1
            }
        }
        PieceKind::Lance => {
            // 同筋で前方向のみ（遮蔽物無視）
            if df != 0 {
                return false;
            }
            if color == Color::Sente { dr < 0 } else { dr > 0 }
        }
        PieceKind::Rook => {
            // 十字方向（遮蔽物無視）
            df == 0 || dr == 0
        }
        PieceKind::Bishop => {
            // 斜め方向（遮蔽物無視）
            df.abs() == dr.abs()
        }
        PieceKind::PromotedRook => {
            // 竜: 十字スライド + 斜め1マス
            df == 0 || dr == 0 || (df.abs() <= 1 && dr.abs() <= 1)
        }
        PieceKind::PromotedBishop => {
            // 馬: 斜めスライド + 十字1マス
            df.abs() == dr.abs() || (df.abs() <= 1 && dr.abs() <= 1)
        }
    }
}

/// ある方向にスライドできる攻め方の駒種か
fn is_slider_for_direction(kind: PieceKind, df: i8, dr: i8, color: Color) -> bool {
    match kind {
        PieceKind::Rook | PieceKind::PromotedRook => df == 0 || dr == 0,
        PieceKind::Bishop | PieceKind::PromotedBishop => df.abs() == dr.abs() && df != 0,
        PieceKind::Lance => {
            df == 0 && if color == Color::Sente { dr < 0 } else { dr > 0 }
        }
        _ => false,
    }
}

/// 開き王手の候補マスを計算
/// king_sq から8方向をスキャンし、攻め方の駒の後ろにスライダーがある場合、
/// 手前の駒のマスを discovery_square として返す
fn compute_discovery_squares(pos: &Position, king_sq: Square, attacker: Color) -> Vec<Square> {
    let directions: [(i8, i8); 8] = [
        (0, -1), (0, 1), (-1, 0), (1, 0),
        (-1, -1), (-1, 1), (1, -1), (1, 1),
    ];
    let mut result = Vec::new();

    for &(df, dr) in &directions {
        let mut f = king_sq.file as i8 + df;
        let mut r = king_sq.rank as i8 + dr;
        let mut first_piece_sq: Option<Square> = None;

        while Square::is_valid(f, r) {
            let sq = Square::new(f as u8, r as u8);
            if let Some(piece) = pos.piece_at(sq) {
                if piece.color == attacker {
                    if first_piece_sq.is_none() {
                        // 最初に見つけた攻め方の駒 → 開き王手の候補
                        first_piece_sq = Some(sq);
                    } else {
                        // 2番目の攻め方の駒 → スライダーならdiscovery確定
                        if is_slider_for_direction(piece.kind, -df, -dr, attacker) {
                            result.push(first_piece_sq.unwrap());
                        }
                        break;
                    }
                } else {
                    // 相手の駒に当たった → このラインは終了
                    break;
                }
            }
            f += df;
            r += dr;
        }
    }

    result
}

/// 手 mv が king_sq に王手を与えうるかの粗判定（偽陰性なし）
fn could_give_check(mv: &Move, king_sq: Square, discovery_sqs: &[Square], attacker: Color) -> bool {
    // A. 直接王手の可能性
    let piece_kind_after = if let Some(drop_kind) = mv.drop_piece {
        drop_kind
    } else if let Some(moved_kind) = mv.moved_piece_kind {
        if mv.promotion {
            moved_kind.promoted().unwrap_or(moved_kind)
        } else {
            moved_kind
        }
    } else {
        return true; // 安全側に倒す
    };

    if can_attack_rough(piece_kind_after, mv.to, king_sq, attacker) {
        return true;
    }

    // B. 開き王手の可能性（盤上の駒移動のみ）
    if let Some(from) = mv.from {
        if discovery_sqs.contains(&from) {
            // 移動先が同じラインに留まるかチェック
            let df_king = king_sq.file as i8 - from.file as i8;
            let dr_king = king_sq.rank as i8 - from.rank as i8;
            let df_to = mv.to.file as i8 - from.file as i8;
            let dr_to = mv.to.rank as i8 - from.rank as i8;

            // 同じライン上に留まるかの判定
            // ライン方向を正規化して比較
            let stays_on_line = if df_king == 0 {
                df_to == 0
            } else if dr_king == 0 {
                dr_to == 0
            } else {
                // 斜め方向: df_to/dr_to が df_king/dr_king と同じ方向比
                df_to != 0 && dr_to != 0
                    && df_king.signum() * dr_to == dr_king.signum() * df_to
            };

            if !stays_on_line {
                return true;
            }
        }
    }

    false
}

/// OR ノードの候補手生成結果キャッシュ
/// mid_or が同一 OR ノードを再訪問する際の generate_attack_candidates 再計算を回避
/// 注: legal_move_sets はキャッシュしない。meta_position_hash は局面順序に依存しないが、
/// legal_move_sets は positions の順序に依存するため、キャッシュすると順序不一致で
/// apply_attack_move_split_with_sets が誤った legal/illegal 分割を行うバグの原因になる。
struct CachedOrCandidates {
    candidates: Vec<Move>,
}

/// AND ノードの展開結果キャッシュ
/// mid_and が同一 AND ノードを再訪問する際の expand_defense_moves 再計算を回避
#[derive(Clone)]
struct CachedAndExpansion {
    obs_terminal: Vec<bool>,
    obs_hash: Vec<u64>,
    obs_metas: Vec<MetaPosition>,
    obs_depth_inc: Vec<u32>,
    obs_hands: Vec<[u8; 7]>,
    parent_hand: [u8; 7],
}

/// 衝立詰将棋 df-pn ソルバー
///
/// 通常の df-pn が Position で動くのに対し、MetaPosition で動く。
/// - OR ノード: MetaPosition（攻め方が手を選ぶ）
/// - AND ノード: 観測分岐（Checkmate / Captured / NoCapture / Illegal、最大4分岐）
///
/// AND ノードの分岐数が最大4であるため、通常の詰将棋の AND ノード
/// （王手回避手＝数十手）に比べて大幅に効率的。
pub struct TsuitateDfpnSolver {
    /// OR ノードの転置表: MetaPosition ハッシュ → (pn, dn)
    or_table: HashMap<u64, PnDn>,
    /// AND ノードの転置表: hash(meta_hash, move) → (pn, dn)
    and_table: HashMap<u64, PnDn>,
    /// 探索ノード数
    pub nodes_searched: u64,
    /// 優越関係ヒット数（診断用）
    pub dominance_hits: u64,
    /// ノード数上限
    node_limit: u64,
    /// キャンセルフラグ
    cancelled: Arc<AtomicBool>,
    /// ルートで除外する手（余詰めチェック用）
    excluded_root_moves: Vec<Move>,
    /// ルートのメタポジションハッシュ（除外判定用）
    root_hash: Option<u64>,
    /// 深さ上限（None = 制限なし）
    depth_limit: Option<u32>,
    /// 優越関係テーブル: board_set_hash → Vec<[u8; 7]> (Pareto最小の proof_hand)
    /// proof_hand <= sente_hand (要素ごと) で判定: 証明に必要な持ち駒が揃っていれば証明済み
    dominance_table: HashMap<u64, Vec<[u8; 7]>>,
    /// OR ノードの証明駒: meta_hash → proof_hand (証明に必要な最小の攻め方持ち駒)
    or_proof_hands: HashMap<u64, [u8; 7]>,
    /// AND ノードの証明駒: and_key → proof_hand
    and_proof_hands: HashMap<u64, [u8; 7]>,
    /// 盤面のみのハッシュ → 証明された手（手順ヒント用）
    move_hints: HashMap<u64, Move>,
    /// 盤面のみのハッシュ → 証明木リスト（リプレイキャッシュ、複数手順対応）
    proof_cache: HashMap<u64, Vec<ProofNode>>,
    /// AND ノードの展開結果キャッシュ: and_key → 構造データ
    /// mid_and が同一 AND ノードを再訪問する際の expand_defense_moves 再計算を回避
    and_expansion_cache: HashMap<u64, CachedAndExpansion>,
    /// OR ノードの候補手キャッシュ: meta_hash → 候補手 + 合法手セット
    /// mid_or が同一 OR ノードを再訪問する際の generate_attack_candidates 再計算を回避
    or_candidate_cache: HashMap<u64, CachedOrCandidates>,
    /// 局面レベルの合法手キャッシュ: zobrist_hash → 合法手リスト
    /// 同一局面が異なるMetaPositionに出現する場合の重複計算を回避
    legal_moves_cache: HashMap<u64, Vec<Move>>,
    /// 局面レベルの王手手キャッシュ: zobrist_hash → 王手になる手のリスト
    /// 同一局面が異なるMetaPositionに出現する場合の重複計算を回避
    check_moves_cache: HashMap<u64, Vec<Move>>,
    /// 診断用カウンタ
    pub proof_replay_attempts: u64,
    pub proof_replay_full_success: u64,
    pub proof_replay_partial: u64,
    pub proof_replay_fail: u64,
    pub expand_defense_calls: u64,
    pub mid_and_calls: u64,
    /// 診断: 探索中に遭遇した最大 MetaPosition サイズ
    pub max_meta_size: usize,
    /// 診断: generate_attack_candidates の累計時間 (ナノ秒)
    pub gen_candidates_nanos: u64,
    /// 診断: expand_defense_moves の累計時間 (ナノ秒)
    pub expand_defense_nanos: u64,
    /// 診断: all_effectively_checkmate の累計時間 (ナノ秒)
    pub aec_nanos: u64,
    /// 診断: generate_attack_candidates の非キャッシュ呼出回数
    pub gen_candidates_calls: u64,
    /// 診断: generate_attack_candidates で処理した局面の合計数
    pub gen_candidates_total_positions: u64,
    /// 診断: legal_moves_cache ヒット数
    pub legal_moves_cache_hits: u64,
    /// 診断: legal_moves_cache ミス数
    pub legal_moves_cache_misses: u64,
}

impl TsuitateDfpnSolver {
    pub fn new(node_limit: u64, cancelled: Arc<AtomicBool>) -> Self {
        Self {
            or_table: HashMap::new(),
            and_table: HashMap::new(),
            nodes_searched: 0,
            dominance_hits: 0,
            node_limit,
            cancelled,
            excluded_root_moves: Vec::new(),
            root_hash: None,
            depth_limit: None,
            dominance_table: HashMap::new(),
            or_proof_hands: HashMap::new(),
            and_proof_hands: HashMap::new(),
            move_hints: HashMap::new(),
            proof_cache: HashMap::new(),
            and_expansion_cache: HashMap::new(),
            or_candidate_cache: HashMap::new(),
            legal_moves_cache: HashMap::new(),
            check_moves_cache: HashMap::new(),
            proof_replay_attempts: 0,
            proof_replay_full_success: 0,
            proof_replay_partial: 0,
            proof_replay_fail: 0,
            expand_defense_calls: 0,
            mid_and_calls: 0,
            max_meta_size: 0,
            gen_candidates_nanos: 0,
            expand_defense_nanos: 0,
            aec_nanos: 0,
            gen_candidates_calls: 0,
            gen_candidates_total_positions: 0,
            legal_moves_cache_hits: 0,
            legal_moves_cache_misses: 0,
        }
    }

    /// メタポジションが詰むかどうかを判定する
    pub fn solve(&mut self, meta: &MetaPosition) -> TsuitateDfpnResult {
        let hash = meta_position_hash(meta);
        self.root_hash = Some(hash);
        let mut path = vec![hash];

        self.mid_or(meta, INF, INF, &mut path, 0);

        let entry = self.or_table.get(&hash).copied().unwrap_or(PnDn { pn: 1, dn: 1 });
        if entry.pn == 0 {
            TsuitateDfpnResult::Proven
        } else if entry.dn == 0 {
            TsuitateDfpnResult::Disproven
        } else {
            TsuitateDfpnResult::Unknown
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }

    fn should_stop(&self) -> bool {
        self.nodes_searched >= self.node_limit || self.is_cancelled()
    }

    fn and_key(meta_hash: u64, mv: &Move) -> u64 {
        let mut h = DefaultHasher::new();
        h.write_u64(meta_hash);
        mv.hash(&mut h);
        h.finish()
    }

    /// OR ノード（攻め方）: 候補手から1つでも詰みに導ければ成功
    ///
    /// pn = min{pn_c}, dn = sum{dn_c}
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

        // MetaPosition サイズ記録
        if meta.positions.len() > self.max_meta_size {
            self.max_meta_size = meta.positions.len();
        }

        // 終端チェック
        if meta.is_empty() {
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            return;
        }
        let aec_start = std::time::Instant::now();
        let is_aec = meta.all_effectively_checkmate();
        self.aec_nanos += aec_start.elapsed().as_nanos() as u64;
        if is_aec {
            self.or_table.insert(hash, PnDn { pn: 0, dn: INF });
            let proof_hand = [0u8; 7];
            self.or_proof_hands.insert(hash, proof_hand);
            if !meta.positions.is_empty() {
                let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
                let board_hash = meta_board_set_hash(meta);
                self.add_dominance_entry(board_hash, sente_hand);
            }
            return;
        }
        if meta.positions.len() > MAX_META_POSITIONS {
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            return;
        }

        // 深さ制限チェック
        if let Some(limit) = self.depth_limit {
            if depth >= limit {
                self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
                return;
            }
        }

        // 優越関係チェック: proof_hand <= sente_hand (要素ごと)
        // 同じ盤面セット + 同じ gote_hand で、sente_hand が proof_hand 以上なら証明済み
        if !meta.positions.is_empty() {
            let board_hash = meta_board_set_hash(meta);
            let sente_hand = meta.positions[0].hand(Color::Sente).counts_array();
            if let Some(entries) = self.dominance_table.get(&board_hash) {
                if let Some(matched_ph) = entries
                    .iter()
                    .find(|ph| ph.iter().zip(sente_hand.iter()).all(|(p, s)| p <= s))
                {
                    self.or_table.insert(hash, PnDn { pn: 0, dn: INF });
                    self.or_proof_hands.insert(hash, *matched_ph);
                    self.dominance_hits += 1;
                    return;
                }
            }
        }

        // 証明木リプレイ: board-only hash でキャッシュされた証明木を試す（複数候補対応）
        if !meta.positions.is_empty() {
            let board_only_hash = meta_board_only_hash(meta);
            if let Some(proofs) = self.proof_cache.get(&board_only_hash).cloned() {
                self.proof_replay_attempts += 1;
                let or_table_size_before = self.or_table.len();
                let mut replayed = false;
                for proof in &proofs {
                    if let Some(_or_ph) = self.try_replay_proof(meta, proof, hash, path, depth) {
                        self.proof_replay_full_success += 1;
                        replayed = true;
                        break;
                    }
                }
                if !replayed {
                    if self.or_table.len() > or_table_size_before {
                        self.proof_replay_partial += 1;
                    } else {
                        self.proof_replay_fail += 1;
                    }
                } else {
                    return;
                }
            }
        }

        // 候補手を生成（キャッシュ活用、remove/re-insert パターン）
        // legal_move_sets は毎回 positions の順序に合わせて再生成する
        // （meta_position_hash は順序非依存だが legal_move_sets は順序依存のため）
        let candidates_raw = if let Some(cached) = self.or_candidate_cache.remove(&hash) {
            cached.candidates
        } else {
            let gen_start = std::time::Instant::now();
            let (candidates, _) = self.generate_attack_candidates(meta);
            self.gen_candidates_nanos += gen_start.elapsed().as_nanos() as u64;
            candidates
        };
        let legal_move_sets: Vec<Vec<Move>> = meta.positions.iter().map(|pos| {
            let h = pos.zobrist_hash;
            if let Some(cached) = self.legal_moves_cache.get(&h) {
                return cached.clone();
            }
            let moves = pos.generate_legal_moves();
            self.legal_moves_cache.insert(h, moves.clone());
            moves
        }).collect();
        if self.should_stop() {
            self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw.clone() });
            return;
        }
        let mut candidates = candidates_raw.clone();

        // ルートノードでは除外手をフィルタ
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
            self.or_table.insert(hash, PnDn { pn: INF, dn: 0 });
            self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw.clone() });
            return;
        }

        // 手順ヒントによる候補手の並べ替え
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

        // 子ノード（AND）の初期化
        let mut children: Vec<(Move, u64, u32, u32)> = Vec::with_capacity(candidates.len());
        for mv in &candidates {
            let ak = Self::and_key(hash, mv);
            let (cpn, cdn) = if let Some(e) = self.and_table.get(&ak) {
                (e.pn, e.dn)
            } else {
                (1, 1)
            };
            children.push((*mv, ak, cpn, cdn));
        }

        loop {
            if self.should_stop() {
                self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw.clone() });
                return;
            }

            // OR 集約: pn = min{pn_c}, dn = sum{dn_c}
            let pn_n = children.iter().map(|c| c.2).min().unwrap_or(INF);
            let dn_n = children.iter().map(|c| c.3).fold(0u32, |a, d| a.saturating_add(d));

            if pn_n == 0 || dn_n == 0 {
                self.or_table.insert(hash, PnDn { pn: pn_n, dn: dn_n });
                if pn_n == 0 {
                    // 証明駒の計算: 証明された AND 子ノードの proof_hand の要素ごと最小値
                    let fallback_ph = meta.positions.first()
                        .map(|p| p.hand(Color::Sente).counts_array())
                        .unwrap_or([0; 7]);
                    let mut or_ph = [u8::MAX; 7];
                    for child in &children {
                        if child.2 == 0 {
                            let and_ph = self.and_proof_hands.get(&child.1)
                                .copied().unwrap_or(fallback_ph);
                            for j in 0..7 {
                                or_ph[j] = or_ph[j].min(and_ph[j]);
                            }
                        }
                    }
                    if or_ph[0] == u8::MAX {
                        or_ph = fallback_ph;
                    }
                    self.or_proof_hands.insert(hash, or_ph);
                    self.record_proven_dominance(meta, or_ph);
                    // 盤面のみのハッシュで手順ヒントを記録
                    if !meta.positions.is_empty() {
                        let board_only_hash = meta_board_only_hash(meta);
                        if let Some(proven_child) = children.iter().find(|c| c.2 == 0) {
                            self.move_hints.insert(board_only_hash, proven_child.0);
                        }
                        // 証明木をキャッシュ（リプレイ用、複数手順対応）
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
                self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw.clone() });
                return;
            }

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.or_table.insert(hash, PnDn { pn: pn_n, dn: dn_n });
                self.or_candidate_cache.insert(hash, CachedOrCandidates { candidates: candidates_raw.clone() });
                return;
            }

            // 最善の子（pn 最小）を選択
            let (best_idx, _) = children.iter().enumerate().min_by_key(|(_, c)| c.2).unwrap();
            let pn_2nd = children
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != best_idx)
                .map(|(_, c)| c.2)
                .min()
                .unwrap_or(INF);
            let best_dn = children[best_idx].3;

            // 子のしきい値を計算（1+ε trick）
            let child_pn_limit =
                pn_limit.min(pn_2nd.saturating_add(1).saturating_add(pn_2nd / 4));
            let child_dn_limit = dn_limit.saturating_sub(dn_n).saturating_add(best_dn);

            // AND ノードに再帰
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

            // AND テーブルから値を読み戻す
            let ak = children[best_idx].1;
            let e = self.and_table.get(&ak).copied().unwrap_or(PnDn { pn: 1, dn: 1 });
            children[best_idx].2 = e.pn;
            children[best_idx].3 = e.dn;
        }
    }

    /// AND ノード（観測分岐）: 全分岐で詰めば証明
    ///
    /// pn = sum{pn_c}, dn = min{dn_c}
    /// 分岐は最大4つ: Illegal, Checkmate, Captured, NoCapture
    fn mid_and(
        &mut self,
        meta: &MetaPosition,
        mv: Move,
        legal_move_sets: &[Vec<Move>],
        pn_limit: u32,
        dn_limit: u32,
        path: &mut Vec<u64>,
        depth: u32,
    ) {
        self.mid_and_calls += 1;
        let meta_hash = meta_position_hash(meta);
        let and_key = Self::and_key(meta_hash, &mv);

        // Phase 1: キャッシュから展開結果を取得、なければ計算
        // remove で所有権を取り、ループ後に再挿入（borrow checker 回避）
        let expansion = if let Some(cached) = self.and_expansion_cache.remove(&and_key) {
            cached
        } else {
            // 手を適用して合法/不正に分割
            let (legal_meta, illegal_meta) =
                meta.apply_attack_move_split_with_sets(mv, legal_move_sets);

            if legal_meta.is_empty() && illegal_meta.is_empty() {
                self.and_table.insert(and_key, PnDn { pn: INF, dn: 0 });
                return;
            }

            // 全合法盤面で王手がかかっているか（衝立詰将棋: 毎手王手が必要）
            if !legal_meta.is_empty()
                && !legal_meta
                    .positions
                    .iter()
                    .all(|pos| pos.is_in_check(pos.side_to_move))
            {
                self.and_table.insert(and_key, PnDn { pn: INF, dn: 0 });
                return;
            }

            // 持ち駒取得: 全 position 同一なら正確値、混合なら成分最大値（保守的推定）
            let parent_hand = Self::parent_sente_hand(meta, [0; 7]);

            let mut obs_terminal: Vec<bool> = Vec::new();
            let mut obs_hash: Vec<u64> = Vec::new();
            let mut obs_metas: Vec<MetaPosition> = Vec::new();
            let mut obs_depth_inc: Vec<u32> = Vec::new();
            let mut obs_hands: Vec<[u8; 7]> = Vec::new();

            // Illegal 分岐
            if !illegal_meta.is_empty() {
                let bh = meta_position_hash(&illegal_meta);
                obs_terminal.push(false);
                obs_hash.push(bh);
                obs_hands.push(parent_hand);
                obs_metas.push(illegal_meta);
                obs_depth_inc.push(0);
            }

            // 合法分岐
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
                } else {
                    self.expand_defense_calls += 1;
                    let ed_start = std::time::Instant::now();
                    let raw_branches = legal_meta.expand_defense_moves(mv);
                    self.expand_defense_nanos += ed_start.elapsed().as_nanos() as u64;

                    // 同一観測タイプの分岐をマージ
                    // sente_hand_hash で分かれた分岐を統合し、AND の分岐数を削減
                    // 異なる合駒で生じた持ち駒の違いは legal/illegal split で処理される
                    let branches = Self::merge_observation_branches(raw_branches);

                    for (obs, branch_meta) in branches {
                        match obs {
                            Observation::Checkmate => {
                                // 持ち駒の成分最小値を使用（混合対応）
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                obs_terminal.push(true);
                                obs_hash.push(0);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(0);
                            }
                            Observation::Captured { .. } | Observation::NoCapture => {
                                if branch_meta.positions.len() > MAX_META_POSITIONS {
                                    self.and_table
                                        .insert(and_key, PnDn { pn: INF, dn: 0 });
                                    return;
                                }
                                // 持ち駒の成分最小値を使用（混合対応、保守的推定）
                                let branch_hand = Self::child_sente_hand(&branch_meta, parent_hand);
                                let bh = meta_position_hash(&branch_meta);
                                obs_terminal.push(false);
                                obs_hash.push(bh);
                                obs_hands.push(branch_hand);
                                obs_metas.push(branch_meta);
                                obs_depth_inc.push(2);
                            }
                            Observation::Illegal => {}
                        }
                    }
                }
            }

            if obs_terminal.is_empty() {
                self.and_table.insert(and_key, PnDn { pn: INF, dn: 0 });
                return;
            }

            CachedAndExpansion {
                obs_terminal,
                obs_hash,
                obs_metas,
                obs_depth_inc,
                obs_hands,
                parent_hand,
            }
        };

        let n = expansion.obs_terminal.len();

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

            // AND 集約: pn = sum{pn_c}, dn = min{dn_c}
            let pn_n = obs_pn.iter().fold(0u32, |a, &p| a.saturating_add(p));
            let dn_n = obs_dn.iter().copied().min().unwrap_or(INF);

            if pn_n == 0 || dn_n == 0 {
                self.and_table.insert(and_key, PnDn { pn: pn_n, dn: dn_n });
                if pn_n == 0 {
                    // AND 証明駒の計算
                    let mut and_ph = [0u8; 7];
                    for i in 0..n {
                        let child_ph = if expansion.obs_terminal[i] {
                            [0u8; 7]
                        } else {
                            self.or_proof_hands.get(&expansion.obs_hash[i])
                                .copied().unwrap_or(expansion.parent_hand)
                        };
                        let child_hand = expansion.obs_hands[i];
                        for j in 0..7 {
                            let eff = (child_ph[j] as i16) + (expansion.parent_hand[j] as i16)
                                - (child_hand[j] as i16);
                            let eff = eff.max(0) as u8;
                            and_ph[j] = and_ph[j].max(eff);
                        }
                    }
                    self.and_proof_hands.insert(and_key, and_ph);
                }
                break;
            }

            if pn_n >= pn_limit || dn_n >= dn_limit {
                self.and_table.insert(and_key, PnDn { pn: pn_n, dn: dn_n });
                break;
            }

            // 最弱の非終端子（dn 最小）を選択
            let best = (0..n)
                .filter(|&i| !expansion.obs_terminal[i])
                .min_by_key(|&i| obs_dn[i]);
            let best_idx = match best {
                Some(i) => i,
                None => {
                    self.and_table.insert(and_key, PnDn { pn: pn_n, dn: dn_n });
                    break;
                }
            };

            let dn_2nd = (0..n)
                .filter(|&i| i != best_idx)
                .map(|i| obs_dn[i])
                .min()
                .unwrap_or(INF);
            let best_pn = obs_pn[best_idx];

            // 子のしきい値を計算（1+ε trick）
            let child_dn_limit =
                dn_limit.min(dn_2nd.saturating_add(1).saturating_add(dn_2nd / 4));
            let child_pn_limit = pn_limit.saturating_sub(pn_n).saturating_add(best_pn);

            // OR ノードに再帰
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

            // OR テーブルから値を読み戻す
            let e = self
                .or_table
                .get(&child_hash)
                .copied()
                .unwrap_or(PnDn { pn: 1, dn: 1 });
            obs_pn[best_idx] = e.pn;
            obs_dn[best_idx] = e.dn;
        }

        // Phase 4: キャッシュに再挿入
        self.and_expansion_cache.insert(and_key, expansion);
    }

    /// OR ノードの初期値を取得（転置表 or デフォルト）
    /// ループ検出時は反証扱い
    fn lookup_or(&self, hash: u64, path: &[u64]) -> (u32, u32) {
        if path.contains(&hash) {
            return (INF, 0);
        }
        if let Some(e) = self.or_table.get(&hash) {
            (e.pn, e.dn)
        } else {
            (1, 1)
        }
    }

    /// 同一観測タイプの分岐をマージ（sente_hand_hash グループ統合）
    /// 異なる合駒で生じた持ち駒の違いは、後続の legal/illegal split で正しく処理される
    /// 3 つ以上の同一観測グループがある場合のみマージ（少数グループではコスト対効果が悪い）
    /// 同一観測タイプの分岐をマージ（sente_hand_hash グループ統合）
    /// マージ条件:
    ///   1. 同一観測タイプが 3 グループ以上存在する
    ///   2. マージ対象の合計 position 数が MAX_MERGE_POSITIONS 以下
    /// 条件2により、深い探索でのcompound merging（マージ→展開→再マージ）による
    /// MetaPosition サイズの指数的膨張を防ぐ
    const MAX_MERGE_POSITIONS: usize = 7;

    fn merge_observation_branches(
        branches: Vec<(Observation, MetaPosition)>,
    ) -> Vec<(Observation, MetaPosition)> {
        use super::metaposition::position_hash;

        // 各観測タイプの出現回数と合計 position 数をカウント
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
                // マージ対象: 同一観測の既存分岐に統合（重複排除つき）
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
                // マージ対象外: そのまま保持
                result.push((obs, meta));
            }
        }
        result
    }

    /// MetaPosition の sente_hand を取得（proof_hand 計算用）
    /// 全 position が同一の sente_hand を持つ場合はその値を返す（正確）
    /// 混合 sente_hand の場合（マージ後）は成分最小値を返す（保守的: proof_hand を大きくする方向）
    fn child_sente_hand(meta: &MetaPosition, default: [u8; 7]) -> [u8; 7] {
        if meta.positions.is_empty() {
            return default;
        }
        let first = meta.positions[0].hand(Color::Sente).counts_array();
        // 全 position が同一かチェック（ほとんどの場合 true → 高速パス）
        let all_same = meta.positions[1..].iter().all(|pos| {
            pos.hand(Color::Sente).counts_array() == first
        });
        if all_same {
            return first;
        }
        // 混合の場合: 成分最小値（proof_hand を大きくする保守的推定）
        let mut result = first;
        for pos in &meta.positions[1..] {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].min(h[j]);
            }
        }
        result
    }

    /// MetaPosition の sente_hand を取得（parent_hand 計算用）
    /// 全 position が同一の sente_hand を持つ場合はその値を返す（正確）
    /// 混合 sente_hand の場合（マージ後）は成分最大値を返す（保守的: proof_hand を大きくする方向）
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
        // 混合の場合: 成分最大値（proof_hand を大きくする保守的推定）
        let mut result = first;
        for pos in &meta.positions {
            let h = pos.hand(Color::Sente).counts_array();
            for j in 0..7 {
                result[j] = result[j].max(h[j]);
            }
        }
        result
    }

    /// 優越関係テーブルに proof_hand（証明に必要な最小の攻め方持ち駒）を記録する
    /// gote_hand を含むハッシュと組み合わせて使用: 同じ盤面+同じgote_hand で proof_hand <= S_new
    fn record_proven_dominance(&mut self, meta: &MetaPosition, proof_hand: [u8; 7]) {
        if meta.positions.is_empty() {
            return;
        }
        let board_hash = meta_board_set_hash(meta);
        self.add_dominance_entry(board_hash, proof_hand);
    }

    /// dominance_table にエントリを追加（Pareto 最小を維持）
    /// 追加された場合 true を返す
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

    /// 攻め方の候補手を生成
    /// 全盤面から王手の手を収集し、プローブ手を追加
    ///
    /// Phase 2 では幾何学的事前フィルタで王手候補を絞り込む:
    /// - 直接王手: 駒種の移動先が玉に利きうるか（遮蔽物無視）
    /// - 開き王手: 移動元が discovery_squares に含まれるか
    fn generate_attack_candidates(
        &mut self,
        meta: &MetaPosition,
    ) -> (Vec<Move>, Vec<Vec<Move>>) {
        if meta.positions.is_empty() {
            return (Vec::new(), Vec::new());
        }

        let n = meta.positions.len();
        self.gen_candidates_calls += 1;
        self.gen_candidates_total_positions += n as u64;

        // 各盤面の合法手セットを事前計算（局面レベルのキャッシュを活用）
        let legal_move_sets: Vec<Vec<Move>> = meta
            .positions
            .iter()
            .map(|pos| {
                if self.is_cancelled() {
                    return Vec::new();
                }
                let hash = pos.zobrist_hash;
                if let Some(cached) = self.legal_moves_cache.get(&hash) {
                    self.legal_moves_cache_hits += 1;
                    return cached.clone();
                }
                self.legal_moves_cache_misses += 1;
                let moves = pos.generate_legal_moves();
                self.legal_moves_cache.insert(hash, moves.clone());
                moves
            })
            .collect();

        if self.is_cancelled() {
            return (Vec::new(), legal_move_sets);
        }

        let mut seen = HashSet::new();
        let mut check_moves = Vec::new();

        // 全盤面から王手の手を収集（union）
        // 局面レベルのキャッシュ + 幾何学的事前フィルタで候補を絞り込む
        for (i, pos) in meta.positions.iter().enumerate() {
            let attacker = pos.side_to_move;
            let gote_king_sq = pos.find_king(attacker.opponent());
            let Some(ksq) = gote_king_sq else { continue };

            // 局面レベルの王手手キャッシュを確認
            let pos_hash = pos.zobrist_hash;
            if let Some(cached_checks) = self.check_moves_cache.get(&pos_hash) {
                for mv in cached_checks {
                    if seen.insert(*mv) {
                        check_moves.push(*mv);
                    }
                }
                continue;
            }

            // 開き王手の候補マスを事前計算
            let discovery_sqs = compute_discovery_squares(pos, ksq, attacker);
            let mut test_pos = pos.clone();
            let mut pos_checks = Vec::new();
            for mv in &legal_move_sets[i] {
                // 玉を取る手は王手ではない
                if mv.to == ksq {
                    continue;
                }
                // 幾何学的事前フィルタ: 王手になりえない手をスキップ
                if !could_give_check(mv, ksq, &discovery_sqs, attacker) {
                    continue;
                }
                // 正確な王手判定
                let undo = test_pos.make_move(*mv);
                let is_check = test_pos.is_attacked(ksq, attacker);
                if is_check {
                    pos_checks.push(*mv);
                }
                test_pos.unmake_move(&undo);
            }

            // キャッシュに保存
            for mv in &pos_checks {
                if seen.insert(*mv) {
                    check_moves.push(*mv);
                }
            }
            self.check_moves_cache.insert(pos_hash, pos_checks);
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
            (
                is_drop,
                dist,
                interposition,
                piece_priority,
                mv.to.file,
                mv.to.rank,
            )
        };
        check_moves.sort_by_key(|mv| sort_key(mv));

        // 合駒可能マスが多い長距離王手を除外（打ち駒のみ）
        // 盤上の駒を動かす手は持ち駒を消費しないため除外しない
        let king_sq_for_filter = meta.positions[0].find_king(Color::Gote);
        check_moves.retain(|mv| {
            // 盤上の駒を動かす手は常に候補に残す
            if mv.drop_piece.is_none() {
                return true;
            }
            let piece_kind = mv.drop_piece.unwrap();
            let is_slider = matches!(
                piece_kind,
                PieceKind::Rook
                    | PieceKind::Bishop
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

        // メタポジションが1つならプローブ不要
        if n <= 1 {
            return (check_moves, legal_move_sets);
        }

        // プローブ手: 一部の盤面でのみ合法な手
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
    // 証明木の抽出
    // ========================================================================

    /// 探索して SolutionData を返す（既存ソルバーと同じインターフェース）
    pub fn solve_to_solution(&mut self, meta: &MetaPosition, find_shortest: bool) -> SolutionData {
        self.solve_to_solution_inner(meta, false, find_shortest)
    }

    /// 余詰めチェック付きで探索して SolutionData を返す
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

                // 最短解を探す
                let (tree, depth, shorten_nodes) = if find_shortest && depth > 1 {
                    self.shorten_solution(meta, tree, depth)
                } else {
                    (tree, depth, 0)
                };
                let nodes_before_second = initial_nodes + shorten_nodes;

                // 余詰めチェック
                // find_second_solution は self.nodes_searched を first_phase_nodes として使うため、
                // shorten 分を含めた累計値を設定しておく
                self.nodes_searched = nodes_before_second;
                let (second_tree, kizu_trees, total_nodes, second_msg) = if find_second {
                    if let Some(ref t) = tree {
                        self.find_second_solution(meta, t)
                    } else {
                        (None, vec![], nodes_before_second, String::new())
                    }
                } else {
                    (None, vec![], nodes_before_second, String::new())
                };

                let kizu_suffix = if !kizu_trees.is_empty() {
                    format!("、キズ: {}件", kizu_trees.len())
                } else {
                    String::new()
                };

                let message = if second_tree.is_some() {
                    let second_depth = second_tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
                    format!(
                        "余詰めあり: {}手詰めと{}手詰めが見つかりました (探索ノード数: {}{})",
                        depth, second_depth, total_nodes, kizu_suffix
                    )
                } else if find_second && !second_msg.is_empty() {
                    if kizu_trees.is_empty() {
                        format!(
                            "{}手詰めが見つかりました（余詰めなし、探索ノード数: {}）",
                            depth, total_nodes
                        )
                    } else {
                        format!(
                            "{}手詰めが見つかりました（余詰めなし{}、探索ノード数: {}）",
                            depth, kizu_suffix, total_nodes
                        )
                    }
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
                    kizu_trees,
                    message,
                    trace: Vec::new(),
                }
            }
            TsuitateDfpnResult::Disproven => SolutionData {
                found: false,
                tree: None,
                second_tree: None,
                kizu_trees: vec![],
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
                kizu_trees: vec![],
                message: format!(
                    "探索を打ち切りました (探索ノード数: {})",
                    self.nodes_searched
                ),
                trace: Vec::new(),
            },
        }
    }

    /// 線形スキャンで最短の深さを求める
    ///
    /// 浅い深さから順に探索し、最初に証明に成功した深さが最短解。
    /// 常に「深くなる方向」に進むため、前回の証明済みエントリを常に再利用でき、
    /// 二分探索のような「浅くなる方向」での全テーブルクリアが不要。
    ///
    /// 最適化:
    /// - 深さ非依存のキャッシュ（and_expansion_cache, or_candidate_cache, move_hints,
    ///   proof_cache）は全反復で保持
    /// - or_table/and_table の証明済みエントリ (pn=0) を毎回保持
    ///   （浅い深さの証明は深い深さでも有効）
    /// - dominance_table, or_proof_hands, and_proof_hands も保持
    /// - OR ノードは偶数深さにのみ存在するため、奇数 depth_limit のみ試行
    ///   （depth_limit=2k と 2k+1 は同一結果 → step=2 で探索）
    fn shorten_solution(
        &mut self,
        meta: &MetaPosition,
        initial_tree: Option<SolutionNode>,
        initial_depth: u32,
    ) -> (Option<SolutionNode>, u32, u64) {
        let mut best_tree = initial_tree;
        let mut best_depth = initial_depth;
        let mut total_nodes: u64 = 0;

        // 初回: 無限深さから有限深さへの遷移。証明依存テーブルをクリア。
        // 深さ非依存キャッシュ（and_expansion_cache, or_candidate_cache,
        // move_hints, proof_cache）は保持。
        self.or_table.clear();
        self.and_table.clear();
        self.dominance_table.clear();
        self.or_proof_hands.clear();
        self.and_proof_hands.clear();

        // 浅い深さから順に探索（常に深くなる方向 → テーブル再利用可能）
        let mut depth: u32 = 1;
        while depth < best_depth {
            if self.should_stop() {
                break;
            }

            // 前回より深い → 証明済みエントリ(pn=0)のみ保持、反証・中間値は除去
            // dominance_table, or_proof_hands, and_proof_hands は保持
            if depth > 1 {
                self.or_table.retain(|_, v| v.pn == 0);
                self.and_table.retain(|_, v| v.pn == 0);
            }

            self.nodes_searched = 0;
            self.depth_limit = Some(depth);

            let result = self.solve(meta);
            total_nodes += self.nodes_searched;

            if result == TsuitateDfpnResult::Proven {
                let tree = self.extract_solution(meta);
                let actual_depth = tree.as_ref().map(|t| t.max_moves()).unwrap_or(0);
                if actual_depth < best_depth {
                    best_tree = tree;
                    best_depth = actual_depth;
                }
                break; // 最初に見つかった深さが最短
            }

            depth += 2; // OR ノードは偶数深さのみ → 奇数 depth_limit のみ意味がある
        }

        self.depth_limit = None;
        (best_tree, best_depth, total_nodes)
    }

    /// 1つ目の解の初手を除外して2つ目の解を探す
    /// MoveData から Move に変換する
    fn move_data_to_move(mv: &MoveData) -> Move {
        let to = Square::new(mv.to_file, mv.to_rank);
        let from = match (mv.from_file, mv.from_rank) {
            (Some(f), Some(r)) => Some(Square::new(f, r)),
            _ => None,
        };
        let drop_piece = mv.drop_piece.as_ref().and_then(|s| match s.as_str() {
            "飛" => Some(PieceKind::Rook),
            "角" => Some(PieceKind::Bishop),
            "金" => Some(PieceKind::Gold),
            "銀" => Some(PieceKind::Silver),
            "桂" => Some(PieceKind::Knight),
            "香" => Some(PieceKind::Lance),
            "歩" => Some(PieceKind::Pawn),
            _ => None,
        });
        Move {
            from,
            to,
            promotion: mv.promotion,
            drop_piece,
            moved_piece_kind: None,
        }
    }

    /// ソルバーの転置表・キャッシュを全てクリアする
    fn clear_tables(&mut self) {
        self.or_table.clear();
        self.and_table.clear();
        self.dominance_table.clear();
        self.or_proof_hands.clear();
        self.and_proof_hands.clear();
        self.move_hints.clear();
        self.proof_cache.clear();
        self.and_expansion_cache.clear();
        self.or_candidate_cache.clear();
        self.legal_moves_cache.clear();
        self.check_moves_cache.clear();
    }

    /// 最長応手経路上の全ORノードで証明手が一意かどうかをチェックする
    ///
    /// 初回solve後、テーブルをクリアする前に呼び出す。
    /// 解の手順木をルートから最長応手分岐に沿って走査し、各ORノードで
    /// ANDテーブルの pn==0 の手を数える。いずれかの最長経路上で全ORノードが
    /// 一意（proven_count <= 1）であれば true を返す。
    ///
    /// extract_solution で保存された meta_hash を使用してANDテーブルを参照するため、
    /// MetaPosition の再構築が不要。
    fn has_unique_longest_path(&self, meta: &MetaPosition, node: &SolutionNode) -> bool {
        self.check_unique_longest_recursive(meta, node)
    }

    fn check_unique_longest_recursive(
        &self,
        meta: &MetaPosition,
        node: &SolutionNode,
    ) -> bool {
        let (mv_data, branches, meta_hash) = match node {
            SolutionNode::Checkmate { .. } => return true,
            SolutionNode::AttackMove { mv, branches, meta_hash } => (mv, branches, *meta_hash),
        };

        // 最終手は対象外（最終手余詰は余詰と見なさない）
        if node.is_final_move() {
            return true;
        }

        // extract_solution で保存された meta_hash を使用
        // meta_hash がない場合（旧ソルバー等）は MetaPosition から計算
        let hash = meta_hash.unwrap_or_else(|| meta_position_hash(meta));

        // meta_hash がある場合はそれを使ってANDテーブルのみで証明手数を確認
        // （MetaPosition から合法手を生成して全候補をチェックする）
        let legal_move_sets: Vec<Vec<Move>> = meta
            .positions
            .iter()
            .map(|pos| pos.generate_legal_moves())
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

        // ANDテーブルで pn==0 の手を数える
        let proven_count = all_moves.iter().filter(|mv| {
            let ak = Self::and_key(hash, mv);
            self.and_table.get(&ak).map_or(false, |e| e.pn == 0)
        }).count();

        if proven_count > 1 {
            return false; // 複数の証明手がある → 一意でない
        }

        // 最長分岐の深さを計算
        let max_depth = node.max_moves();

        // 最長分岐のみ再帰
        // 子ノードの MetaPosition は extract_solution で meta_hash が保存されているため、
        // MetaPosition 再構築の代わりに meta_hash を直接参照する。
        // ただし子ノードの合法手生成には MetaPosition が必要なので、
        // 再構築可能な場合のみ再帰する。再構築できない場合は保守的に false を返す。
        let mv = Self::move_data_to_move(mv_data);
        let (legal_meta, illegal_meta) = if !meta.is_empty() {
            meta.apply_attack_move_split_fast(mv)
        } else {
            // meta が空の場合、子ノードの MetaPosition を再構築できないが、
            // 子ノードに meta_hash があれば AND テーブルのハッシュ参照は可能。
            // ただし合法手リスト生成ができないため保守的に false を返す。
            return false;
        };

        let obs_metas: Vec<(Observation, MetaPosition)> =
            if !legal_meta.is_empty() && !legal_meta.all_effectively_checkmate() {
                let raw = legal_meta.expand_defense_moves(mv);
                Self::merge_observation_branches(raw)
            } else {
                vec![]
            };

        for branch in branches {
            let branch_depth = match branch.observation {
                Observation::Checkmate => 1,
                Observation::Illegal => branch.continuation.max_moves(),
                _ => 2 + branch.continuation.max_moves(),
            };
            if branch_depth < max_depth {
                continue; // 最長ではない分岐はスキップ
            }

            let child_meta = match &branch.observation {
                Observation::Checkmate => continue,
                Observation::Illegal => illegal_meta.clone(),
                obs => match obs_metas.iter().find(|(o, _)| o == obs) {
                    Some((_, m)) => m.clone(),
                    None => continue, // 再構築不可 → この分岐をスキップ
                },
            };

            if self.check_unique_longest_recursive(&child_meta, &branch.continuation) {
                return true;
            }
        }

        false
    }

    /// キズ（許容余詰め）の判定
    ///
    /// 主解の手と別解の手が両方ともプローブ手（Illegal分岐を持つ）である場合、
    /// これはプローブ代替手に過ぎず、真の余詰めではないと判断する。
    fn is_kizu(main_node: &SolutionNode, alt_node: &SolutionNode) -> bool {
        fn has_illegal_branch(node: &SolutionNode) -> bool {
            match node {
                SolutionNode::AttackMove { branches, .. } => {
                    branches.iter().any(|b| b.observation == Observation::Illegal)
                }
                _ => false,
            }
        }
        has_illegal_branch(main_node) && has_illegal_branch(alt_node)
    }

    fn find_second_solution(
        &mut self,
        meta: &MetaPosition,
        first_tree: &SolutionNode,
    ) -> (Option<SolutionNode>, Vec<SolutionNode>, u64, String) {
        let first_mv = match first_tree {
            SolutionNode::AttackMove { mv, .. } => Self::move_data_to_move(mv),
            SolutionNode::Checkmate { .. } => {
                return (None, vec![], self.nodes_searched, String::new());
            }
        };

        // プレチェック: テーブルクリア前に最長応手経路の一意性を確認
        if self.has_unique_longest_path(meta, first_tree) {
            return (None, vec![], self.nodes_searched, "unique_longest_path".to_string());
        }

        let first_hashes = first_tree.collect_attack_subtree_hashes();
        let initial_nodes = self.nodes_searched;
        let mut kizu_trees: Vec<SolutionNode> = vec![];

        // Phase 1: 初手除外で再探索
        let mut excluded = vec![first_mv];
        if first_mv.from.is_some() {
            let mut counterpart = first_mv;
            counterpart.promotion = !counterpart.promotion;
            excluded.push(counterpart);
        }

        self.clear_tables();
        self.nodes_searched = 0;
        self.excluded_root_moves = excluded;

        let result = self.solve(meta);
        let mut total_nodes = initial_nodes + self.nodes_searched;
        self.excluded_root_moves.clear();

        if let TsuitateDfpnResult::Proven = result {
            let second_tree = self.extract_solution(meta);
            if let Some(ref second) = second_tree {
                if !second.is_subsumed_by(&first_hashes) {
                    if Self::is_kizu(first_tree, second) {
                        // キズ（プローブ代替手）として収集、探索を続行
                        kizu_trees.push(second_tree.unwrap());
                    } else {
                        // 真の余詰め → 即座に返す
                        return (second_tree, kizu_trees, total_nodes, "found".to_string());
                    }
                }
            }
        }

        // Phase 2: 内部ORノードで別手順を探索
        if let Some(alt_tree) = self.find_inner_alternative(
            meta, first_tree, &first_hashes, &mut total_nodes, &mut kizu_trees,
        ) {
            return (Some(alt_tree), kizu_trees, total_nodes, "found".to_string());
        }

        let method = if kizu_trees.is_empty() { "not_found" } else { "kizu_only" };
        (None, kizu_trees, total_nodes, method.to_string())
    }

    /// 手順木の内部ORノードで余詰め（別手順）を探す
    ///
    /// 1つ目の解の手順木を辿りながら、各ORノードで選ばれた手を除外して
    /// 再探索し、別の詰み手順がないかチェックする。
    /// キズ（プローブ代替手）は kizu_trees に収集し、真の余詰めのみ返す。
    fn find_inner_alternative(
        &mut self,
        meta: &MetaPosition,
        first_tree: &SolutionNode,
        first_hashes: &HashSet<u64>,
        total_nodes: &mut u64,
        kizu_trees: &mut Vec<SolutionNode>,
    ) -> Option<SolutionNode> {
        self.find_inner_alt_recursive(meta, first_tree, first_tree, first_hashes, total_nodes, kizu_trees)
    }

    fn find_inner_alt_recursive(
        &mut self,
        meta: &MetaPosition,
        node: &SolutionNode,
        root_node: &SolutionNode,
        first_hashes: &HashSet<u64>,
        total_nodes: &mut u64,
        kizu_trees: &mut Vec<SolutionNode>,
    ) -> Option<SolutionNode> {
        let (mv_data, branches) = match node {
            SolutionNode::Checkmate { .. } => return None,
            SolutionNode::AttackMove { mv, branches, .. } => (mv, branches),
        };

        let mv = Self::move_data_to_move(mv_data);

        // この攻め手を適用して観測分岐を復元
        let (legal_meta, illegal_meta) = meta.apply_attack_move_split_fast(mv);

        let obs_metas: Vec<(Observation, MetaPosition)> =
            if !legal_meta.is_empty() && !legal_meta.all_effectively_checkmate() {
                let raw = legal_meta.expand_defense_moves(mv);
                Self::merge_observation_branches(raw)
            } else {
                vec![]
            };

        // 最長応手分岐のみ探索
        let max_depth = node.max_moves();

        // 各ブランチの子MetaPositionを特定して再帰
        for (branch_idx, branch) in branches.iter().enumerate() {
            // 最長でない分岐はスキップ
            let branch_depth = match branch.observation {
                Observation::Checkmate => 1,
                Observation::Illegal => branch.continuation.max_moves(),
                _ => 2 + branch.continuation.max_moves(),
            };
            if branch_depth < max_depth {
                continue;
            }

            let child_meta = match &branch.observation {
                Observation::Checkmate => continue,
                Observation::Illegal => illegal_meta.clone(),
                obs => match obs_metas.iter().find(|(o, _)| o == obs) {
                    Some((_, m)) => m.clone(),
                    None => continue,
                },
            };

            // この子ORノードで別の手を試す（最終手は余詰の対象外）
            if let SolutionNode::AttackMove { mv: child_mv, .. } = branch.continuation.as_ref() {
                if !branch.continuation.is_final_move() {
                    let child_move = Self::move_data_to_move(child_mv);
                    if let Some(alt_subtree) =
                        self.try_solve_excluding(&child_meta, child_move, first_hashes, total_nodes)
                    {
                        if Self::is_kizu(&branch.continuation, &alt_subtree) {
                            // キズとして収集、full tree を構築して kizu_trees に追加
                            let mut new_branches = branches.clone();
                            new_branches[branch_idx].continuation = Box::new(alt_subtree);
                            kizu_trees.push(Self::rebuild_full_tree(root_node, node, branch_idx, new_branches.clone()));
                            // continue — 真の余詰めを探し続ける
                        } else {
                            // 真の余詰め → 即座に返す
                            let mut new_branches = branches.clone();
                            new_branches[branch_idx].continuation = Box::new(alt_subtree);
                            return Some(SolutionNode::AttackMove {
                                mv: mv_data.clone(),
                                branches: new_branches,
                                meta_hash: None,
                            });
                        }
                    }
                }
            }

            // 更に深いノードで試す
            if let Some(deep_alt) = self.find_inner_alt_recursive(
                &child_meta,
                &branch.continuation,
                root_node,
                first_hashes,
                total_nodes,
                kizu_trees,
            ) {
                let mut new_branches = branches.clone();
                new_branches[branch_idx].continuation = Box::new(deep_alt);
                return Some(SolutionNode::AttackMove {
                    mv: mv_data.clone(),
                    branches: new_branches,
                    meta_hash: None,
                });
            }
        }

        None
    }

    /// find_inner_alt_recursive でキズを検出した際、ルートからの完全な手順木を構築する
    ///
    /// inner_alt_recursive は再帰的に呼び出されるため、検出時点では部分木しか持っていない。
    /// キズの表示にはルートからの完全な手順木が必要なので、root_node をベースに
    /// 変更箇所のみ差し替えた完全な手順木を返す。
    ///
    /// 簡易実装: 現在のノードレベルの new_branches を使って AttackMove を構築する。
    /// find_inner_alt_recursive の戻り値と同様の形式（親ノードの branches を差し替えた部分木）。
    fn rebuild_full_tree(
        _root_node: &SolutionNode,
        current_node: &SolutionNode,
        _branch_idx: usize,
        new_branches: Vec<SolutionBranch>,
    ) -> SolutionNode {
        match current_node {
            SolutionNode::AttackMove { mv, .. } => SolutionNode::AttackMove {
                mv: mv.clone(),
                branches: new_branches,
                meta_hash: None,
            },
            _ => current_node.clone(),
        }
    }

    /// 指定MetaPositionで特定の手を除外して解を探す
    fn try_solve_excluding(
        &mut self,
        meta: &MetaPosition,
        excluded_move: Move,
        first_hashes: &HashSet<u64>,
        total_nodes: &mut u64,
    ) -> Option<SolutionNode> {
        if self.is_cancelled() {
            return None;
        }

        let mut excluded = vec![excluded_move];
        if excluded_move.from.is_some() {
            let mut counterpart = excluded_move;
            counterpart.promotion = !counterpart.promotion;
            excluded.push(counterpart);
        }

        self.clear_tables();
        self.nodes_searched = 0;
        self.excluded_root_moves = excluded;

        let result = self.solve(meta);
        *total_nodes += self.nodes_searched;
        self.excluded_root_moves.clear();

        match result {
            TsuitateDfpnResult::Proven => {
                let tree = self.extract_solution(meta);
                if let Some(ref t) = tree {
                    if !t.is_subsumed_by(first_hashes) {
                        return tree;
                    }
                }
                None
            }
            _ => None,
        }
    }

    /// 証明木を抽出する（solve() で Proven になった後に呼ぶ）
    pub fn extract_solution(&self, meta: &MetaPosition) -> Option<SolutionNode> {
        self.extract_or(meta, 0)
    }

    /// OR ノードから証明木を抽出する
    fn extract_or(&self, meta: &MetaPosition, depth: u32) -> Option<SolutionNode> {
        if meta.is_empty() {
            return None;
        }
        if meta.all_effectively_checkmate() {
            return Some(SolutionNode::Checkmate { depth });
        }

        let hash = meta_position_hash(meta);
        let entry = self.or_table.get(&hash)?;
        if entry.pn != 0 {
            return None; // 証明されていない
        }

        // 全盤面の合法手を収集
        let legal_move_sets: Vec<Vec<Move>> = meta
            .positions
            .iter()
            .map(|pos| pos.generate_legal_moves())
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

        // 候補手をソート（解の見た目を良くするため）
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

        // AND テーブルで pn=0 の手を探す
        for mv in &all_moves {
            let ak = Self::and_key(hash, mv);
            if let Some(and_entry) = self.and_table.get(&ak) {
                if and_entry.pn == 0 {
                    if let Some(branches) =
                        self.extract_and(meta, *mv, &legal_move_sets, depth)
                    {
                        return Some(SolutionNode::AttackMove {
                            mv: MoveData::from_move(*mv, Color::Sente),
                            branches,
                            meta_hash: Some(hash),
                        });
                    }
                }
            }
        }

        // AND テーブルで見つからない場合（優越関係で証明されたノード）
        // 各候補手で直接 extract_and を試す
        for mv in &all_moves {
            if let Some(branches) = self.extract_and(meta, *mv, &legal_move_sets, depth) {
                return Some(SolutionNode::AttackMove {
                    mv: MoveData::from_move(*mv, Color::Sente),
                    branches,
                    meta_hash: Some(hash),
                });
            }
        }
        None
    }

    /// AND ノード（観測分岐）から証明木を抽出する
    fn extract_and(
        &self,
        meta: &MetaPosition,
        mv: Move,
        legal_move_sets: &[Vec<Move>],
        depth: u32,
    ) -> Option<Vec<SolutionBranch>> {
        let (legal_meta, illegal_meta) =
            meta.apply_attack_move_split_with_sets(mv, legal_move_sets);

        let mut branches = Vec::new();

        // Illegal 分岐
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

        // 全合法盤面で実質詰み
        if legal_meta.all_effectively_checkmate() {
            branches.push(SolutionBranch {
                observation: Observation::Checkmate,
                continuation: Box::new(SolutionNode::Checkmate { depth: depth + 1 }),
            });
            return Some(branches);
        }

        // 玉方の応手を展開（mid_and と同じマージを適用）
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
                    // depth + 2: 攻め方の手(+1) + 玉方の応手(+1)
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
    // コンパクト証明木の抽出（リプレイキャッシュ用）
    // ========================================================================

    /// コンパクト証明木を抽出（リプレイキャッシュ用）
    fn extract_compact_or(&self, meta: &MetaPosition) -> Option<ProofNode> {
        if meta.is_empty() { return None; }
        if meta.all_effectively_checkmate() { return Some(ProofNode::Checkmate); }

        let hash = meta_position_hash(meta);
        let entry = self.or_table.get(&hash)?;
        if entry.pn != 0 { return None; }

        // 全合法手を収集（extract_or と同じ）
        let legal_move_sets: Vec<Vec<Move>> = meta.positions.iter()
            .map(|pos| pos.generate_legal_moves())
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
            let ak = Self::and_key(hash, mv);
            if let Some(e) = self.and_table.get(&ak) {
                if e.pn == 0 {
                    if let Some(branches) = self.extract_compact_and(meta, *mv, &legal_move_sets) {
                        return Some(ProofNode::Attack { mv: *mv, branches });
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
        legal_move_sets: &[Vec<Move>],
    ) -> Option<Vec<(ProofObs, ProofNode)>> {
        let (legal_meta, illegal_meta) = meta.apply_attack_move_split_with_sets(mv, legal_move_sets);
        let mut branches = Vec::new();

        if !illegal_meta.is_empty() {
            let node = self.extract_compact_or(&illegal_meta)?;
            branches.push((ProofObs::Illegal, node));
        }
        if legal_meta.is_empty() {
            return if branches.is_empty() { None } else { Some(branches) };
        }
        if legal_meta.all_effectively_checkmate() {
            branches.push((ProofObs::Checkmate, ProofNode::Checkmate));
            return Some(branches);
        }

        let raw_obs_branches = legal_meta.expand_defense_moves(mv);
        let obs_branches = Self::merge_observation_branches(raw_obs_branches);
        for (obs, branch_meta) in obs_branches {
            match obs {
                Observation::Checkmate => {
                    branches.push((ProofObs::Checkmate, ProofNode::Checkmate));
                }
                Observation::Captured { file, rank } => {
                    let node = self.extract_compact_or(&branch_meta)?;
                    branches.push((ProofObs::Captured(file, rank), node));
                }
                Observation::NoCapture => {
                    let node = self.extract_compact_or(&branch_meta)?;
                    branches.push((ProofObs::NoCapture, node));
                }
                Observation::Illegal => {}
            }
        }
        Some(branches)
    }

    // ========================================================================
    // 証明木リプレイ
    // ========================================================================

    /// 証明木のリプレイを試みる
    /// 成功: Some(proof_hand) — 転置表を更新済み
    /// 失敗: None — フォールバック
    fn try_replay_proof(
        &mut self,
        meta: &MetaPosition,
        proof: &ProofNode,
        hash: u64,
        path: &[u64],
        depth: u32,
    ) -> Option<[u8; 7]> {
        // ループ検出
        if path.contains(&hash) { return None; }
        // 深さ制限チェック
        if let Some(limit) = self.depth_limit {
            if depth >= limit { return None; }
        }
        // should_stop チェック
        if self.should_stop() { return None; }

        match proof {
            ProofNode::Checkmate => {
                if meta.all_effectively_checkmate() {
                    let proof_hand = [0u8; 7];
                    self.or_table.insert(hash, PnDn { pn: 0, dn: INF });
                    self.or_proof_hands.insert(hash, proof_hand);
                    Some(proof_hand)
                } else {
                    None
                }
            }
            ProofNode::Attack { mv, branches: proof_branches } => {
                // 高速分割（全合法手生成をスキップ）
                let (legal_meta, illegal_meta) = meta.apply_attack_move_split_fast(*mv);

                // 全合法盤面で王手がかかっているか（毎手王手の要件）
                if !legal_meta.is_empty()
                    && !legal_meta.positions.iter()
                        .all(|pos| pos.is_in_check(pos.side_to_move))
                {
                    return None;
                }

                // 実際の観測分岐を構築
                let mut actual_branches: Vec<(Observation, MetaPosition)> = Vec::new();

                // Illegal 分岐
                if !illegal_meta.is_empty() {
                    actual_branches.push((Observation::Illegal, illegal_meta));
                }

                // 合法分岐
                if !legal_meta.is_empty() {
                    if legal_meta.all_effectively_checkmate() {
                        actual_branches.push((Observation::Checkmate, MetaPosition { positions: Vec::new() }));
                    } else {
                        let raw_obs = legal_meta.expand_defense_moves(*mv);
                        let obs = Self::merge_observation_branches(raw_obs);
                        actual_branches.extend(obs);
                    }
                }

                if actual_branches.is_empty() { return None; }

                // 各実際の分岐に対応する証明分岐を探してリプレイ
                // 部分リプレイ: 一部の分岐が失敗しても、成功した分岐の結果は
                // 転置表に残す（再帰呼び出し内で or_table に記録される）。
                // これにより、フォールバック時の df-pn で成功済みの分岐をスキップできる。
                let parent_hand = meta.positions.first()
                    .map(|p| p.hand(Color::Sente).counts_array())
                    .unwrap_or([0; 7]);
                let mut and_ph = [0u8; 7];
                let mut all_succeeded = true;

                for (actual_obs, actual_meta) in &actual_branches {
                    // 証明分岐から一致するもの全てを収集（sente_hand_hash グループ化対応）
                    let matching_proofs: Vec<&ProofNode> = proof_branches.iter()
                        .filter(|(po, _)| po.matches(actual_obs))
                        .map(|(_, sp)| sp)
                        .collect();
                    if matching_proofs.is_empty() {
                        all_succeeded = false;
                        continue;
                    }

                    match actual_obs {
                        Observation::Checkmate => {
                            // Checkmate は proof_hand 寄与 0
                        }
                        Observation::Illegal => {
                            let child_hash = meta_position_hash(actual_meta);
                            let mut child_path = path.to_vec();
                            child_path.push(hash);
                            let mut branch_ok = false;
                            for sub_proof in &matching_proofs {
                                if let Some(child_ph) = self.try_replay_proof(
                                    actual_meta, sub_proof, child_hash, &child_path, depth,
                                ) {
                                    for j in 0..7 {
                                        let eff = (child_ph[j] as i16 + parent_hand[j] as i16
                                            - parent_hand[j] as i16).max(0) as u8;
                                        and_ph[j] = and_ph[j].max(eff);
                                    }
                                    branch_ok = true;
                                    break;
                                }
                            }
                            if !branch_ok { all_succeeded = false; }
                        }
                        Observation::Captured { .. } | Observation::NoCapture => {
                            let child_hash = meta_position_hash(actual_meta);
                            let child_hand = actual_meta.positions.first()
                                .map(|p| p.hand(Color::Sente).counts_array())
                                .unwrap_or(parent_hand);
                            let mut child_path = path.to_vec();
                            child_path.push(hash);
                            let mut branch_ok = false;
                            for sub_proof in &matching_proofs {
                                if let Some(child_ph) = self.try_replay_proof(
                                    actual_meta, sub_proof, child_hash, &child_path, depth + 2,
                                ) {
                                    for j in 0..7 {
                                        let eff = (child_ph[j] as i16 + parent_hand[j] as i16
                                            - child_hand[j] as i16).max(0) as u8;
                                        and_ph[j] = and_ph[j].max(eff);
                                    }
                                    branch_ok = true;
                                    break;
                                }
                            }
                            if !branch_ok { all_succeeded = false; }
                        }
                    }
                }

                // move_hints は部分成功でも記録（df-pn フォールバック時に候補手ソートで有効）
                if !meta.positions.is_empty() {
                    let board_only_hash = meta_board_only_hash(meta);
                    self.move_hints.entry(board_only_hash).or_insert(*mv);
                }

                if all_succeeded {
                    // 全分岐成功 → OR ノードの proof_hand
                    let or_ph = and_ph;

                    self.or_table.insert(hash, PnDn { pn: 0, dn: INF });
                    self.or_proof_hands.insert(hash, or_ph);
                    self.record_proven_dominance(meta, or_ph);

                    // and_table にも記録
                    let and_key = Self::and_key(hash, mv);
                    self.and_table.insert(and_key, PnDn { pn: 0, dn: INF });
                    self.and_proof_hands.insert(and_key, and_ph);

                    Some(or_ph)
                } else {
                    // 部分失敗: 成功したサブ分岐は再帰呼び出し内で転置表に記録済み
                    None
                }
            }
        }
    }
}
