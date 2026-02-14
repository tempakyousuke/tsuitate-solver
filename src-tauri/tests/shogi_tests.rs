use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;

/// ヘルパー: 簡単に局面を作成
fn setup_position(pieces: &[(Square, Piece)]) -> Position {
    let mut pos = Position::new();
    for &(sq, piece) in pieces {
        pos.set_piece(sq, piece);
    }
    pos
}

#[test]
fn test_square_basic() {
    let sq = Square::new(5, 5);
    assert_eq!(sq.file, 5);
    assert_eq!(sq.rank, 5);

    let sq2 = Square::from_index(sq.index());
    assert_eq!(sq, sq2);
}

#[test]
fn test_square_to_japanese() {
    let sq = Square::new(2, 3);
    assert_eq!(sq.to_japanese(), "２三");

    let sq2 = Square::new(1, 1);
    assert_eq!(sq2.to_japanese(), "１一");
}

#[test]
fn test_piece_promotion() {
    assert_eq!(PieceKind::Pawn.promoted(), Some(PieceKind::PromotedPawn));
    assert_eq!(PieceKind::Rook.promoted(), Some(PieceKind::PromotedRook));
    assert_eq!(PieceKind::King.promoted(), None);
    assert_eq!(PieceKind::Gold.promoted(), None);
}

#[test]
fn test_piece_unpromoted() {
    assert_eq!(PieceKind::PromotedPawn.unpromoted(), PieceKind::Pawn);
    assert_eq!(PieceKind::PromotedRook.unpromoted(), PieceKind::Rook);
    assert_eq!(PieceKind::Gold.unpromoted(), PieceKind::Gold);
}

#[test]
fn test_hand_pieces() {
    let mut hand = HandPieces::new();
    assert!(!hand.has(PieceKind::Pawn));

    hand.add(PieceKind::Pawn);
    assert!(hand.has(PieceKind::Pawn));
    assert_eq!(hand.count(PieceKind::Pawn), 1);

    hand.add(PieceKind::Pawn);
    assert_eq!(hand.count(PieceKind::Pawn), 2);

    hand.remove(PieceKind::Pawn);
    assert_eq!(hand.count(PieceKind::Pawn), 1);
    assert!(hand.has(PieceKind::Pawn));
}

#[test]
fn test_hand_pieces_promoted_becomes_unpromoted() {
    let mut hand = HandPieces::new();
    hand.add(PieceKind::PromotedPawn); // 成駒を加えると歩として追加
    assert!(hand.has(PieceKind::Pawn));
    assert_eq!(hand.count(PieceKind::Pawn), 1);
}

#[test]
fn test_empty_position() {
    let pos = Position::new();
    assert!(pos.piece_at(Square::new(5, 5)).is_none());
    assert!(pos.find_king(Color::Sente).is_none());
}

#[test]
fn test_set_and_get_piece() {
    let mut pos = Position::new();
    let piece = Piece::new(Color::Sente, PieceKind::King);
    pos.set_piece(Square::new(5, 9), piece);
    assert_eq!(pos.piece_at(Square::new(5, 9)), Some(piece));
    assert!(pos.piece_at(Square::new(5, 8)).is_none());
}

#[test]
fn test_find_king() {
    let pos = setup_position(&[
        (Square::new(5, 1), Piece::new(Color::Gote, PieceKind::King)),
        (Square::new(5, 9), Piece::new(Color::Sente, PieceKind::King)),
    ]);

    assert_eq!(pos.find_king(Color::Sente), Some(Square::new(5, 9)));
    assert_eq!(pos.find_king(Color::Gote), Some(Square::new(5, 1)));
}

#[test]
fn test_king_moves() {
    // 玉が5五にいる場合、最大8方向に移動可能
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    // 玉は8方向に動ける
    assert_eq!(moves.len(), 8);
}

#[test]
fn test_pawn_move() {
    // 先手の歩は上（rank-1方向）に動く
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Pawn),
    );
    // 自玉を置かないと王手放置のチェックで落ちるので、離れた場所に配置
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    // 歩は１マス前進のみ + 玉の移動
    let pawn_moves: Vec<_> = moves
        .iter()
        .filter(|m| m.from == Some(Square::new(5, 5)))
        .collect();
    assert_eq!(pawn_moves.len(), 1);
    assert_eq!(pawn_moves[0].to, Square::new(5, 4));
}

#[test]
fn test_pawn_promotion_forced() {
    // 先手の歩が2段目にいる場合、1段目に進むとき成りが強制
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 2),
        Piece::new(Color::Sente, PieceKind::Pawn),
    );
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    let pawn_moves: Vec<_> = moves
        .iter()
        .filter(|m| m.from == Some(Square::new(5, 2)))
        .collect();
    // 1段目への移動は成りのみ（不成は行き所がないため禁止）
    assert_eq!(pawn_moves.len(), 1);
    assert!(pawn_moves[0].promotion);
}

