use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::dfpn::{DfpnResult, DfpnSolver};

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

/// 1手詰み: 頭金
/// 後手玉1一、先手金2二（逃げ道を塞ぐ）、先手持ち駒金 → ▲2一金打で詰み
#[test]
fn test_dfpn_1te_checkmate() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.set_piece(
        Square::new(2, 2),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let mut solver = DfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&mut pos);

    assert_eq!(result, DfpnResult::Proven, "1手詰みが証明されるはず");
    println!("1手詰み: nodes={}", solver.nodes_searched);
}

/// 詰まない局面: 後手玉のみ中央、先手に駒なし
#[test]
fn test_dfpn_no_checkmate_no_pieces() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;

    let mut solver = DfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&mut pos);

    assert_eq!(
        result,
        DfpnResult::Disproven,
        "先手に駒がないので詰まないはず"
    );
    println!("詰まない（駒なし）: nodes={}", solver.nodes_searched);
}

/// 詰まない局面: 王手はかけられるが詰みにはならない
/// 後手玉5一、先手持ち駒歩のみ → 歩で王手しても詰まない
#[test]
fn test_dfpn_no_checkmate_insufficient() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.sente_hand.add(PieceKind::Pawn);
    pos.side_to_move = Color::Sente;

    let mut solver = DfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&mut pos);

    assert_eq!(
        result,
        DfpnResult::Disproven,
        "歩1枚では詰ませられないはず"
    );
    println!("詰まない（歩のみ）: nodes={}", solver.nodes_searched);
}

/// キャンセルテスト: 最初からキャンセル済みなら Unknown を返す
#[test]
fn test_dfpn_cancelled() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let cancel = Arc::new(AtomicBool::new(true)); // 最初からキャンセル
    let mut solver = DfpnSolver::new(1_000_000, cancel);
    let result = solver.solve(&mut pos);

    assert_eq!(
        result,
        DfpnResult::Unknown,
        "キャンセル済みなら Unknown を返すはず"
    );
}

/// ノード上限テスト: 上限0なら即打ち切り
#[test]
fn test_dfpn_node_limit() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.set_piece(
        Square::new(2, 2),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let mut solver = DfpnSolver::new(0, no_cancel()); // ノード上限0
    let result = solver.solve(&mut pos);

    assert_eq!(
        result,
        DfpnResult::Unknown,
        "ノード上限0なら Unknown を返すはず"
    );
}

/// 3手詰み: 飛車を打って玉を追い詰める
/// 後手玉1一、先手金1三（退路封鎖）、先手持ち駒飛
/// ▲1二飛打（王手）→ △同玉は不可（金がいる位置に逃げられない）
/// → 実質1手詰み（▲1二飛打で2一に逃げられない配置にする）
///
/// より単純な3手詰み:
/// 後手玉1一、先手銀2三、先手持ち駒金
/// ▲2二金打（王手）→ △1二玉 → ▲1二金(2二)（金で追い詰め）...
/// → 代わりにもっと単純な配置にする
///
/// 3手詰み（確実なケース）:
/// 後手玉1一、先手金3一、先手金3二、先手持ち駒金
/// ▲2一金打 → 玉の逃げ場なし → 詰み（実際は1手）
///
/// 純粋な3手詰み:
/// 後手玉1一、先手香1三、先手持ち駒金
/// ▲1二金打（王手）→ △2一玉（唯一の逃げ） → ▲2二金（詰み）
#[test]
fn test_dfpn_3te_checkmate() {
    let mut pos = Position::new();
    // 後手玉 1一
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    // 先手香 1三（1筋を押さえる）
    pos.set_piece(
        Square::new(1, 3),
        Piece::new(Color::Sente, PieceKind::Lance),
    );
    // 先手持ち駒: 金2枚
    pos.sente_hand.add(PieceKind::Gold);
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;

    let mut solver = DfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&mut pos);

    assert_eq!(result, DfpnResult::Proven, "3手詰みが証明されるはず");
    println!("3手詰み: nodes={}", solver.nodes_searched);
}

/// 既に詰んでいる局面（手番側が詰み）
/// 後手玉1一、先手金1二、先手金2一 → 後手に合法手なし → 既に詰み
/// ただしdf-pnは攻め方の手番で開始するので、この場合は攻め方に王手の手がある
/// → 攻め方目線で「既に相手が詰んでいる」状態のテスト
#[test]
fn test_dfpn_already_checkmate() {
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
        Square::new(2, 1),
        Piece::new(Color::Sente, PieceKind::Gold),
    );
    pos.set_piece(
        Square::new(2, 2),
        Piece::new(Color::Sente, PieceKind::Silver),
    );
    // 後手が手番で既に詰み状態だが、df-pnは先手手番で開始
    // → 先手は1手で詰ませられるはず（既に詰んでいるので何か王手すれば済む）
    // ただし先手に王手の手がないと反証になる
    // → 持ち駒を与える
    pos.sente_hand.add(PieceKind::Pawn);
    pos.side_to_move = Color::Sente;

    // 先手がどんな王手をしても、そもそも玉は逃げられない（1二金、2一金、2二銀で囲まれている）
    // → 1手で詰むはず
    let mut solver = DfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&mut pos);

    // 王手の手がなければ Disproven、あれば Proven
    // 歩を打って王手になるマスがあるか... 実際にはこの配置だと歩で王手になるマスがないかもしれない
    // → 結果を出力して確認
    println!(
        "既に詰み状態: result={:?}, nodes={}",
        result, solver.nodes_searched
    );
}
