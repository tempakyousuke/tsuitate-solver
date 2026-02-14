use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::metaposition::MetaPosition;
use tsuitate_resolver_lib::solver::tsuitate_dfpn::{TsuitateDfpnResult, TsuitateDfpnSolver};

/// sample-questions の JSON 形式
#[derive(Debug, Deserialize)]
struct QuestionJson {
    board: Vec<BoardPiece>,
    #[serde(default)]
    sente_hand: Vec<HandPieceJson>,
}

#[derive(Debug, Deserialize)]
struct BoardPiece {
    file: u8,
    rank: u8,
    color: String,
    kind: String,
}

#[derive(Debug, Deserialize)]
struct HandPieceJson {
    kind: String,
    count: u8,
}

const MAX_PIECES: [(PieceKind, u8); 7] = [
    (PieceKind::Rook, 2),
    (PieceKind::Bishop, 2),
    (PieceKind::Gold, 4),
    (PieceKind::Silver, 4),
    (PieceKind::Knight, 4),
    (PieceKind::Lance, 4),
    (PieceKind::Pawn, 18),
];

fn parse_piece_kind(s: &str) -> PieceKind {
    match s {
        "king" => PieceKind::King,
        "rook" => PieceKind::Rook,
        "bishop" => PieceKind::Bishop,
        "gold" => PieceKind::Gold,
        "silver" => PieceKind::Silver,
        "knight" => PieceKind::Knight,
        "lance" => PieceKind::Lance,
        "pawn" => PieceKind::Pawn,
        "promoted_rook" => PieceKind::PromotedRook,
        "promoted_bishop" => PieceKind::PromotedBishop,
        "promoted_silver" => PieceKind::PromotedSilver,
        "promoted_knight" => PieceKind::PromotedKnight,
        "promoted_lance" => PieceKind::PromotedLance,
        "promoted_pawn" => PieceKind::PromotedPawn,
        _ => panic!("Unknown piece kind: {}", s),
    }
}

fn parse_color(s: &str) -> Color {
    match s {
        "sente" => Color::Sente,
        "gote" => Color::Gote,
        _ => panic!("Unknown color: {}", s),
    }
}

fn load_question(path: &std::path::Path) -> Position {
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    let q: QuestionJson = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

    let mut pos = Position::new();
    let mut used_counts = [0u8; 7];

    for bp in &q.board {
        let kind = parse_piece_kind(&bp.kind);
        let color = parse_color(&bp.color);
        pos.set_piece(Square::new(bp.file, bp.rank), Piece::new(color, kind));

        let base_kind = kind.unpromoted();
        if base_kind != PieceKind::King {
            let idx = hand_kind_index(base_kind);
            used_counts[idx] += 1;
        }
    }

    for hp in &q.sente_hand {
        let kind = parse_piece_kind(&hp.kind);
        for _ in 0..hp.count {
            pos.sente_hand.add(kind);
        }
        let idx = hand_kind_index(kind);
        used_counts[idx] += hp.count;
    }

    for &(kind, max) in &MAX_PIECES {
        let idx = hand_kind_index(kind);
        let remaining = max.saturating_sub(used_counts[idx]);
        for _ in 0..remaining {
            pos.gote_hand.add(kind);
        }
    }

    pos.side_to_move = Color::Sente;
    pos
}

fn hand_kind_index(kind: PieceKind) -> usize {
    match kind {
        PieceKind::Rook => 0,
        PieceKind::Bishop => 1,
        PieceKind::Gold => 2,
        PieceKind::Silver => 3,
        PieceKind::Knight => 4,
        PieceKind::Lance => 5,
        PieceKind::Pawn => 6,
        _ => panic!("Not a hand piece kind: {:?}", kind),
    }
}

fn sample_questions_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("sample-questions")
}

fn no_cancel() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn cancel_after(secs: u64) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    cancel
}