#[test]
fn test_is_in_check() {
    // 後手玉に先手の金で王手をかける
    let pos = setup_position(&[
        (Square::new(5, 1), Piece::new(Color::Gote, PieceKind::King)),
        (
            Square::new(5, 2),
            Piece::new(Color::Sente, PieceKind::Gold),
        ),
    ]);

    assert!(pos.is_in_check(Color::Gote));
    assert!(!pos.is_in_check(Color::Sente)); // 先手は玉がないのでcheck判定なし
}

#[test]
fn test_simple_checkmate() {
    // 頭金で詰みのパターン
    // 後手玉: 1一、先手金: 1二、2二
    // 玉は (2,1) に逃げたいが両方の金の利きで塞がれ、
    // 金を取ってもお互いに紐がついているので詰み
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.set_piece(
        Square::new(1, 2),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.set_piece(
        Square::new(2, 2),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.side_to_move = Color::Gote; // 後手番

    assert!(pos.is_in_check(Color::Gote));
    assert!(pos.is_checkmate());
}

#[test]
fn test_make_unmake_move() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Pawn),
    );
    pos.side_to_move = Color::Sente;

    let mv = Move::normal(Square::new(5, 5), Square::new(5, 4), false);
    let undo = pos.make_move(mv);

    assert!(pos.piece_at(Square::new(5, 5)).is_none());
    assert_eq!(
        pos.piece_at(Square::new(5, 4)),
        Some(Piece::new(Color::Sente, PieceKind::Pawn))
    );
    assert_eq!(pos.side_to_move, Color::Gote);

    pos.unmake_move(&undo);

    assert_eq!(
        pos.piece_at(Square::new(5, 5)),
        Some(Piece::new(Color::Sente, PieceKind::Pawn))
    );
    assert!(pos.piece_at(Square::new(5, 4)).is_none());
    assert_eq!(pos.side_to_move, Color::Sente);
}

#[test]
fn test_capture_and_hand() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Rook),
    );
    pos.set_piece(
        Square::new(5, 1),
        Piece::new(Color::Gote, PieceKind::Pawn),
    );
    pos.side_to_move = Color::Sente;

    let mv = Move::normal(Square::new(5, 5), Square::new(5, 1), true); // 成って取る
    pos.make_move(mv);

    // 先手の持ち駒に歩が追加される
    assert!(pos.hand(Color::Sente).has(PieceKind::Pawn));
    // 5一に先手の竜が配置される
    assert_eq!(
        pos.piece_at(Square::new(5, 1)),
        Some(Piece::new(Color::Sente, PieceKind::PromotedRook))
    );
}

#[test]
fn test_drop_move() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let mv = Move::drop(Square::new(5, 5), PieceKind::Gold);
    pos.make_move(mv);

    assert_eq!(
        pos.piece_at(Square::new(5, 5)),
        Some(Piece::new(Color::Sente, PieceKind::Gold))
    );
    assert!(!pos.hand(Color::Sente).has(PieceKind::Gold));
}

#[test]
fn test_nifu_check() {
    // 二歩は禁止
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Pawn),
    );
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.sente_hand.add(PieceKind::Pawn);
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    // 5筋に歩は打てない
    let pawn_drops_on_file5: Vec<_> = moves
        .iter()
        .filter(|m| {
            m.drop_piece == Some(PieceKind::Pawn) && m.to.file == 5
        })
        .collect();
    assert!(pawn_drops_on_file5.is_empty());
}

