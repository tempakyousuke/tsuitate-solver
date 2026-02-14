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
            moves.push(Move::normal(from, to, false, PieceKind::PromotedRook));
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
            moves.push(Move::normal(from, to, false, PieceKind::PromotedBishop));
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
        moves.push(Move::normal(from, to, true, piece.kind));

        // 不成が合法な場合のみ追加
        if !must_promote(piece.kind, to, color) {
            moves.push(Move::normal(from, to, false, piece.kind));
        }
    } else {
        moves.push(Move::normal(from, to, false, piece.kind));
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

// 打ち歩詰め判定の再帰ガード
// is_pawn_drop_mate → generate_legal_moves → generate_drop_moves → is_pawn_drop_mate の
// 相互再帰を防止するため、スレッドローカルフラグで既にチェック中かを管理する。
// ネストした呼び出しでは打ち歩詰めチェックをスキップし、基本合法性のみ判定する。
thread_local! {
    static IN_PAWN_DROP_MATE_CHECK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 打ち歩詰め判定
fn is_pawn_drop_mate(pos: &Position, sq: Square, color: Color) -> bool {
    // 既に打ち歩詰めチェック中なら再帰しない
    if IN_PAWN_DROP_MATE_CHECK.with(|f| f.get()) {
        return false;
    }

    let mut test_pos = pos.clone();
    test_pos.set_piece(sq, Piece::new(color, PieceKind::Pawn));
    test_pos.side_to_move = color.opponent();

    if !test_pos.is_in_check(color.opponent()) {
        return false;
    }

    // ネストした generate_legal_moves では打ち歩詰めチェックをスキップ
    IN_PAWN_DROP_MATE_CHECK.with(|f| f.set(true));
    let result = test_pos.generate_legal_moves().is_empty();
    IN_PAWN_DROP_MATE_CHECK.with(|f| f.set(false));

    result
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

/// 王手回避手を生成（王手されている局面専用の高効率合法手生成）
/// 通常の generate_legal_moves() の代わりに使用する。
/// 王手に対する合法応手のみを生成するため、持ち駒が多い局面で大幅に高速。
pub fn generate_check_evasions(pos: &Position) -> Vec<Move> {
    let color = pos.side_to_move;
    let king_sq = match pos.find_king(color) {
        Some(sq) => sq,
        None => return Vec::new(),
    };
    let opponent = color.opponent();

    let checkers = find_checkers(pos, king_sq, opponent);
    if checkers.is_empty() {
        return pos.generate_legal_moves();
    }

    let mut evasions = Vec::new();
    let mut test = pos.clone();

    // 1. 玉の移動（常に候補）
    for &(df, dr) in &king_offsets() {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let to = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(to) {
            if p.color == color {
                continue;
            }
        }
        let mv = Move::normal(king_sq, to, false, PieceKind::King);
        let undo = test.make_move(mv);
        if !test.is_in_check(color) {
            evasions.push(mv);
        }
        test.unmake_move(&undo);
    }

    // 両王手の場合は玉の移動のみ
    if checkers.len() > 1 {
        return evasions;
    }

    let checker_sq = checkers[0];

    // 2. 王手している駒を取る（玉以外の駒で）
    let capture_moves = generate_moves_to_square(pos, checker_sq, color, false);
    for mv in capture_moves {
        let undo = test.make_move(mv);
        if !test.is_in_check(color) {
            evasions.push(mv);
        }
        test.unmake_move(&undo);
    }

    // 3. 合駒（スライド攻撃の場合のみ）
    let interpose_sqs = compute_interposition_squares(king_sq, checker_sq);
    for sq in &interpose_sqs {
        let interpose_moves = generate_moves_to_square(pos, *sq, color, true);
        for mv in interpose_moves {
            let undo = test.make_move(mv);
            if !test.is_in_check(color) {
                evasions.push(mv);
            }
            test.unmake_move(&undo);
        }
    }

    evasions
}

/// 指定マスに到達可能な指し手を生成（玉以外、合法性チェックなし）
fn generate_moves_to_square(
    pos: &Position,
    to: Square,
    color: Color,
    include_drops: bool,
) -> Vec<Move> {
    let mut moves = Vec::new();

    for idx in 0..81 {
        let from = Square::from_index(idx);
        if let Some(piece) = pos.piece_at(from) {
            if piece.color != color || piece.kind == PieceKind::King {
                continue;
            }
            if can_piece_reach(pos, from, piece, to) {
                add_move_with_promotion(from, to, color, piece.kind.can_promote(), pos, &mut moves);
            }
        }
    }

    if include_drops && pos.piece_at(to).is_none() {
        let hand = pos.hand(color);
        for &kind in &PieceKind::HAND_PIECES {
            if !hand.has(kind) {
                continue;
            }
            if !can_drop_to(pos, kind, to, color) {
                continue;
            }
            moves.push(Move::drop(to, kind));
        }
    }

    moves
}

/// 駒がfromからtoに到達可能かチェック（盤上の障害物を考慮）
fn can_piece_reach(pos: &Position, from: Square, piece: Piece, to: Square) -> bool {
    let color = piece.color;
    let df = to.file as i8 - from.file as i8;
    let dr = to.rank as i8 - from.rank as i8;

    match piece.kind {
        PieceKind::King => false,
        PieceKind::Gold
        | PieceKind::PromotedSilver
        | PieceKind::PromotedKnight
        | PieceKind::PromotedLance
        | PieceKind::PromotedPawn => gold_offsets(color).contains(&(df, dr)),
        PieceKind::Silver => silver_offsets(color).contains(&(df, dr)),
        PieceKind::Knight => knight_offsets(color).contains(&(df, dr)),
        PieceKind::Pawn => pawn_offsets(color).contains(&(df, dr)),
        PieceKind::Lance => {
            if df != 0 {
                return false;
            }
            let expected_dr = if color == Color::Sente { -1 } else { 1 };
            if dr.signum() != expected_dr {
                return false;
            }
            is_path_clear(pos, from, 0, expected_dr, to)
        }
        PieceKind::Rook => {
            if (df != 0 && dr != 0) || (df == 0 && dr == 0) {
                return false;
            }
            is_path_clear(pos, from, df.signum(), dr.signum(), to)
        }
        PieceKind::Bishop => {
            if df.abs() != dr.abs() || df == 0 {
                return false;
            }
            is_path_clear(pos, from, df.signum(), dr.signum(), to)
        }
        PieceKind::PromotedRook => {
            if df == 0 && dr == 0 {
                return false;
            }
            if df == 0 || dr == 0 {
                is_path_clear(pos, from, df.signum(), dr.signum(), to)
            } else {
                df.abs() <= 1 && dr.abs() <= 1
            }
        }
        PieceKind::PromotedBishop => {
            if df == 0 && dr == 0 {
                return false;
            }
            if df.abs() == dr.abs() {
                is_path_clear(pos, from, df.signum(), dr.signum(), to)
            } else {
                (df == 0 || dr == 0) && df.abs() + dr.abs() == 1
            }
        }
    }
}

/// fromからtoまでの直線上に障害物がないか（toは含まない）
fn is_path_clear(pos: &Position, from: Square, df: i8, dr: i8, to: Square) -> bool {
    let mut f = from.file as i8 + df;
    let mut r = from.rank as i8 + dr;
    while f != to.file as i8 || r != to.rank as i8 {
        if !Square::is_valid(f, r) {
            return false;
        }
        if pos.piece_at(Square::new(f as u8, r as u8)).is_some() {
            return false;
        }
        f += df;
        r += dr;
    }
    true
}

/// 駒が指定マスに打てるかチェック
fn can_drop_to(pos: &Position, kind: PieceKind, sq: Square, color: Color) -> bool {
    match kind {
        PieceKind::Pawn => {
            if (color == Color::Sente && sq.rank == 1) || (color == Color::Gote && sq.rank == 9) {
                return false;
            }
            if pos.has_pawn_on_file(color, sq.file) {
                return false;
            }
            if is_pawn_drop_mate(pos, sq, color) {
                return false;
            }
            true
        }
        PieceKind::Lance => {
            !((color == Color::Sente && sq.rank == 1) || (color == Color::Gote && sq.rank == 9))
        }
        PieceKind::Knight => {
            !((color == Color::Sente && sq.rank <= 2) || (color == Color::Gote && sq.rank >= 8))
        }
        _ => true,
    }
}

/// 王手している駒の位置を全て見つける
fn find_checkers(pos: &Position, king_sq: Square, attacker_color: Color) -> Vec<Square> {
    let mut checkers = Vec::new();

    // 金型
    for &(df, dr) in &gold_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color
                && matches!(
                    p.kind,
                    PieceKind::Gold
                        | PieceKind::PromotedSilver
                        | PieceKind::PromotedKnight
                        | PieceKind::PromotedLance
                        | PieceKind::PromotedPawn
                )
            {
                checkers.push(sq);
            }
        }
    }

    // 銀
    for &(df, dr) in &silver_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color && p.kind == PieceKind::Silver {
                checkers.push(sq);
            }
        }
    }

    // 桂
    for &(df, dr) in &knight_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color && p.kind == PieceKind::Knight {
                checkers.push(sq);
            }
        }
    }

    // 歩
    for &(df, dr) in &pawn_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color && p.kind == PieceKind::Pawn {
                checkers.push(sq);
            }
        }
    }

    // 香
    let lance_dr: i8 = if attacker_color == Color::Sente { 1 } else { -1 };
    if let Some(sq) = find_slide_checker(pos, king_sq, 0, lance_dr, attacker_color, &[PieceKind::Lance]) {
        checkers.push(sq);
    }

    // 飛車/竜（十字スライド）
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        if let Some(sq) = find_slide_checker(
            pos, king_sq, df, dr, attacker_color,
            &[PieceKind::Rook, PieceKind::PromotedRook],
        ) {
            checkers.push(sq);
        }
    }

    // 角/馬（斜めスライド）
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        if let Some(sq) = find_slide_checker(
            pos, king_sq, df, dr, attacker_color,
            &[PieceKind::Bishop, PieceKind::PromotedBishop],
        ) {
            checkers.push(sq);
        }
    }

    // 竜の斜め1マス
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if !checkers.contains(&sq) {
            if let Some(p) = pos.piece_at(sq) {
                if p.color == attacker_color && p.kind == PieceKind::PromotedRook {
                    checkers.push(sq);
                }
            }
        }
    }

    // 馬の十字1マス
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if !checkers.contains(&sq) {
            if let Some(p) = pos.piece_at(sq) {
                if p.color == attacker_color && p.kind == PieceKind::PromotedBishop {
                    checkers.push(sq);
                }
            }
        }
    }

    checkers
}

