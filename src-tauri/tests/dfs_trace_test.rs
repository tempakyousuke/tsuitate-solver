use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::path::PathBuf;

use serde::Deserialize;
use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::dfs_solver::DfsTsuitateSolver;
use tsuitate_resolver_lib::solver::metaposition::MetaPosition;

#[derive(Debug, Deserialize)]
struct QuestionJson {
    board: Vec<BoardPiece>,
    #[serde(default)]
    sente_hand: Vec<HandPieceJson>,
}
#[derive(Debug, Deserialize)]
struct BoardPiece { file: u8, rank: u8, color: String, kind: String }
#[derive(Debug, Deserialize)]
struct HandPieceJson { kind: String, count: u8 }

fn parse_piece_kind(s: &str) -> PieceKind {
    match s {
        "king" => PieceKind::King, "rook" => PieceKind::Rook, "bishop" => PieceKind::Bishop,
        "gold" => PieceKind::Gold, "silver" => PieceKind::Silver, "knight" => PieceKind::Knight,
        "lance" => PieceKind::Lance, "pawn" => PieceKind::Pawn,
        "promoted_rook" => PieceKind::PromotedRook, "promoted_bishop" => PieceKind::PromotedBishop,
        "promoted_silver" => PieceKind::PromotedSilver, "promoted_knight" => PieceKind::PromotedKnight,
        "promoted_lance" => PieceKind::PromotedLance, "promoted_pawn" => PieceKind::PromotedPawn,
        _ => panic!("Unknown piece kind: {}", s),
    }
}

fn hki(kind: PieceKind) -> usize {
    match kind {
        PieceKind::Rook=>0, PieceKind::Bishop=>1, PieceKind::Gold=>2,
        PieceKind::Silver=>3, PieceKind::Knight=>4, PieceKind::Lance=>5, PieceKind::Pawn=>6,
        _ => panic!(),
    }
}

fn load_question(path: &std::path::Path) -> Position {
    let json_str = std::fs::read_to_string(path).unwrap();
    let q: QuestionJson = serde_json::from_str(&json_str).unwrap();
    let mut pos = Position::new();
    let max_pieces: [(PieceKind, u8); 7] = [
        (PieceKind::Rook, 2), (PieceKind::Bishop, 2), (PieceKind::Gold, 4),
        (PieceKind::Silver, 4), (PieceKind::Knight, 4), (PieceKind::Lance, 4), (PieceKind::Pawn, 18),
    ];
    let mut used = [0u8; 7];
    for bp in &q.board {
        let kind = parse_piece_kind(&bp.kind);
        let color = if bp.color == "sente" { Color::Sente } else { Color::Gote };
        pos.set_piece(Square::new(bp.file, bp.rank), Piece::new(color, kind));
        let base = kind.unpromoted();
        if base != PieceKind::King { used[hki(base)] += 1; }
    }
    for hp in &q.sente_hand {
        let kind = parse_piece_kind(&hp.kind);
        for _ in 0..hp.count { pos.sente_hand.add(kind); }
        used[hki(kind)] += hp.count;
    }
    for &(kind, max) in &max_pieces {
        let rem = max.saturating_sub(used[hki(kind)]);
        for _ in 0..rem { pos.gote_hand.add(kind); }
    }
    pos.side_to_move = Color::Sente;
    pos
}

#[test]
#[ignore]
fn dfs_trace_question_01() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("sample-questions/1.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);
    let cancel = Arc::new(AtomicBool::new(false));

    let mut solver = DfsTsuitateSolver::new(15, cancel);
    solver.set_trace_enabled(false); // depth <= 2 のログのみ

    let result = solver.solve(&meta);

    println!("=== DFS Trace (root-level) ===");
    for line in &result.trace {
        println!("{}", line);
    }
    println!("\nTotal nodes: {}", solver.nodes_searched);
    println!("Found: {}, Message: {}", result.found, result.message);
}

#[test]
#[ignore]
fn dfs_trace_question_01_deep() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent().unwrap()
        .join("sample-questions/1.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);
    let cancel = Arc::new(AtomicBool::new(false));

    let mut solver = DfsTsuitateSolver::new(15, cancel);
    solver.set_trace_enabled(true); // 全深度のログ

    let result = solver.solve(&meta);

    // ▲７四角成 → ▲９四角打 のCaptured分岐周辺だけを抽出表示
    println!("=== Deep Trace (filtered for 94角 Captured) ===");
    let mut in_target = false;
    let mut target_depth = 0;
    for line in &result.trace {
        // ▲７四角成 の成功パス内、深さ2以降の ▲９四角打 関連を追跡
        if line.contains("▲９四角打") && line.contains("試行") {
            in_target = true;
            target_depth = line.len() - line.trim_start().len();
            println!("{}", line);
        } else if in_target {
            let indent = line.len() - line.trim_start().len();
            // ターゲット深度より浅い行で別の候補手に移ったら停止
            if indent <= target_depth && !line.trim().is_empty()
                && (line.contains("試行:") || line.contains("成功:") || line.contains("[D"))
                && !line.contains("▲９四角打")
            {
                in_target = false;
            } else {
                println!("{}", line);
            }
        }
    }
    println!("\nTotal nodes: {}", solver.nodes_searched);
}
