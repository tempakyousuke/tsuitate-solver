use std::collections::HashSet;
use std::hash::{Hash, Hasher};

use super::bitboard::Bitboard;
use super::movegen;
use super::types::*;

// === Zobrist Hashing ===

struct ZobristKeys {
    piece: [[[u64; 81]; 14]; 2], // [color][kind][square]
    hand: [[[u64; 20]; 7]; 2],   // [color][hand_kind_idx][count]
    color: u64,                   // XOR when side changes
}

const fn splitmix64_next(state: u64) -> (u64, u64) {
    let s = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = s;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z = z ^ (z >> 31);
    (z, s)
}

const fn piece_kind_zobrist_index(kind: PieceKind) -> usize {
    match kind {
        PieceKind::King => 0,
        PieceKind::Rook => 1,
        PieceKind::Bishop => 2,
        PieceKind::Gold => 3,
        PieceKind::Silver => 4,
        PieceKind::Knight => 5,
        PieceKind::Lance => 6,
        PieceKind::Pawn => 7,
        PieceKind::PromotedRook => 8,
        PieceKind::PromotedBishop => 9,
        PieceKind::PromotedSilver => 10,
        PieceKind::PromotedKnight => 11,
        PieceKind::PromotedLance => 12,
        PieceKind::PromotedPawn => 13,
    }
}

const fn color_zobrist_index(color: Color) -> usize {
    match color {
        Color::Sente => 0,
        Color::Gote => 1,
    }
}

const fn hand_kind_zobrist_index(kind: PieceKind) -> usize {
    match kind {
        PieceKind::Rook => 0,
        PieceKind::Bishop => 1,
        PieceKind::Gold => 2,
        PieceKind::Silver => 3,
        PieceKind::Knight => 4,
        PieceKind::Lance => 5,
        PieceKind::Pawn => 6,
        _ => 0, // unreachable in normal use
    }
}

const fn generate_zobrist_keys() -> ZobristKeys {
    let mut keys = ZobristKeys {
        piece: [[[0u64; 81]; 14]; 2],
        hand: [[[0u64; 20]; 7]; 2],
        color: 0,
    };
    let mut state: u64 = 0x85A3_08D3_1319_8A2E;

    let mut c = 0usize;
    while c < 2 {
        let mut k = 0usize;
        while k < 14 {
            let mut s = 0usize;
            while s < 81 {
                let (val, new_state) = splitmix64_next(state);
                keys.piece[c][k][s] = val;
                state = new_state;
                s += 1;
            }
            k += 1;
        }
        c += 1;
    }

    c = 0;
    while c < 2 {
        let mut k = 0usize;
        while k < 7 {
            let mut n = 0usize;
            while n < 20 {
                let (val, new_state) = splitmix64_next(state);
                keys.hand[c][k][n] = val;
                state = new_state;
                n += 1;
            }
            k += 1;
        }
        c += 1;
    }

    let (val, _) = splitmix64_next(state);
    keys.color = val;

    keys
}

static ZOBRIST: ZobristKeys = generate_zobrist_keys();

/// 盤面状態
#[derive(Debug, Clone)]
pub struct Position {
    /// 9x9の盤面 (board[file-1][rank-1])
    board: [Option<Piece>; 81],
    /// 先手の持ち駒
    pub sente_hand: HandPieces,
    /// 後手の持ち駒
    pub gote_hand: HandPieces,
    /// 手番
    pub side_to_move: Color,
    /// Zobrist ハッシュ値（インクリメンタル更新）
    pub zobrist_hash: u64,
    /// Zobrist ハッシュの先手持ち駒コンポーネント
    zobrist_sente_hand: u64,
    /// Zobrist ハッシュの後手持ち駒コンポーネント
    zobrist_gote_hand: u64,
    /// 色別の占有ビットボード [先手, 後手]（board と常に同期、set_piece/remove_piece で更新）
    occupancy: [Bitboard; 2],
}

impl Hash for Position {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.board.hash(state);
        self.sente_hand.hash(state);
        self.gote_hand.hash(state);
        self.side_to_move.hash(state);
    }
}