/// スライド方向に攻撃駒を探す（位置を返す）
fn find_slide_checker(
    pos: &Position,
    sq: Square,
    df: i8,
    dr: i8,
    color: Color,
    kinds: &[PieceKind],
) -> Option<Square> {
    let mut f = sq.file as i8 + df;
    let mut r = sq.rank as i8 + dr;
    while Square::is_valid(f, r) {
        let check_sq = Square::new(f as u8, r as u8);
        if let Some(piece) = pos.piece_at(check_sq) {
            if piece.color == color && kinds.contains(&piece.kind) {
                return Some(check_sq);
            }
            return None;
        }
        f += df;
        r += dr;
    }
    None
}

/// 王手回避マスの列挙（玉と王手駒の間のマス）
fn compute_interposition_squares(king_sq: Square, checker_sq: Square) -> Vec<Square> {
    let df = checker_sq.file as i8 - king_sq.file as i8;
    let dr = checker_sq.rank as i8 - king_sq.rank as i8;

    let is_line = df == 0 || dr == 0 || df.abs() == dr.abs();
    if !is_line {
        return Vec::new();
    }

    let distance = df.abs().max(dr.abs());
    if distance <= 1 {
        return Vec::new();
    }

    let step_f = df.signum();
    let step_r = dr.signum();

    let mut squares = Vec::new();
    let mut f = king_sq.file as i8 + step_f;
    let mut r = king_sq.rank as i8 + step_r;
    while f != checker_sq.file as i8 || r != checker_sq.rank as i8 {
        squares.push(Square::new(f as u8, r as u8));
        f += step_f;
        r += step_r;
    }

    squares
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
