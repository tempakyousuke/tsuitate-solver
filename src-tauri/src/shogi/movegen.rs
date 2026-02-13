use super::position::Position;
use super::types::*;

/// 擬似合法手を生成（自玉の安全は確認しない）
pub fn generate_pseudo_legal_moves(pos: &Position, color: Color) -> Vec<Move> {
    let mut moves = Vec::new();

    // 盤上の駒の移動
    for idx in 0..81 {
        let sq = Square::from_index(idx);
        if let Some(piece) = pos.piece_at(sq) {
            if piece.color == color {
                generate_piece_moves(pos, sq, piece, &mut moves);
            }
        }
    }

    // 持ち駒を打つ
    generate_drop_moves(pos, color, &mut moves);

    moves
}

/// 駒の移動手を生成
fn generate_piece_moves(pos: &Position, from: Square, piece: Piece, moves: &mut Vec<Move>) {
    let color = piece.color;
    match piece.kind {
        PieceKind::King => generate_step_moves(pos, from, color, &king_offsets(), false, moves),
        PieceKind::Gold | PieceKind::PromotedSilver | PieceKind::PromotedKnight
        | PieceKind::PromotedLance | PieceKind::PromotedPawn => {
            generate_step_moves(pos, from, color, &gold_offsets(color), false, moves)
        }
        PieceKind::Silver => {
            generate_step_moves(pos, from, color, &silver_offsets(color), true, moves)
        }
        PieceKind::Knight => {
            generate_step_moves(pos, from, color, &knight_offsets(color), true, moves)
        }
        PieceKind::Pawn => {
            generate_step_moves(pos, from, color, &pawn_offsets(color), true, moves)
        }
        PieceKind::Lance => generate_lance_moves(pos, from, color, moves),
        PieceKind::Rook => generate_rook_moves(pos, from, color, false, moves),
        PieceKind::Bishop => generate_bishop_moves(pos, from, color, false, moves),
        PieceKind::PromotedRook => generate_rook_moves(pos, from, color, true, moves),
        PieceKind::PromotedBishop => generate_bishop_moves(pos, from, color, true, moves),
    }
}

/// ステップ移動（1マス移動）の手を生成
fn generate_step_moves(
    pos: &Position,
    from: Square,
    color: Color,
    offsets: &[(i8, i8)],
    can_promote: bool,
    moves: &mut Vec<Move>,
) {
    for &(df, dr) in offsets {
        let nf = from.file as i8 + df;
        let nr = from.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let to = Square::new(nf as u8, nr as u8);
        if let Some(target) = pos.piece_at(to) {
            if target.color == color {
                continue; // 自駒がある
            }
        }
        add_move_with_promotion(from, to, color, can_promote, pos, moves);
    }
}

/// 香車の移動
fn generate_lance_moves(pos: &Position, from: Square, color: Color, moves: &mut Vec<Move>) {
    let dr: i8 = if color == Color::Sente { -1 } else { 1 };
    let mut rank = from.rank as i8 + dr;
    while (1..=9).contains(&rank) {
        let to = Square::new(from.file, rank as u8);
        if let Some(target) = pos.piece_at(to) {
            if target.color != color {
                add_move_with_promotion(from, to, color, true, pos, moves);
            }
            break;
        }
        add_move_with_promotion(from, to, color, true, pos, moves);
        rank += dr;
    }
}

/// 飛車（+竜）の移動
fn generate_rook_moves(
    pos: &Position,
    from: Square,
    color: Color,
    promoted: bool,
    moves: &mut Vec<Move>,
) {
    // 十字方向にスライド
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        generate_slide_moves(pos, from, color, df, dr, !promoted, moves);
    }
    // 竜は斜め1マスも移動可能
    if promoted {
        for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
            let nf = from.file as i8 + df;
            let nr = from.rank as i8 + dr;
            if !Square::is_valid(nf, nr) {
                continue;
            }
            let to = Square::new(nf as u8, nr as u8);
            if let Some(target) = pos.piece_at(to) {
                if target.color == color {
                    continue;
                }
            }
            moves.push(Move::normal(from, to, false));
        }
    }
}

/// 角（+馬）の移動
fn generate_bishop_moves(
    pos: &Position,
    from: Square,
    color: Color,
    promoted: bool,
    moves: &mut Vec<Move>,
) {
    // 斜め方向にスライド
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        generate_slide_moves(pos, from, color, df, dr, !promoted, moves);
    }
    // 馬は十字1マスも移動可能
    if promoted {
        for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
            let nf = from.file as i8 + df;
            let nr = from.rank as i8 + dr;
            if !Square::is_valid(nf, nr) {
                continue;
            }
            let to = Square::new(nf as u8, nr as u8);
            if let Some(target) = pos.piece_at(to) {
                if target.color == color {
                    continue;
                }
            }
            moves.push(Move::normal(from, to, false));
        }
    }
}

