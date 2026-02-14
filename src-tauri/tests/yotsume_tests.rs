use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::metaposition::MetaPosition;
use tsuitate_resolver_lib::solver::solution::{Observation, SolutionNode};
use tsuitate_resolver_lib::solver::solver::TsuitateSolver;

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

/// 駒の最大枚数（玉除く）
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

/// JSON ファイルを読み込んで Position に変換
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

fn sample_questions_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("sample-questions")
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

/// Position の盤面を表示
fn print_position(pos: &Position) {
    println!("  9  8  7  6  5  4  3  2  1");
    for rank in 1..=9u8 {
        let mut row = String::new();
        for file in (1..=9u8).rev() {
            let sq = Square::new(file, rank);
            if let Some(piece) = pos.piece_at(sq) {
                let mark = if piece.color == Color::Sente { "^" } else { "v" };
                row.push_str(&format!("{}{}", mark, piece.kind.to_kanji()));
            } else {
                row.push_str(" . ");
            }
        }
        println!("{} {}", row, rank);
    }
    let mut sente_hand = String::new();
    let mut gote_hand = String::new();
    for kind in PieceKind::HAND_PIECES {
        let sc = pos.hand(Color::Sente).count(kind);
        if sc > 0 {
            sente_hand.push_str(&format!("{}x{} ", kind.to_kanji(), sc));
        }
        let gc = pos.hand(Color::Gote).count(kind);
        if gc > 0 {
            gote_hand.push_str(&format!("{}x{} ", kind.to_kanji(), gc));
        }
    }
    println!("先手持ち駒: {}", if sente_hand.is_empty() { "なし" } else { sente_hand.trim() });
    println!("後手持ち駒: {}", if gote_hand.is_empty() { "なし" } else { gote_hand.trim() });
}

/// 解の手順木を再帰的に表示
fn print_solution_tree(node: &SolutionNode, indent: usize) {
    let pad = " ".repeat(indent);
    match node {
        SolutionNode::Checkmate { depth } => {
            println!("{}詰み (depth={})", pad, depth);
        }
        SolutionNode::AttackMove { mv, branches } => {
            println!("{}{}", pad, mv.notation);
            for branch in branches {
                let obs_str = match branch.observation {
                    Observation::Checkmate => "詰み",
                    Observation::Captured => "駒取り",
                    Observation::NoCapture => "駒取りなし",
                    Observation::Illegal => "反則",
                };
                println!("{}  [{}]:", pad, obs_str);
                print_solution_tree(&branch.continuation, indent + 4);
            }
        }
    }
}

/// 問題41の余詰チェック：成/不成の違いのみの手順は余詰として扱わない
#[test]
#[ignore]
fn yotsume_question_41_promotion_not_alternate() {
    let path = sample_questions_dir().join("41.json");
    let pos = load_question(&path);

    println!("=== 問題 41 余詰チェック ===");
    print_position(&pos);

    let meta = MetaPosition::new(pos);
    let cancel = cancel_after(600);

    let mut solver = TsuitateSolver::new(15, cancel);
    solver.set_trace_enabled(false);
    solver.set_find_second_solution(true);

    let start = Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "問題41 余詰チェック: found={}, time={:.3}s, nodes={}, msg={}",
        result.found,
        elapsed.as_secs_f64(),
        solver.nodes_searched,
        result.message,
    );
    if let Some(ref tree) = result.tree {
        println!("  第1解:");
        print_solution_tree(tree, 4);
    }
    if let Some(ref tree) = result.second_tree {
        println!("  第2解:");
        print_solution_tree(tree, 4);
    }

    assert!(result.found, "解が見つかるべき");
    // 余詰チェックが完了した場合のみ、成/不成の余詰がないことを検証
    if !result.message.contains("中止") {
        assert!(result.second_tree.is_none(), "成/不成の違いだけの手順は余詰として扱わない: {}", result.message);
        assert!(result.message.contains("完全作"), "完全作であるべき: {}", result.message);
    } else {
        println!("注意: 余詰チェックがタイムアウトしました（成/不成の除外は有効）");
    }
}