impl PartialEq for Position {
    fn eq(&self, other: &Self) -> bool {
        self.board == other.board
            && self.sente_hand == other.sente_hand
            && self.gote_hand == other.gote_hand
            && self.side_to_move == other.side_to_move
    }
}

impl Eq for Position {}

/// 指し手の取消に必要な情報
#[derive(Debug, Clone)]
pub struct UndoInfo {
    pub mv: Move,
    pub captured: Option<Piece>,
    pub moved_piece: Piece,
}

impl Position {
    /// 空の盤面作成
    pub fn new() -> Self {
        // 空盤面の初期 Zobrist ハッシュ: 全持ち駒カウント0のキーを XOR
        let mut sente_hand_hash = 0u64;
        let mut gote_hand_hash = 0u64;
        for hi in 0..7 {
            sente_hand_hash ^= ZOBRIST.hand[0][hi][0];
            gote_hand_hash ^= ZOBRIST.hand[1][hi][0];
        }
        Self {
            board: [None; 81],
            sente_hand: HandPieces::new(),
            gote_hand: HandPieces::new(),
            side_to_move: Color::Sente,
            zobrist_hash: sente_hand_hash ^ gote_hand_hash,
            zobrist_sente_hand: sente_hand_hash,
            zobrist_gote_hand: gote_hand_hash,
            occupancy: [0, 0],
        }
    }

    #[inline]
    const fn occ_index(color: Color) -> usize {
        match color {
            Color::Sente => 0,
            Color::Gote => 1,
        }
    }

    /// 指定した色の占有ビットボード
    #[inline]
    pub fn occupancy(&self, color: Color) -> Bitboard {
        self.occupancy[Self::occ_index(color)]
    }

    /// 全駒の占有ビットボード
    #[inline]
    pub fn occupancy_all(&self) -> Bitboard {
        self.occupancy[0] | self.occupancy[1]
    }