#[test]
fn test_generate_check_moves() {
    // 先手の金が後手玉に王手をかける手のみ生成
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.set_piece(
        Square::new(5, 3),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let check_moves = pos.generate_check_moves();
    // 金が5二に進めば王手
    assert!(check_moves.iter().any(|m| m.to == Square::new(5, 2)));
    // 金が4二や6二に進んでも王手になる（金の斜め前の利き）
    let gold_check_moves: Vec<_> = check_moves
        .iter()
        .filter(|m| m.from == Some(Square::new(5, 3)))
        .collect();
    assert!(gold_check_moves.len() >= 1);
    // 5二は必ず含まれる
    assert!(gold_check_moves.iter().any(|m| m.to == Square::new(5, 2)));
}

#[test]
fn test_lance_moves() {
    // 先手の香車は上方向にスライド
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::Lance),
    );
    // 玉を別の筋に置いて香を邪魔しない
    pos.set_piece(
        Square::new(9, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    let lance_moves: Vec<_> = moves
        .iter()
        .filter(|m| m.from == Some(Square::new(1, 9)))
        .collect();
    // 1九から上方向に8マス: 1八〜1一
    // 1四〜1八は不成のみ (5手)
    // 1三は成/不成 (2手)
    // 1二は成/不成 (2手)
    // 1一は成のみ (1手、行き所がないため強制)
    // 合計: 5 + 2 + 2 + 1 = 10
    assert_eq!(lance_moves.len(), 10);
    // 1一に到達する手は成りのみ（行き所がないため）
    let to_rank1: Vec<_> = lance_moves
        .iter()
        .filter(|m| m.to == Square::new(1, 1))
        .collect();
    assert_eq!(to_rank1.len(), 1);
    assert!(to_rank1[0].promotion);
}

#[test]
fn test_rook_slide() {
    // 飛車のスライド移動
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Rook),
    );
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    let rook_moves: Vec<_> = moves
        .iter()
        .filter(|m| m.from == Some(Square::new(5, 5)))
        .collect();
    // 上4マス + 下4マス + 左4マス + 右4マス = 16マス
    // 1-3段目への移動は成り/不成で2通り
    // 成り対象: to.rank 1,2,3 → 上方向4マス(rank 4,3,2,1)のうち rank 1,2,3 の3マス
    // 各3マスで成り+不成 = 6通り追加
    // 合計: 16(不成) + 3(成) = 19 ... ただしrank4は成り不可
    // 正確に: rank 1(成のみではなく成/不成可), rank 2(成/不成), rank 3(成/不成), rank4(不成のみ)
    // 上: rank4(1) + rank3(2) + rank2(2) + rank1(2) = 7
    // 下: rank6,7,8,9 各1 = 4
    // 左: file4,3,2,1 各1 = 4
    // 右: file6,7,8,9 各1 = 4
    // 合計: 7 + 4 + 4 + 4 = 19
    assert_eq!(rook_moves.len(), 19);
}

#[test]
fn test_bishop_slide() {
    // 角のスライド移動
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Bishop),
    );
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let moves = pos.generate_legal_moves();
    let bishop_moves: Vec<_> = moves
        .iter()
        .filter(|m| m.from == Some(Square::new(5, 5)))
        .collect();
    // 斜め4方向:
    // 右上(file+,rank-): (6,4),(7,3),(8,2),(9,1) → rank4不成, rank3,2,1成/不成 → 1+2+2+2=7
    // 左上(file-,rank-): (4,4),(3,3),(2,2),(1,1) → 同様 → 7
    // 右下(file+,rank+): (6,6),(7,7),(8,8),(9,9) → 不成のみ → 4
    // 左下(file-,rank+): (4,6),(3,7),(2,8) → (1,9)は自玉がいて進めない → 3
    // 合計: 7+7+4+3 = 21
    assert_eq!(bishop_moves.len(), 21);
}

/// 問題22（双玉）の局面で generate_legal_moves が正常に完了するかテスト
#[test]
fn test_dual_king_legal_moves() {
    use std::time::Instant;

    let mut pos = Position::new();
    pos.set_piece(Square::new(1, 8), Piece::new(Color::Sente, PieceKind::King));
    pos.set_piece(Square::new(1, 9), Piece::new(Color::Sente, PieceKind::Lance));
    pos.set_piece(Square::new(2, 5), Piece::new(Color::Sente, PieceKind::Silver));
    pos.set_piece(Square::new(2, 6), Piece::new(Color::Gote, PieceKind::King));
    pos.set_piece(Square::new(3, 4), Piece::new(Color::Sente, PieceKind::Silver));
    pos.set_piece(Square::new(3, 5), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(3, 9), Piece::new(Color::Sente, PieceKind::Gold));
    pos.set_piece(Square::new(4, 6), Piece::new(Color::Gote, PieceKind::PromotedPawn));
    pos.set_piece(Square::new(4, 9), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.sente_hand.add(PieceKind::Pawn);
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Rook); }
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Bishop); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Gold); }
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Knight); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Lance); }
    for _ in 0..14 { pos.gote_hand.add(PieceKind::Pawn); }
    pos.side_to_move = Color::Sente;

    // 先手の合法手生成
    let start = Instant::now();
    let sente_moves = pos.generate_legal_moves();
    let sente_time = start.elapsed();
    println!("先手合法手数: {}, 時間: {:?}", sente_moves.len(), sente_time);
    assert!(sente_time.as_millis() < 100, "先手合法手生成が100ms超え: {:?}", sente_time);

    // 後手の合法手生成
    let mut pos_gote = pos.clone();
    pos_gote.side_to_move = Color::Gote;
    let start = Instant::now();
    let gote_moves = pos_gote.generate_legal_moves();
    let gote_time = start.elapsed();
    println!("後手合法手数: {}, 時間: {:?}", gote_moves.len(), gote_time);
    assert!(gote_time.as_millis() < 100, "後手合法手生成が100ms超え: {:?}", gote_time);
}
