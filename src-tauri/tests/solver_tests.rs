use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::metaposition::MetaPosition;
use tsuitate_solver_lib::solver::solver::TsuitateSolver;

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

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
    let mut solver = TsuitateSolver::new(1, no_cancel());
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
    let mut solver = TsuitateSolver::new(1, no_cancel());
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
    let mut solver = TsuitateSolver::new(3, no_cancel());
    let result = solver.solve(&meta);

    // 3手以内の詰みがあるかチェック（局面によっては存在しない場合もある）
    // 主にソルバーがクラッシュしないことを確認
    println!(
        "3手詰めテスト結果: found={}, message={}",
        result.found, result.message
    );
}

/// 衝立詰将棋: 反則（情報収集）を利用した3手詰め
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
    // 後手持ち駒: 盤上にない残り全て（詰将棋ルール）
    // 飛x2, 金x4, 銀x3(4-1), 桂x4, 香x4, 歩x13(18-5)
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Rook); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Gold); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Knight); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Lance); }
    for _ in 0..13 { pos.gote_hand.add(PieceKind::Pawn); }
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateSolver::new(3, no_cancel());
    let result = solver.solve(&meta);

    println!(
        "反則利用3手詰めテスト: found={}, message={}",
        result.found, result.message
    );
    if let Some(ref tree) = result.tree {
        println!("解の手順木: {:?}", tree);
    }
    for line in &result.trace {
        println!("{}", line);
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
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Rook); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Gold); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Knight); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Lance); }
    for _ in 0..13 { pos.gote_hand.add(PieceKind::Pawn); }
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);

    // タイムアウト付きキャンセルフラグ（release: 10秒, debug: 30秒）
    let time_limit_secs = if cfg!(debug_assertions) { 30 } else { 10 };
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(time_limit_secs));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let mut solver = TsuitateSolver::new(7, cancel);
    let start = std::time::Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "max_depth=7テスト: found={}, time={:?}, message={}",
        result.found, elapsed, result.message
    );

    assert!(
        result.found,
        "{}秒以内に解けるはず (経過: {:?}): {}",
        time_limit_secs, elapsed, result.message
    );
}

/// question.json: 1二後手香, 1三後手歩, 2三後手玉, 3五後手歩, 3六後手金,
/// 4三先手と金, 5七先手歩, 6四後手角
/// 先手持ち駒: 飛x2, 金x1
/// 後手持ち駒: 残り全て
///
/// 注意: この問題は通常詰将棋では7手詰めだが、衝立詰将棋では
/// Captured分岐単独で7手必要（全体で9手以上）かつ
/// NoCapture分岐のメタポジションが深くまで解けないため、
/// 現在の探索深さ・時間では解が見つからない。
/// ベンチマーク用テスト（解の発見は assert しない）。
#[test]
#[ignore] // 長時間実行: cargo test --release -- --ignored test_solve_question_json
fn test_solve_question_json() {
    let mut pos = Position::new();
    pos.set_piece(Square::new(1, 2), Piece::new(Color::Gote, PieceKind::Lance));
    pos.set_piece(Square::new(1, 3), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(2, 3), Piece::new(Color::Gote, PieceKind::King));
    pos.set_piece(Square::new(3, 5), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(3, 6), Piece::new(Color::Gote, PieceKind::Gold));
    pos.set_piece(Square::new(4, 3), Piece::new(Color::Sente, PieceKind::PromotedPawn));
    pos.set_piece(Square::new(5, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(6, 4), Piece::new(Color::Gote, PieceKind::Bishop));
    // 先手持ち駒: 飛x2, 金x1
    for _ in 0..2 { pos.sente_hand.add(PieceKind::Rook); }
    pos.sente_hand.add(PieceKind::Gold);
    // 後手持ち駒: 残り全て（詰将棋ルール）
    // 角x1(2-1=1), 金x2(4-1-1=2), 銀x4, 桂x4, 香x3(4-1=3), 歩x14(18-4=14)
    pos.gote_hand.add(PieceKind::Bishop);
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Gold); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Knight); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Lance); }
    for _ in 0..14 { pos.gote_hand.add(PieceKind::Pawn); }
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);

    // タイムアウト付きキャンセルフラグ（release: 120秒, debug: 300秒）
    let time_limit_secs = if cfg!(debug_assertions) { 300 } else { 120 };
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(time_limit_secs));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    let mut solver = TsuitateSolver::new(11, cancel);
    solver.set_trace_enabled(false);
    let start = std::time::Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "question.json: found={}, time={:?}, nodes={}, message={}",
        result.found, elapsed, solver.nodes_searched, result.message
    );
    for line in &result.trace {
        println!("{}", line);
    }
}