    /// マス目の駒を取得
    #[inline]
    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.board[sq.index()]
    }

    /// 駒を配置
    pub fn set_piece(&mut self, sq: Square, piece: Piece) {
        let idx = sq.index();
        if let Some(old) = self.board[idx] {
            self.occupancy[Self::occ_index(old.color)] &= !(1u128 << idx);
        }
        self.board[idx] = Some(piece);
        self.occupancy[Self::occ_index(piece.color)] |= 1u128 << idx;
    }

    /// 駒を除去
    pub fn remove_piece(&mut self, sq: Square) -> Option<Piece> {
        let idx = sq.index();
        let piece = self.board[idx];
        if let Some(p) = piece {
            self.occupancy[Self::occ_index(p.color)] &= !(1u128 << idx);
        }
        self.board[idx] = None;
        piece
    }

    /// 持ち駒の参照
    pub fn hand(&self, color: Color) -> &HandPieces {
        match color {
            Color::Sente => &self.sente_hand,
            Color::Gote => &self.gote_hand,
        }
    }

    /// 持ち駒の可変参照
    pub fn hand_mut(&mut self, color: Color) -> &mut HandPieces {
        match color {
            Color::Sente => &mut self.sente_hand,
            Color::Gote => &mut self.gote_hand,
        }
    }

    /// 指定した色の玉の位置を探す
    pub fn find_king(&self, color: Color) -> Option<Square> {
        let mut bb = self.occupancy(color);
        while bb != 0 {
            let idx = bb.trailing_zeros() as usize;
            bb &= bb - 1;
            if let Some(piece) = self.board[idx] {
                if piece.kind == PieceKind::King {
                    return Some(Square::from_index(idx));
                }
            }
        }
        None
    }

    /// Zobrist 持ち駒ハッシュ更新ヘルパー
    #[inline]
    fn update_hand_zobrist(&mut self, color: Color, kind: PieceKind, old_count: usize, new_count: usize) {
        let ci = color_zobrist_index(color);
        let hi = hand_kind_zobrist_index(kind);
        let delta = ZOBRIST.hand[ci][hi][old_count] ^ ZOBRIST.hand[ci][hi][new_count];
        self.zobrist_hash ^= delta;
        match color {
            Color::Sente => self.zobrist_sente_hand ^= delta,
            Color::Gote => self.zobrist_gote_hand ^= delta,
        }
    }

    /// 指し手の実行
    pub fn make_move(&mut self, mv: Move) -> UndoInfo {
        let color = self.side_to_move;
        let ci = color_zobrist_index(color);

        if let Some(drop_kind) = mv.drop_piece {
            // 駒を打つ
            let piece = Piece::new(color, drop_kind);

            // Zobrist: 持ち駒から除去
            let old_count = self.hand(color).count(drop_kind) as usize;
            self.update_hand_zobrist(color, drop_kind, old_count, old_count - 1);
            self.hand_mut(color).remove(drop_kind);

            // Zobrist: 盤面に配置
            let ki = piece_kind_zobrist_index(drop_kind);
            self.zobrist_hash ^= ZOBRIST.piece[ci][ki][mv.to.index()];
            self.set_piece(mv.to, piece);

            // Zobrist: 手番変更
            self.zobrist_hash ^= ZOBRIST.color;
            self.side_to_move = color.opponent();

            UndoInfo { mv, captured: None, moved_piece: piece }
        } else {
            // 盤上の駒を移動
            let from = mv.from.unwrap();
            let moved = self.piece_at(from).unwrap();

            // Zobrist: 移動先の捕獲駒を除去
            let captured = self.remove_piece(mv.to);
            if let Some(cap) = captured {
                let cap_ci = color_zobrist_index(cap.color);
                let cap_ki = piece_kind_zobrist_index(cap.kind);
                self.zobrist_hash ^= ZOBRIST.piece[cap_ci][cap_ki][mv.to.index()];
            }

            // Zobrist: 移動元の駒を除去
            self.remove_piece(from);
            let moved_ki = piece_kind_zobrist_index(moved.kind);
            self.zobrist_hash ^= ZOBRIST.piece[ci][moved_ki][from.index()];

            // 駒を取った場合、持ち駒に加える（玉は持ち駒にできない）
            if let Some(cap) = captured {
                if cap.kind != PieceKind::King {
                    let unpromoted = cap.kind.unpromoted();
                    let old_count = self.hand(color).count(unpromoted) as usize;
                    self.update_hand_zobrist(color, unpromoted, old_count, old_count + 1);
                    self.hand_mut(color).add(cap.kind);
                }
            }

            // 成り
            let new_kind = if mv.promotion {
                moved.kind.promoted().unwrap_or(moved.kind)
            } else {
                moved.kind
            };

            // Zobrist: 移動先に配置
            let new_ki = piece_kind_zobrist_index(new_kind);
            self.zobrist_hash ^= ZOBRIST.piece[ci][new_ki][mv.to.index()];
            self.set_piece(mv.to, Piece::new(color, new_kind));

            // Zobrist: 手番変更
            self.zobrist_hash ^= ZOBRIST.color;
            self.side_to_move = color.opponent();

            UndoInfo { mv, captured, moved_piece: moved }
        }
    }

    /// 指し手の取消
    pub fn unmake_move(&mut self, undo: &UndoInfo) {
        // Zobrist: 手番変更（取消）
        self.zobrist_hash ^= ZOBRIST.color;

        self.side_to_move = self.side_to_move.opponent();
        let color = self.side_to_move;
        let ci = color_zobrist_index(color);
        let mv = undo.mv;

        if mv.drop_piece.is_some() {
            // 打ちの取消
            let kind = mv.drop_piece.unwrap();

            // Zobrist: 盤面から除去
            let ki = piece_kind_zobrist_index(kind);
            self.zobrist_hash ^= ZOBRIST.piece[ci][ki][mv.to.index()];
            self.remove_piece(mv.to);

            // Zobrist: 持ち駒に追加
            let old_count = self.hand(color).count(kind) as usize;
            self.update_hand_zobrist(color, kind, old_count, old_count + 1);
            self.hand_mut(color).add(kind);
        } else {
            // 移動の取消
            let from = mv.from.unwrap();

            // Zobrist: 移動先の駒を除去（成り後の駒）
            let piece_at_to = self.piece_at(mv.to).unwrap();
            let ki_at_to = piece_kind_zobrist_index(piece_at_to.kind);
            self.zobrist_hash ^= ZOBRIST.piece[ci][ki_at_to][mv.to.index()];
            self.remove_piece(mv.to);

            // Zobrist: 移動元に元の駒を復元
            let moved_ki = piece_kind_zobrist_index(undo.moved_piece.kind);
            self.zobrist_hash ^= ZOBRIST.piece[ci][moved_ki][from.index()];
            self.set_piece(from, undo.moved_piece);

            // 取った駒を元に戻す
            if let Some(cap) = undo.captured {
                // Zobrist: 捕獲駒を盤面に復元
                let cap_ci = color_zobrist_index(cap.color);
                let cap_ki = piece_kind_zobrist_index(cap.kind);
                self.zobrist_hash ^= ZOBRIST.piece[cap_ci][cap_ki][mv.to.index()];
                self.set_piece(mv.to, cap);

                if cap.kind != PieceKind::King {
                    // Zobrist: 持ち駒から除去
                    let unpromoted = cap.kind.unpromoted();
                    let cur_count = self.hand(color).count(unpromoted) as usize;
                    self.update_hand_zobrist(color, unpromoted, cur_count, cur_count - 1);
                    self.hand_mut(color).remove(cap.kind);
                }
            }
        }
    }

    /// 指定した色の王が王手されているか
    pub fn is_in_check(&self, color: Color) -> bool {
        if let Some(king_sq) = self.find_king(color) {
            self.is_attacked(king_sq, color.opponent())
        } else {
            false
        }
    }

    /// 指定したマスが指定した色の駒に攻撃されているか
    pub fn is_attacked(&self, sq: Square, by_color: Color) -> bool {
        movegen::is_square_attacked(self, sq, by_color)
    }

    /// 合法手を生成
    pub fn generate_legal_moves(&self) -> Vec<Move> {
        let color = self.side_to_move;
        let pseudo_moves = movegen::generate_pseudo_legal_moves(self, color);

        // 自玉がなければ全擬似合法手が合法（詰将棋の先手に該当）
        if self.find_king(color).is_none() {
            return pseudo_moves;
        }

        let mut legal_moves = Vec::new();

        let mut test_pos = self.clone();
        for mv in pseudo_moves {
            let undo = test_pos.make_move(mv);
            // 自玉が取られる手は除外
            if !test_pos.is_in_check(color) {
                legal_moves.push(mv);
            }
            test_pos.unmake_move(&undo);
        }

        legal_moves
    }

    /// 王手回避手を生成（王手されている局面専用、高効率版）
    /// 持ち駒が多い局面では generate_legal_moves() よりも大幅に高速
    pub fn generate_check_evasions(&self) -> Vec<Move> {
        movegen::generate_check_evasions(self)
    }

    /// 王手になる手のみ生成（攻め方用）
    pub fn generate_check_moves(&self) -> Vec<Move> {
        let color = self.side_to_move;
        let legal = self.generate_legal_moves();
        let opponent = color.opponent();

        let mut test_pos = self.clone();
        legal
            .into_iter()
            .filter(|mv| {
                let undo = test_pos.make_move(*mv);
                let is_check = test_pos.is_in_check(opponent);
                test_pos.unmake_move(&undo);
                is_check
            })
            .collect()
    }

    /// 詰んでいるか（手番側に合法手がなく王手されている）
    pub fn is_checkmate(&self) -> bool {
        let color = self.side_to_move;
        self.is_in_check(color) && self.generate_check_evasions().is_empty()
    }

    /// sente_hand を除外したハッシュ（優越関係の判定用）
    /// gote_hand は含む（衝立詰将棋では観測構造が gote_hand に依存するため）
    pub fn hash_without_sente_hand(&self) -> u64 {
        self.zobrist_hash ^ self.zobrist_sente_hand
    }

    /// 盤面と手番のみのハッシュ（持ち駒を全て除外）
    /// 手順ヒント用: 盤面配置が同じなら証明手順が同じ可能性が高い
    pub fn hash_board_only(&self) -> u64 {
        self.zobrist_hash ^ self.zobrist_sente_hand ^ self.zobrist_gote_hand
    }

    /// Zobrist ハッシュをスクラッチから計算して設定する
    /// Position のセットアップ完了後に一度呼び出す
    pub fn init_zobrist_hash(&mut self) {
        let mut board_hash = 0u64;
        for idx in 0..81 {
            if let Some(piece) = self.board[idx] {
                let ci = color_zobrist_index(piece.color);
                let ki = piece_kind_zobrist_index(piece.kind);
                board_hash ^= ZOBRIST.piece[ci][ki][idx];
            }
        }

        let mut sente_hand_hash = 0u64;
        let mut gote_hand_hash = 0u64;
        const HAND_KINDS: [PieceKind; 7] = [
            PieceKind::Rook, PieceKind::Bishop, PieceKind::Gold,
            PieceKind::Silver, PieceKind::Knight, PieceKind::Lance, PieceKind::Pawn,
        ];
        for &kind in &HAND_KINDS {
            let hi = hand_kind_zobrist_index(kind);
            sente_hand_hash ^= ZOBRIST.hand[0][hi][self.sente_hand.count(kind) as usize];
            gote_hand_hash ^= ZOBRIST.hand[1][hi][self.gote_hand.count(kind) as usize];
        }

        let mut hash = board_hash ^ sente_hand_hash ^ gote_hand_hash;
        if self.side_to_move == Color::Gote {
            hash ^= ZOBRIST.color;
        }

        self.zobrist_hash = hash;
        self.zobrist_sente_hand = sente_hand_hash;
        self.zobrist_gote_hand = gote_hand_hash;
    }

    /// 玉が逃げられるマスの集合を返す（8方向チェック: 盤内・自駒なし・相手の利きなし）
    /// X-ray 効果は無視するが、前後比較では同じバイアスなので問題なし
    fn king_escape_squares(&self, king_sq: Square, defender_color: Color) -> HashSet<Square> {
        let attacker_color = defender_color.opponent();
        let mut escapes = HashSet::new();
        for &(df, dr) in &[
            (-1i8, -1i8), (0, -1), (1, -1),
            (-1, 0), (1, 0),
            (-1, 1), (0, 1), (1, 1),
        ] {
            let nf = king_sq.file as i8 + df;
            let nr = king_sq.rank as i8 + dr;
            if !Square::is_valid(nf, nr) {
                continue;
            }
            let sq = Square::new(nf as u8, nr as u8);
            // 味方の駒がいたら移動不可
            if let Some(piece) = self.piece_at(sq) {
                if piece.color == defender_color {
                    continue;
                }
            }
            // 相手の利きがあれば移動不可
            if self.is_attacked(sq, attacker_color) {
                continue;
            }
            escapes.insert(sq);
        }
        escapes
    }

    /// slider_from → capture_sq → king_sq が一直線上にあり、
    /// 駒種が該当方向のスライダーかどうかを判定する。
    /// 取り返しがスライダーの王手ラインに沿った移動であることの確認用。
    fn is_slider_line_recapture(
        &self,
        cap_from: Square,
        capture_sq: Square,
        king_sq: Square,
        attacker_color: Color,
    ) -> bool {
        let piece = match self.piece_at(cap_from) {
            Some(p) => p,
            None => return false,
        };

        // capture_sq → king_sq の方向
        let df_ck = (king_sq.file as i8 - capture_sq.file as i8).signum();
        let dr_ck = (king_sq.rank as i8 - capture_sq.rank as i8).signum();

        // slider_from → capture_sq の方向
        let df_sc = (capture_sq.file as i8 - cap_from.file as i8).signum();
        let dr_sc = (capture_sq.rank as i8 - cap_from.rank as i8).signum();

        // 同じ直線上でなければ不適格
        if df_ck != df_sc || dr_ck != dr_sc {
            return false;
        }

        // slider_from → capture_sq が実際に直線移動か確認（斜めまたは十字）
        let abs_df = (capture_sq.file as i8 - cap_from.file as i8).abs();
        let abs_dr = (capture_sq.rank as i8 - cap_from.rank as i8).abs();
        if abs_df != abs_dr && abs_df != 0 && abs_dr != 0 {
            return false; // 直線でも斜めでもない
        }

        let is_orthogonal = df_ck == 0 || dr_ck == 0;

        match piece.kind {
            PieceKind::Rook | PieceKind::PromotedRook => is_orthogonal,
            PieceKind::Bishop | PieceKind::PromotedBishop => !is_orthogonal,
            PieceKind::Lance => {
                if !is_orthogonal || df_ck != 0 {
                    return false; // 香車は縦方向のみ
                }
                // 先手香: rank 減少方向 (dr < 0)、後手香: rank 増加方向 (dr > 0)
                (attacker_color == Color::Sente && dr_sc < 0)
                    || (attacker_color == Color::Gote && dr_sc > 0)
            }
            _ => false,
        }
    }

    /// 無駄合い判定（詰将棋ルール）
    /// 王手に対する合駒（玉以外の応手）が無駄かどうかを判定する。
    /// 合駒を取り返して再び王手＋詰みになる場合、無駄合いとみなす。
    /// 再帰的に判定し、連続合駒も正しく処理する。
    pub fn is_futile_interposition(&self, def_mv: &Move) -> bool {
        self.is_futile_interposition_impl(def_mv, 3)
    }

    fn is_futile_interposition_impl(&self, def_mv: &Move, remaining_depth: u32) -> bool {
        if remaining_depth == 0 {
            return false;
        }

        // 玉の移動は合駒ではない
        if let Some(from) = def_mv.from {
            if let Some(piece) = self.piece_at(from) {
                if piece.kind == PieceKind::King {
                    return false;
                }
            }
        }

        let mut pos_after = self.clone();
        pos_after.make_move(*def_mv);

        let capture_sq = def_mv.to;
        let attacker_color = pos_after.side_to_move;

        // 高速プリチェック: 攻め方の駒がそのマスに利いていなければ無駄合いではない
        if !pos_after.is_attacked(capture_sq, attacker_color) {
            return false;
        }

        // 全合法手の代わりに擬似合法手を生成し、取り返しマスのみフィルタして
        // 合法性を個別チェック（全合法手生成より大幅に高速）
        let pseudo_moves = movegen::generate_pseudo_legal_moves(&pos_after, attacker_color);

        for cap_mv in &pseudo_moves {
            if cap_mv.to != capture_sq {
                continue;
            }

            let undo_cap = pos_after.make_move(*cap_mv);

            // 合法性チェック: 自玉が王手されていないか
            if pos_after.is_in_check(attacker_color) {
                pos_after.unmake_move(&undo_cap);
                continue;
            }

            // 取り返した後に王手でなければ無駄合いではない
            let defender_color = pos_after.side_to_move;
            if !pos_after.is_in_check(defender_color) {
                pos_after.unmake_move(&undo_cap);
                continue;
            }

            // 取り返し後に即詰みなら無駄合い
            if pos_after.is_checkmate() {
                return true;
            }

            // スライダー筋の無駄合い判定（打ち合駒のみ）:
            // 条件A: 取り返しが王手ラインに沿ったスライダー移動
            // 条件B: 取り返し後も王手（上で検証済み）
            // 条件C: 取り返し後の玉の逃げ場 ⊆ 合駒前の玉の逃げ場
            // 条件D: 後手が新位置のスライダーを取れない
            // 全条件満たせば合駒は玉方にとって厳密に損（逃げ場不変＋先手駒得）
            if def_mv.from.is_none() {
                if let Some(cap_from) = cap_mv.from {
                    if let Some(king_sq) = pos_after.find_king(defender_color) {
                        // 条件A: スライダーラインに沿った取り返しか
                        if self.is_slider_line_recapture(
                            cap_from, capture_sq, king_sq, attacker_color,
                        ) {
                            // 条件D: 後手がスライダーの新位置を取れないか
                            if !pos_after.is_attacked(capture_sq, defender_color) {
                                // 条件C: 逃げ場が減っていないか
                                let escapes_before =
                                    self.king_escape_squares(king_sq, defender_color);
                                let escapes_after =
                                    pos_after.king_escape_squares(king_sq, defender_color);
                                if escapes_after.is_subset(&escapes_before) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }

            // 取り返し後の全応手を生成
            let responses = pos_after.generate_legal_moves();

            // 広義の無駄合い判定（打ち合駒の場合）:
            // 取り返しが盤上の駒による手で、後手がそのマスに取り返せず、
            // かつ取り返しが味方スライダーの利きを遮断しない場合は無駄合い
            if def_mv.from.is_none() {
                if let Some(cap_from) = cap_mv.from {
                    let defender_can_recapture =
                        responses.iter().any(|m| m.to == capture_sq);
                    if !defender_can_recapture
                        && !self.recapture_blocks_friendly_slider(
                            cap_from,
                            capture_sq,
                            attacker_color,
                        )
                    {
                        return true;
                    }
                }
            }

            // 取り返し後の全応手も無駄合いなら、元の合駒も無駄合い
            let all_futile = !responses.is_empty()
                && responses.iter().all(|dm| {
                    pos_after.is_futile_interposition_impl(dm, remaining_depth - 1)
                });

            if all_futile {
                return true;
            }

            pos_after.unmake_move(&undo_cap);
        }

        false
    }

    /// 攻め方の盤上の駒が from_sq から to_sq に移動した場合に、
    /// 味方の飛び駒（飛/竜/角/馬/香）の利きを遮断するかどうかを判定する。
    /// ただし、from_sq がスライダーと to_sq の間にある場合は
    /// 既に遮断していたため false を返す。
    fn recapture_blocks_friendly_slider(
        &self,
        from_sq: Square,
        to_sq: Square,
        attacker_color: Color,
    ) -> bool {
        // 十字方向（飛車/竜 + 香車）
        for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
            if self.check_slider_line_blocked(from_sq, to_sq, df, dr, attacker_color, true) {
                return true;
            }
        }
        // 斜め方向（角/馬）
        for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
            if self.check_slider_line_blocked(from_sq, to_sq, df, dr, attacker_color, false) {
                return true;
            }
        }
        false
    }

    /// to_sq から (df, dr) 方向にスキャンして味方スライダーを探す。
    /// from_sq は移動予定の駒なので空きマスとして扱う。
    /// スライダーが見つかり、かつ from_sq がスライダーと to_sq の間に
    /// なければ true（遮断発生）を返す。
    fn check_slider_line_blocked(
        &self,
        from_sq: Square,
        to_sq: Square,
        df: i8,
        dr: i8,
        attacker_color: Color,
        is_orthogonal: bool,
    ) -> bool {
        let slider_kinds: &[PieceKind] = if is_orthogonal {
            &[PieceKind::Rook, PieceKind::PromotedRook]
        } else {
            &[PieceKind::Bishop, PieceKind::PromotedBishop]
        };

        let mut f = to_sq.file as i8 + df;
        let mut r = to_sq.rank as i8 + dr;
        let mut from_sq_on_line = false;

        while Square::is_valid(f, r) {
            let sq = Square::new(f as u8, r as u8);

            if sq == from_sq {
                // from_sq は移動予定 → 空きマスとして扱いスキップ
                from_sq_on_line = true;
                f += df;
                r += dr;
                continue;
            }

            if let Some(piece) = self.piece_at(sq) {
                if piece.color == attacker_color {
                    // 味方の飛び駒かチェック
                    if slider_kinds.contains(&piece.kind) {
                        // from_sq が間にあれば既に遮断済み → 新たな遮断なし
                        return !from_sq_on_line;
                    }
                    // 香車の特殊判定（十字方向のうち特定方向のみ）
                    if is_orthogonal
                        && piece.kind == PieceKind::Lance
                        && df == 0
                    {
                        // 先手香は (0,-1) 方向に攻撃 → (0,1) スキャンで見つかる
                        // 後手香は (0,1) 方向に攻撃 → (0,-1) スキャンで見つかる
                        let lance_matches =
                            (attacker_color == Color::Sente && dr == 1)
                                || (attacker_color == Color::Gote && dr == -1);
                        if lance_matches {
                            return !from_sq_on_line;
                        }
                    }
                }
                // 別の駒で遮断されている → このライン上の問題なし
                return false;
            }

            f += df;
            r += dr;
        }

        false
    }

    /// 同じ筋に同じ色の歩があるか（二歩判定用）
    pub fn has_pawn_on_file(&self, color: Color, file: u8) -> bool {
        for rank in 1..=9 {
            if let Some(piece) = self.piece_at(Square::new(file, rank)) {
                if piece.color == color && piece.kind == PieceKind::Pawn {
                    return true;
                }
            }
        }
        false
    }
}

impl Default for Position {
    fn default() -> Self {
        Self::new()
    }
}