/// スライド移動（飛び駒用）
fn generate_slide_moves(
    pos: &Position,
    from: Square,
    color: Color,
    df: i8,
    dr: i8,
    can_promote: bool,
    moves: &mut Vec<Move>,
) {
    let mut f = from.file as i8 + df;
    let mut r = from.rank as i8 + dr;
    while Square::is_valid(f, r) {
        let to = Square::new(f as u8, r as u8);
        if let Some(target) = pos.piece_at(to) {
            if target.color != color {
                add_move_with_promotion(from, to, color, can_promote, pos, moves);
            }
            break;
        }
        add_move_with_promotion(from, to, color, can_promote, pos, moves);
        f += df;
        r += dr;
    }
}

/// 成り/不成の判定を含めて手を追加
fn add_move_with_promotion(
    from: Square,
    to: Square,
    color: Color,
    can_promote: bool,
    pos: &Position,
    moves: &mut Vec<Move>,
) {
    let piece = pos.piece_at(from).unwrap();
    let promo_zone_start = if color == Color::Sente { 1 } else { 7 };
    let promo_zone_end = if color == Color::Sente { 3 } else { 9 };
    let in_promo_zone =
        (from.rank >= promo_zone_start && from.rank <= promo_zone_end)
            || (to.rank >= promo_zone_start && to.rank <= promo_zone_end);

    if can_promote && piece.kind.can_promote() && in_promo_zone {
        // 成る手
        moves.push(Move::normal(from, to, true));

        // 不成が合法な場合のみ追加
        if !must_promote(piece.kind, to, color) {
            moves.push(Move::normal(from, to, false));
        }
    } else {
        moves.push(Move::normal(from, to, false));
    }
}

/// 成りが強制かどうか（行き所のない駒の判定）
fn must_promote(kind: PieceKind, to: Square, color: Color) -> bool {
    match kind {
        PieceKind::Pawn | PieceKind::Lance => {
            if color == Color::Sente {
                to.rank == 1
            } else {
                to.rank == 9
            }
        }
        PieceKind::Knight => {
            if color == Color::Sente {
                to.rank <= 2
            } else {
                to.rank >= 8
            }
        }
        _ => false,
    }
}

/// 持ち駒を打つ手を生成
fn generate_drop_moves(pos: &Position, color: Color, moves: &mut Vec<Move>) {
    let hand = pos.hand(color);

    for &kind in &PieceKind::HAND_PIECES {
        if !hand.has(kind) {
            continue;
        }

        for idx in 0..81 {
            let sq = Square::from_index(idx);
            if pos.piece_at(sq).is_some() {
                continue; // 駒がある
            }

            // 行き所のない駒は打てない
            match kind {
                PieceKind::Pawn => {
                    if (color == Color::Sente && sq.rank == 1)
                        || (color == Color::Gote && sq.rank == 9)
                    {
                        continue;
                    }
                    // 二歩チェック
                    if pos.has_pawn_on_file(color, sq.file) {
                        continue;
                    }
                    // 打ち歩詰めチェック
                    if is_pawn_drop_mate(pos, sq, color) {
                        continue;
                    }
                }
                PieceKind::Lance => {
                    if (color == Color::Sente && sq.rank == 1)
                        || (color == Color::Gote && sq.rank == 9)
                    {
                        continue;
                    }
                }
                PieceKind::Knight => {
                    if (color == Color::Sente && sq.rank <= 2)
                        || (color == Color::Gote && sq.rank >= 8)
                    {
                        continue;
                    }
                }
                _ => {}
            }

            moves.push(Move::drop(sq, kind));
        }
    }
}

/// 打ち歩詰め判定
fn is_pawn_drop_mate(pos: &Position, sq: Square, color: Color) -> bool {
    let mut test_pos = pos.clone();
    test_pos.set_piece(sq, Piece::new(color, PieceKind::Pawn));
    test_pos.side_to_move = color.opponent();

    // 相手玉に王手がかかっていて、合法手がないなら打ち歩詰め
    test_pos.is_in_check(color.opponent()) && test_pos.generate_legal_moves().is_empty()
}