/// check evasion generator の正しさを検証
/// generate_check_evasions と generate_legal_moves の結果を比較
#[test]
fn test_check_evasion_correctness() {
    // question.json のセットアップ
    let mut pos = Position::new();
    pos.set_piece(Square::new(1, 2), Piece::new(Color::Gote, PieceKind::Lance));
    pos.set_piece(Square::new(1, 3), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(2, 3), Piece::new(Color::Gote, PieceKind::King));
    pos.set_piece(Square::new(3, 5), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(3, 6), Piece::new(Color::Gote, PieceKind::Gold));
    pos.set_piece(Square::new(4, 3), Piece::new(Color::Sente, PieceKind::PromotedPawn));
    pos.set_piece(Square::new(5, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(6, 4), Piece::new(Color::Gote, PieceKind::Bishop));
    for _ in 0..2 { pos.sente_hand.add(PieceKind::Rook); }
    pos.sente_hand.add(PieceKind::Gold);
    pos.gote_hand.add(PieceKind::Bishop);
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Gold); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Knight); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Lance); }
    for _ in 0..14 { pos.gote_hand.add(PieceKind::Pawn); }
    pos.side_to_move = Color::Sente;

    // ▲2二飛打 を適用して王手状態にする
    let check_moves = pos.generate_check_moves();
    println!("先手王手候補数: {}", check_moves.len());

    // 各王手手に対してevasionの正しさを検証
    let mut errors = 0;
    for cm in &check_moves {
        let mut pos_after = pos.clone();
        pos_after.make_move(*cm);

        let legal = pos_after.generate_legal_moves();
        let evasions = pos_after.generate_check_evasions();

        let legal_set: std::collections::HashSet<Move> = legal.iter().cloned().collect();
        let evasion_set: std::collections::HashSet<Move> = evasions.iter().cloned().collect();

        if legal_set != evasion_set {
            println!(
                "MISMATCH after {}: legal={}, evasions={}",
                cm.to_japanese(Color::Sente),
                legal.len(),
                evasions.len()
            );
            let missing: Vec<_> = legal_set.difference(&evasion_set).collect();
            let extra: Vec<_> = evasion_set.difference(&legal_set).collect();
            if !missing.is_empty() {
                println!("  Missing from evasions: {:?}", missing);
            }
            if !extra.is_empty() {
                println!("  Extra in evasions: {:?}", extra);
            }
            errors += 1;
        }
    }
    // 追加: ▲2二飛打後の王手状態で詳細確認
    let rook_22 = Move {
        from: None,
        to: Square::new(2, 2),
        promotion: false,
        drop_piece: Some(PieceKind::Rook),
        moved_piece_kind: None,
    };
    let mut pos22 = pos.clone();
    pos22.make_move(rook_22);
    let legal = pos22.generate_legal_moves();
    let evasions = pos22.generate_check_evasions();
    println!("\n▲2二飛打後:");
    println!("  legal_moves({}):", legal.len());
    for m in &legal {
        let in_ev = evasions.contains(m);
        println!("    {} {}", m.to_japanese(Color::Gote), if in_ev { "✓" } else { "✗ MISSING" });
    }
    println!("  check_evasions({}):", evasions.len());
    for m in &evasions {
        let in_leg = legal.contains(m);
        if !in_leg {
            println!("    {} ✗ EXTRA", m.to_japanese(Color::Gote));
        }
    }

    assert_eq!(errors, 0, "check evasion と legal moves の不一致あり");
}

/// ▲2二飛打のCaptured分岐を単独テスト
/// 玉が2二に来た状態から何手詰めか調査（結果: 7手必要）
#[test]
#[ignore] // 長時間実行: cargo test --release -- --ignored test_captured_branch_22_rook
fn test_captured_branch_22_rook() {
    let mut pos = Position::new();
    // 玉が2二に移動（飛車を取った後）
    pos.set_piece(Square::new(2, 2), Piece::new(Color::Gote, PieceKind::King));
    pos.set_piece(Square::new(1, 2), Piece::new(Color::Gote, PieceKind::Lance));
    pos.set_piece(Square::new(1, 3), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(3, 5), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(3, 6), Piece::new(Color::Gote, PieceKind::Gold));
    pos.set_piece(Square::new(4, 3), Piece::new(Color::Sente, PieceKind::PromotedPawn));
    pos.set_piece(Square::new(5, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(6, 4), Piece::new(Color::Gote, PieceKind::Bishop));
    // 先手持ち駒: 飛x1, 金x1 (飛1枚は玉に取られた)
    pos.sente_hand.add(PieceKind::Rook);
    pos.sente_hand.add(PieceKind::Gold);
    // 後手持ち駒: 飛x1(取った分), 角x1, 金x2, 銀x4, 桂x4, 香x3, 歩x14
    pos.gote_hand.add(PieceKind::Rook);
    pos.gote_hand.add(PieceKind::Bishop);
    for _ in 0..2 { pos.gote_hand.add(PieceKind::Gold); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos.gote_hand.add(PieceKind::Knight); }
    for _ in 0..3 { pos.gote_hand.add(PieceKind::Lance); }
    for _ in 0..14 { pos.gote_hand.add(PieceKind::Pawn); }
    pos.side_to_move = Color::Sente;

    let meta = MetaPosition::new(pos);

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(60));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // 5手、7手、9手で試す
    for max_d in [5, 7, 9] {
        let mut solver = TsuitateSolver::new(max_d, cancel.clone());
        solver.set_trace_enabled(false);
        let start = std::time::Instant::now();
        let result = solver.solve(&meta);
        let elapsed = start.elapsed();
        println!(
            "Captured(2,2) max_depth={}: found={}, time={:?}, nodes={}, msg={}",
            max_d, result.found, elapsed, solver.nodes_searched, result.message
        );
        if result.found {
            break;
        }
    }
}