/// 問題1: 衝立df-pnで詰みを証明する
#[test]
#[ignore]
fn tsuitate_dfpn_question_01() {
    let path = sample_questions_dir().join("1.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);

    let cancel = cancel_after(60);
    let mut solver = TsuitateDfpnSolver::new(10_000_000, cancel);

    let start = Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "問題1: result={:?}, nodes={}, or_table={}, and_table={}, time={:.3}s",
        result,
        solver.nodes_searched,
        0, // table sizes not exposed, just show nodes
        0,
        elapsed.as_secs_f64(),
    );

    assert_eq!(
        result,
        TsuitateDfpnResult::Proven,
        "問題1は詰みが存在するので Proven になるはず"
    );
}

/// 問題2: 衝立df-pnで詰みを証明する
#[test]
#[ignore]
fn tsuitate_dfpn_question_02() {
    let path = sample_questions_dir().join("2.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);

    let cancel = cancel_after(60);
    let mut solver = TsuitateDfpnSolver::new(10_000_000, cancel);

    let start = Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "問題2: result={:?}, nodes={}, time={:.3}s",
        result, solver.nodes_searched, elapsed.as_secs_f64(),
    );

    assert_eq!(
        result,
        TsuitateDfpnResult::Proven,
        "問題2は詰みが存在するので Proven になるはず"
    );
}

/// 複数問題を一括テスト
#[test]
#[ignore]
fn tsuitate_dfpn_batch() {
    let questions: Vec<u32> = (1..=41).collect();
    let time_limit = 120;
    let node_limit = 50_000_000;

    println!("=== 衝立df-pn 一括テスト ===\n");

    for &q in &questions {
        let path = sample_questions_dir().join(format!("{}.json", q));
        if !path.exists() {
            println!("問題{}: ファイルなし、スキップ", q);
            continue;
        }

        let pos = load_question(&path);
        let meta = MetaPosition::new(pos);

        let cancel = cancel_after(time_limit);
        let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

        let start = Instant::now();
        let result = solver.solve(&meta);
        let elapsed = start.elapsed();

        println!(
            "問題{}: result={:?}, nodes={}, time={:.3}s",
            q, result, solver.nodes_searched, elapsed.as_secs_f64(),
        );
    }
}

/// キャンセルテスト
#[test]
fn tsuitate_dfpn_cancelled() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(1, 1),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.sente_hand.add(PieceKind::Gold);
    pos.side_to_move = Color::Sente;
    let meta = MetaPosition::new(pos);

    let cancel = Arc::new(AtomicBool::new(true)); // 最初からキャンセル
    let mut solver = TsuitateDfpnSolver::new(1_000_000, cancel);
    let result = solver.solve(&meta);

    assert_eq!(
        result,
        TsuitateDfpnResult::Unknown,
        "キャンセル済みなら Unknown を返すはず"
    );
}

/// ノード上限テスト
#[test]
fn tsuitate_dfpn_node_limit() {
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
    let meta = MetaPosition::new(pos);

    let mut solver = TsuitateDfpnSolver::new(0, no_cancel()); // ノード上限0
    let result = solver.solve(&meta);

    assert_eq!(
        result,
        TsuitateDfpnResult::Unknown,
        "ノード上限0なら Unknown を返すはず"
    );
}

/// 単純な1手詰み（単一局面 = 通常詰将棋と同等）
#[test]
fn tsuitate_dfpn_simple_1te() {
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
    let meta = MetaPosition::new(pos);

    let mut solver = TsuitateDfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&meta);

    assert_eq!(result, TsuitateDfpnResult::Proven, "1手詰みが証明されるはず");
    println!("1手詰み: nodes={}", solver.nodes_searched);
}

/// 詰まない局面（単一局面、駒なし）
#[test]
fn tsuitate_dfpn_no_checkmate() {
    let mut pos = Position::new();
    pos.set_piece(
        Square::new(5, 5),
        Piece::new(Color::Gote, PieceKind::King),
    );
    pos.side_to_move = Color::Sente;
    let meta = MetaPosition::new(pos);

    let mut solver = TsuitateDfpnSolver::new(1_000_000, no_cancel());
    let result = solver.solve(&meta);

    assert_eq!(
        result,
        TsuitateDfpnResult::Disproven,
        "先手に駒がないので詰まないはず"
    );
    println!("詰まない（駒なし）: nodes={}", solver.nodes_searched);
}
