use super::movegen;
use super::types::*;

/// 盤面状態
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Position {
    /// 9x9の盤面 (board[file-1][rank-1])
    board: [Option<Piece>; 81],
    /// 先手の持ち駒
    pub sente_hand: HandPieces,
    /// 後手の持ち駒
    pub gote_hand: HandPieces,
    /// 手番
    pub side_to_move: Color,
}

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
        Self {
            board: [None; 81],
            sente_hand: HandPieces::new(),
            gote_hand: HandPieces::new(),
            side_to_move: Color::Sente,
        }
    }

    /// マス目の駒を取得
    pub fn piece_at(&self, sq: Square) -> Option<Piece> {
        self.board[sq.index()]
    }

    /// 駒を配置
    pub fn set_piece(&mut self, sq: Square, piece: Piece) {
        self.board[sq.index()] = Some(piece);
    }

    /// 駒を除去
    pub fn remove_piece(&mut self, sq: Square) -> Option<Piece> {
        let piece = self.board[sq.index()];
        self.board[sq.index()] = None;
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
        for idx in 0..81 {
            if let Some(piece) = self.board[idx] {
                if piece.color == color && piece.kind == PieceKind::King {
                    return Some(Square::from_index(idx));
                }
            }
        }
        None
    }

    /// 指し手の実行
    pub fn make_move(&mut self, mv: Move) -> UndoInfo {
        let color = self.side_to_move;

        if let Some(drop_kind) = mv.drop_piece {
            // 駒を打つ
            let piece = Piece::new(color, drop_kind);
            self.hand_mut(color).remove(drop_kind);
            self.set_piece(mv.to, piece);
            let undo = UndoInfo {
                mv,
                captured: None,
                moved_piece: piece,
            };
            self.side_to_move = color.opponent();
            undo
        } else {
            // 盤上の駒を移動
            let from = mv.from.unwrap();
            let moved = self.piece_at(from).unwrap();
            let captured = self.remove_piece(mv.to);
            self.remove_piece(from);

            // 駒を取った場合、持ち駒に加える（玉は持ち駒にできない）
            if let Some(cap) = captured {
                if cap.kind != PieceKind::King {
                    self.hand_mut(color).add(cap.kind);
                }
            }

            // 成り
            let new_kind = if mv.promotion {
                moved.kind.promoted().unwrap_or(moved.kind)
            } else {
                moved.kind
            };
            self.set_piece(mv.to, Piece::new(color, new_kind));

            let undo = UndoInfo {
                mv,
                captured,
                moved_piece: moved,
            };
            self.side_to_move = color.opponent();
            undo
        }
    }

    /// 指し手の取消
    pub fn unmake_move(&mut self, undo: &UndoInfo) {
        self.side_to_move = self.side_to_move.opponent();
        let color = self.side_to_move;
        let mv = undo.mv;

        if mv.drop_piece.is_some() {
            // 打ちの取消
            let kind = mv.drop_piece.unwrap();
            self.remove_piece(mv.to);
            self.hand_mut(color).add(kind);
        } else {
            // 移動の取消
            let from = mv.from.unwrap();
            self.remove_piece(mv.to);
            self.set_piece(from, undo.moved_piece);

            // 取った駒を元に戻す
            if let Some(cap) = undo.captured {
                self.set_piece(mv.to, cap);
                if cap.kind != PieceKind::King {
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
        let mut legal_moves = Vec::new();

        for mv in pseudo_moves {
            let mut pos = self.clone();
            pos.make_move(mv);
            // 自玉が取られる手は除外
            if !pos.is_in_check(color) {
                legal_moves.push(mv);
            }
        }

        legal_moves
    }

    /// 王手になる手のみ生成（攻め方用）
    pub fn generate_check_moves(&self) -> Vec<Move> {
        let color = self.side_to_move;
        let legal = self.generate_legal_moves();
        let opponent = color.opponent();

        legal
            .into_iter()
            .filter(|mv| {
                let mut pos = self.clone();
                pos.make_move(*mv);
                pos.is_in_check(opponent)
            })
            .collect()
    }

    /// 詰んでいるか（手番側に合法手がなく王手されている）
    pub fn is_checkmate(&self) -> bool {
        let color = self.side_to_move;
        self.is_in_check(color) && self.generate_legal_moves().is_empty()
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