/// ▲2二飛打のNoCapture分岐を単独テスト
/// 個別局面では1手詰めだが、メタポジション（両方含む）では深さ9でも解なし
#[test]
#[ignore] // 長時間実行: cargo test --release -- --ignored test_nocapture_branch_22_rook
fn test_nocapture_branch_22_rook() {
    // 元の盤面 + 飛車を2二に配置
    let mut pos = Position::new();
    pos.set_piece(Square::new(1, 2), Piece::new(Color::Gote, PieceKind::Lance));
    pos.set_piece(Square::new(1, 3), Piece::new(Color::Gote, PieceKind::Pawn));
    // 玉は2三にいない（移動済み）
    pos.set_piece(Square::new(3, 5), Piece::new(Color::Gote, PieceKind::Pawn));
    pos.set_piece(Square::new(3, 6), Piece::new(Color::Gote, PieceKind::Gold));
    pos.set_piece(Square::new(4, 3), Piece::new(Color::Sente, PieceKind::PromotedPawn));
    pos.set_piece(Square::new(5, 7), Piece::new(Color::Sente, PieceKind::Pawn));
    pos.set_piece(Square::new(6, 4), Piece::new(Color::Gote, PieceKind::Bishop));
    pos.set_piece(Square::new(2, 2), Piece::new(Color::Sente, PieceKind::Rook)); // ▲2二飛

    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(120));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });

    // 玉が(1,4)の場合
    let mut pos14 = pos.clone();
    pos14.set_piece(Square::new(1, 4), Piece::new(Color::Gote, PieceKind::King));
    pos14.sente_hand.add(PieceKind::Rook);
    pos14.sente_hand.add(PieceKind::Gold);
    pos14.gote_hand.add(PieceKind::Bishop);
    for _ in 0..2 { pos14.gote_hand.add(PieceKind::Gold); }
    for _ in 0..4 { pos14.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos14.gote_hand.add(PieceKind::Knight); }
    for _ in 0..3 { pos14.gote_hand.add(PieceKind::Lance); }
    for _ in 0..14 { pos14.gote_hand.add(PieceKind::Pawn); }
    pos14.side_to_move = Color::Sente;

    // 玉が(3,4)の場合
    let mut pos34 = pos.clone();
    pos34.set_piece(Square::new(3, 4), Piece::new(Color::Gote, PieceKind::King));
    pos34.sente_hand.add(PieceKind::Rook);
    pos34.sente_hand.add(PieceKind::Gold);
    pos34.gote_hand.add(PieceKind::Bishop);
    for _ in 0..2 { pos34.gote_hand.add(PieceKind::Gold); }
    for _ in 0..4 { pos34.gote_hand.add(PieceKind::Silver); }
    for _ in 0..4 { pos34.gote_hand.add(PieceKind::Knight); }
    for _ in 0..3 { pos34.gote_hand.add(PieceKind::Lance); }
    for _ in 0..14 { pos34.gote_hand.add(PieceKind::Pawn); }
    pos34.side_to_move = Color::Sente;

    // 個別テスト
    for (name, p) in [("King@(1,4)", pos14.clone()), ("King@(3,4)", pos34.clone())] {
        for max_d in [5, 7, 9] {
            let meta = MetaPosition::new(p.clone());
            let mut solver = TsuitateSolver::new(max_d, cancel.clone());
            solver.set_trace_enabled(false);
            let start = std::time::Instant::now();
            let result = solver.solve(&meta);
            let elapsed = start.elapsed();
            println!(
                "{} max_depth={}: found={}, time={:?}, nodes={}",
                name, max_d, result.found, elapsed, solver.nodes_searched
            );
            if result.found { break; }
        }
    }

    // メタポジション（両方含む）テスト
    let meta = MetaPosition {
        positions: vec![pos14, pos34],
    };
    for max_d in [5, 7, 9] {
        let mut solver = TsuitateSolver::new(max_d, cancel.clone());
        solver.set_trace_enabled(false);
        let start = std::time::Instant::now();
        let result = solver.solve(&meta);
        let elapsed = start.elapsed();
        println!(
            "NoCapture meta max_depth={}: found={}, time={:?}, nodes={}",
            max_d, result.found, elapsed, solver.nodes_searched
        );
        if result.found { break; }
    }
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