/// マスが特定の色の駒に攻撃されているか
pub fn is_square_attacked(pos: &Position, sq: Square, by_color: Color) -> bool {
    // 各駒種からの攻撃をチェック

    // 玉からの攻撃
    for &(df, dr) in &king_offsets() {
        if check_attacker(pos, sq, df, dr, by_color, PieceKind::King) {
            return true;
        }
    }

    // 金（+成駒）からの攻撃
    // Note: 非対称駒は逆方向にスキャンする（攻撃者の移動方向の逆 = 相手側のオフセット）
    for &(df, dr) in &gold_offsets(by_color.opponent()) {
        if let Some(attacker) = get_piece_at_offset(pos, sq, df, dr) {
            if attacker.color == by_color
                && matches!(
                    attacker.kind,
                    PieceKind::Gold
                        | PieceKind::PromotedSilver
                        | PieceKind::PromotedKnight
                        | PieceKind::PromotedLance
                        | PieceKind::PromotedPawn
                )
            {
                return true;
            }
        }
    }

    // 銀からの攻撃
    for &(df, dr) in &silver_offsets(by_color.opponent()) {
        if check_attacker(pos, sq, df, dr, by_color, PieceKind::Silver) {
            return true;
        }
    }

    // 桂からの攻撃
    for &(df, dr) in &knight_offsets(by_color.opponent()) {
        if check_attacker(pos, sq, df, dr, by_color, PieceKind::Knight) {
            return true;
        }
    }

    // 歩からの攻撃
    for &(df, dr) in &pawn_offsets(by_color.opponent()) {
        if check_attacker(pos, sq, df, dr, by_color, PieceKind::Pawn) {
            return true;
        }
    }

    // 香からの攻撃（縦方向スライド - 逆方向にスキャン）
    let lance_dr: i8 = if by_color == Color::Sente { 1 } else { -1 };
    if check_slide_attacker(
        pos,
        sq,
        0,
        lance_dr,
        by_color,
        &[PieceKind::Lance],
    ) {
        return true;
    }

    // 飛車/竜からの攻撃（十字方向）
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        if check_slide_attacker(
            pos,
            sq,
            df,
            dr,
            by_color,
            &[PieceKind::Rook, PieceKind::PromotedRook],
        ) {
            return true;
        }
    }

    // 角/馬からの攻撃（斜め方向）
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        if check_slide_attacker(
            pos,
            sq,
            df,
            dr,
            by_color,
            &[PieceKind::Bishop, PieceKind::PromotedBishop],
        ) {
            return true;
        }
    }

    // 竜の斜め1マス攻撃
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        if check_attacker(pos, sq, df, dr, by_color, PieceKind::PromotedRook) {
            return true;
        }
    }

    // 馬の十字1マス攻撃
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        if check_attacker(pos, sq, df, dr, by_color, PieceKind::PromotedBishop) {
            return true;
        }
    }

    false
}

/// オフセット位置に特定の駒があるかチェック
fn check_attacker(
    pos: &Position,
    sq: Square,
    df: i8,
    dr: i8,
    color: Color,
    kind: PieceKind,
) -> bool {
    if let Some(piece) = get_piece_at_offset(pos, sq, df, dr) {
        piece.color == color && piece.kind == kind
    } else {
        false
    }
}

/// オフセット位置の駒を取得
fn get_piece_at_offset(pos: &Position, sq: Square, df: i8, dr: i8) -> Option<Piece> {
    let nf = sq.file as i8 + df;
    let nr = sq.rank as i8 + dr;
    if Square::is_valid(nf, nr) {
        pos.piece_at(Square::new(nf as u8, nr as u8))
    } else {
        None
    }
}

/// スライド方向に特定の駒があるかチェック
fn check_slide_attacker(
    pos: &Position,
    sq: Square,
    df: i8,
    dr: i8,
    color: Color,
    kinds: &[PieceKind],
) -> bool {
    let mut f = sq.file as i8 + df;
    let mut r = sq.rank as i8 + dr;
    while Square::is_valid(f, r) {
        let check_sq = Square::new(f as u8, r as u8);
        if let Some(piece) = pos.piece_at(check_sq) {
            return piece.color == color && kinds.contains(&piece.kind);
        }
        f += df;
        r += dr;
    }
    false
}

// 各駒の移動オフセット

fn king_offsets() -> [(i8, i8); 8] {
    [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ]
}

fn gold_offsets(color: Color) -> [(i8, i8); 6] {
    if color == Color::Sente {
        [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (0, 1)]
    } else {
        [(-1, 1), (0, 1), (1, 1), (-1, 0), (1, 0), (0, -1)]
    }
}

fn silver_offsets(color: Color) -> [(i8, i8); 5] {
    if color == Color::Sente {
        [(-1, -1), (0, -1), (1, -1), (-1, 1), (1, 1)]
    } else {
        [(-1, 1), (0, 1), (1, 1), (-1, -1), (1, -1)]
    }
}

fn knight_offsets(color: Color) -> [(i8, i8); 2] {
    if color == Color::Sente {
        [(-1, -2), (1, -2)]
    } else {
        [(-1, 2), (1, 2)]
    }
}

fn pawn_offsets(color: Color) -> [(i8, i8); 1] {
    if color == Color::Sente {
        [(0, -1)]
    } else {
        [(0, 1)]
    }
}
