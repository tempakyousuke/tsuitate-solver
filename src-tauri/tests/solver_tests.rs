use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::metaposition::MetaPosition;
use tsuitate_resolver_lib::solver::solver::TsuitateSolver;

/// 1手詰め: 頭金
/// 後手玉: 1一、先手: 金を持ち駒で、2一に金を打って詰み
#[test]
fn test_solve_1te_headgold() {
    let mut pos = Position::new();
    // 後手玉 1一
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    // 先手の利きで逃げ道を塞ぐ
    // 2一に金を打つために、他の逃げ道を塞ぐ
    pos.set_piece(
        Square::new(2, 2),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    // 先手持ち駒: 金
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateSolver::new(1);
    let result = solver.solve(&meta);

    assert!(result.found, "1手詰めが見つかるはず: {}", result.message);
}

/// 衝立詰将棋: 1手詰め（通常の詰将棋と同じケース - 初期局面1つのみ）
/// 後手玉: 1一、先手: 1二金、2二金 → 後手に合法手なし → 既に詰み
/// （これは初期局面で既に詰みのケース）
#[test]
fn test_already_checkmate() {
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
    pos.side_to_move = Color::Gote;

    // 後手番で既に詰み
    assert!(pos.is_checkmate());
}

/// 衝立詰将棋: MetaPositionの基本動作テスト
#[test]
fn test_metaposition_basic() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    assert_eq!(meta.positions.len(), 1);
    assert!(!meta.is_empty());
    assert!(!meta.all_checkmate());
}

/// 詰みなし局面テスト（玉が広い）
#[test]
fn test_no_checkmate() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.set_piece(
        Square::new(1, 9),
        Piece::new(Color::Sente, PieceKind::King),
    );
    pos.sente_hand.add(PieceKind::Pawn);
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateSolver::new(1);
    let result = solver.solve(&meta);

    // 歩1枚では1手で詰まない
    assert!(!result.found);
}

/// 3手詰め: 初手王手 → 玉応手 → 詰み
/// 衝立ルールでの分岐がある場合
#[test]
fn test_solve_3te() {
    let mut pos = Position::new();
    // 後手玉: 1一
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    // 先手: 1三香（1二に進んで王手可能）
    pos.set_piece(
        Square::new(1, 3),
        Piece::new(Color::Sente, PieceKind::Lance),
    );
    // 先手: 2三金（逃げ道封鎖と追い打ち用）
    pos.set_piece(
        Square::new(2, 3),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    // 先手持ち駒: 金
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateSolver::new(3);
    let result = solver.solve(&meta);

    // 3手以内の詰みがあるかチェック（局面によっては存在しない場合もある）
    // 主にソルバーがクラッシュしないことを確認
    println!(
        "3手詰めテスト結果: found={}, message={}",
        result.found, result.message
    );
}

/// 衝立詰将棋: 反則（情報収集）を利用した3手詰め
/// issue-question.json の問題
/// 正解手順: ▲７四角成 → 玉方応手 → ▲７六角(反則で情報収集) → ▲６五角打(詰み)
#[test]
fn test_solve_tsuitate_3te_with_illegal_probe() {
    let mut pos = Position::new();
    // 盤面の駒 (file, rank は 1-indexed)
    pos.set_piece(Square::new(6, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(6, 8), Piece::new(Color::Sente, PieceKind::Silver));
    pos.set_piece(Square::new(7, 5), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(8, 3), Piece::new(Color::Sente, PieceKind::Bishop));
    pos.set_piece(Square::new(8, 5), Piece::new(Color::Gote, PieceKind::King));
    pos.set_piece(Square::new(8, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(9, 3), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(9, 6), Piece::new(Color::Sente, PieceKind::Pawn));
    // 先手持ち駒: 角
    pos.sente_hand.add(PieceKind::Bishop);
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateSolver::new(3);
    let result = solver.solve(&meta);

    println!(
        "反則利用3手詰めテスト: found={}, message={}",
        result.found, result.message
    );
    if let Some(ref tree) = result.tree {
        println!("解の手順木: {:?}", tree);
    }

    assert!(
        result.found,
        "反則を利用した3手詰めが見つかるはず: {}",
        result.message
    );
}

/// 反則問題をmax_depth=7で解く（アプリのデフォルト設定をシミュレート）
#[test]
fn test_solve_tsuitate_illegal_probe_default_depth() {
    let mut pos = Position::new();
    pos.set_piece(Square::new(6, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(6, 8), Piece::new(Color::Sente, PieceKind::Silver));
    pos.set_piece(Square::new(7, 5), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(8, 3), Piece::new(Color::Sente, PieceKind::Bishop));
    pos.set_piece(Square::new(8, 5), Piece::new(Color::Gote, PieceKind::King));
    pos.set_piece(Square::new(8, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(9, 3), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(9, 6), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.sente_hand.add(PieceKind::Bishop);
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateSolver::new(7); // アプリのデフォルト
    let start = std::time::Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "max_depth=7テスト: found={}, time={:?}, message={}",
        result.found, elapsed, result.message
    );

    assert!(result.found);
    assert!(
        elapsed.as_secs() < 10,
        "10秒以内に解けるはず (実際: {:?})",
        elapsed
    );
}

/// expand_defense_moves テスト
#[test]
fn test_expand_defense_moves() {
    let mut pos = Position::new();
    // 後手玉: 1一
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    // 先手: 1二金（王手中）
    pos.set_piece(
        Square::new(1, 2),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    // 先手: 2三金（逃げ先を制限）
    pos.set_piece(
        Square::new(2, 3),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.side_to_move = Color::Gote;

    let meta = MetaPosition::new(pos);

    // 後手の応手を確認
    let first_pos = &meta.positions[0];
    let legal_moves = first_pos.generate_legal_moves();
    println!("後手の合法手数: {}", legal_moves.len());
    for mv in &legal_moves {
        println!("  {:?}", mv);
    }
}
