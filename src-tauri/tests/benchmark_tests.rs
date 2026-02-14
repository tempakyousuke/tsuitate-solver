use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::metaposition::MetaPosition;
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

/// JSON ファイルを読み込んで Position に変換
/// 後手持ち駒は詰将棋ルールに従い、盤上・先手持ち駒にない残り全てを自動計算
fn load_question(path: &std::path::Path) -> Position {
    let json_str = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e));
    let q: QuestionJson = serde_json::from_str(&json_str)
        .unwrap_or_else(|e| panic!("Failed to parse {}: {}", path.display(), e));

    let mut pos = Position::new();

    // 盤上の駒の種類別カウント（後手持ち駒計算用）
    let mut used_counts = [0u8; 7]; // Rook, Bishop, Gold, Silver, Knight, Lance, Pawn

    // 盤面配置
    for bp in &q.board {
        let kind = parse_piece_kind(&bp.kind);
        let color = parse_color(&bp.color);
        pos.set_piece(Square::new(bp.file, bp.rank), Piece::new(color, kind));

        // 使用駒をカウント（成り駒は元の駒種としてカウント）
        let base_kind = kind.unpromoted();
        if base_kind != PieceKind::King {
            let idx = hand_kind_index(base_kind);
            used_counts[idx] += 1;
        }
    }

    // 先手持ち駒
    for hp in &q.sente_hand {
        let kind = parse_piece_kind(&hp.kind);
        for _ in 0..hp.count {
            pos.sente_hand.add(kind);
        }
        let idx = hand_kind_index(kind);
        used_counts[idx] += hp.count;
    }

    // 後手持ち駒：残り全て
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

fn cancel_after(secs: u64) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    cancel
}

/// 1問を解いて結果を表示するヘルパー
fn run_question(number: u32, max_depth: u32, time_limit_secs: u64) {
    let path = sample_questions_dir().join(format!("{}.json", number));
    let pos = load_question(&path);

    // 問題の盤面を表示
    println!("=== 問題 {} ===", number);
    print_position(&pos);

    let meta = MetaPosition::new(pos);
    let cancel = cancel_after(time_limit_secs);

    let mut solver = TsuitateSolver::new(max_depth, cancel);
    solver.set_trace_enabled(false); // 大量のトレースを抑制

    let start = Instant::now();
    let result = solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "問題{}: found={}, depth={}, time={:.3}s, nodes={}, msg={}",
        number,
        result.found,
        result.tree.as_ref().map_or(0, |t| t.max_moves()),
        elapsed.as_secs_f64(),
        solver.nodes_searched,
        result.message,
    );
    if let Some(ref tree) = result.tree {
        println!("  解の手順木:");
        print_solution_tree(tree, 2);
    }
    println!();
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
    // 持ち駒
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
fn print_solution_tree(node: &tsuitate_resolver_lib::solver::solution::SolutionNode, indent: usize) {
    use tsuitate_resolver_lib::solver::solution::{Observation, SolutionNode};
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

// === 個別テスト（各問題ごと） ===

#[test]
#[ignore]
fn bench_question_01() {
    run_question(1, 15, 60);
}

#[test]
#[ignore]
fn bench_question_02() {
    run_question(2, 15, 60);
}

#[test]
#[ignore]
fn bench_question_03() {
    run_question(3, 15, 60);
}

#[test]
#[ignore]
fn bench_question_04() {
    run_question(4, 15, 60);
}

#[test]
#[ignore]
fn bench_question_05() {
    run_question(5, 15, 60);
}

#[test]
#[ignore]
fn bench_question_06() {
    run_question(6, 15, 60);
}

#[test]
#[ignore]
fn bench_question_07() {
    run_question(7, 15, 60);
}

#[test]
#[ignore]
fn bench_question_08() {
    run_question(8, 15, 60);
}

#[test]
#[ignore]
fn bench_question_09() {
    run_question(9, 15, 60);
}

#[test]
#[ignore]
fn bench_question_10() {
    run_question(10, 15, 60);
}

#[test]
#[ignore]
fn bench_question_11() {
    run_question(11, 15, 60);
}

#[test]
#[ignore]
fn bench_question_12() {
    run_question(12, 15, 60);
}

#[test]
#[ignore]
fn bench_question_13() {
    run_question(13, 15, 60);
}

#[test]
#[ignore]
fn bench_question_14() {
    run_question(14, 15, 60);
}

#[test]
#[ignore]
fn bench_question_15() {
    run_question(15, 15, 60);
}

#[test]
#[ignore]
fn bench_question_16() {
    run_question(16, 15, 60);
}

#[test]
#[ignore]
fn bench_question_17() {
    run_question(17, 15, 60);
}

#[test]
#[ignore]
fn bench_question_18() {
    run_question(18, 15, 60);
}

#[test]
#[ignore]
fn bench_question_19() {
    run_question(19, 15, 60);
}

#[test]
#[ignore]
fn bench_question_20() {
    run_question(20, 15, 60);
}

#[test]
#[ignore]
fn bench_question_21() {
    run_question(21, 15, 60);
}

#[test]
#[ignore]
fn bench_question_22() {
    run_question(22, 15, 60);
}

#[test]
#[ignore]
fn bench_question_23() {
    run_question(23, 15, 60);
}

#[test]
#[ignore]
fn bench_question_24() {
    run_question(24, 15, 60);
}

#[test]
#[ignore]
fn bench_question_25() {
    run_question(25, 15, 60);
}

#[test]
#[ignore]
fn bench_question_26() {
    run_question(26, 15, 60);
}

#[test]
#[ignore]
fn bench_question_27() {
    run_question(27, 15, 60);
}

#[test]
#[ignore]
fn bench_question_28() {
    run_question(28, 15, 60);
}

#[test]
#[ignore]
fn bench_question_29() {
    run_question(29, 15, 60);
}

#[test]
#[ignore]
fn bench_question_30() {
    run_question(30, 15, 60);
}

#[test]
#[ignore]
fn bench_question_31() {
    run_question(31, 15, 60);
}

#[test]
#[ignore]
fn bench_question_32() {
    run_question(32, 15, 60);
}

#[test]
#[ignore]
fn bench_question_33() {
    run_question(33, 15, 60);
}

#[test]
#[ignore]
fn bench_question_34() {
    run_question(34, 17, 60);
}

#[test]
#[ignore]
fn bench_question_35() {
    run_question(35, 15, 60);
}

#[test]
#[ignore]
fn bench_question_36() {
    run_question(36, 15, 60);
}

#[test]
#[ignore]
fn bench_question_37() {
    run_question(37, 15, 60);
}

#[test]
#[ignore]
fn bench_question_38() {
    run_question(38, 15, 60);
}

#[test]
#[ignore]
fn bench_question_39() {
    run_question(39, 15, 60);
}

#[test]
#[ignore]
fn bench_question_40() {
    run_question(40, 15, 60);
}

/// 全問一括ベンチマーク（サマリー表付き）
/// cargo test --release -- --ignored bench_all_questions --nocapture
#[test]
#[ignore]
fn bench_all_questions() {
    let dir = sample_questions_dir();
    let time_limit_secs: u64 = 120;
    let default_max_depth: u32 = 15;

    // 問題ごとの最大探索深さ（デフォルトより深い探索が必要な問題）
    let custom_depths: std::collections::HashMap<u32, u32> =
        [(34, 17)].into_iter().collect();

    println!("=== 全問ベンチマーク (default_max_depth={}, time_limit={}s) ===\n", default_max_depth, time_limit_secs);

    struct Result {
        number: u32,
        found: bool,
        depth: u32,
        time_secs: f64,
        nodes: u64,
    }

    let mut results = Vec::new();

    for number in 1..=40 {
        let path = dir.join(format!("{}.json", number));
        if !path.exists() {
            println!("問題{}: ファイルが見つかりません", number);
            continue;
        }

        let max_depth = custom_depths.get(&number).copied().unwrap_or(default_max_depth);
        let pos = load_question(&path);
        let meta = MetaPosition::new(pos);
        let cancel = cancel_after(time_limit_secs);

        let mut solver = TsuitateSolver::new(max_depth, cancel);
        solver.set_trace_enabled(false);

        let start = Instant::now();
        let result = solver.solve(&meta);
        let elapsed = start.elapsed();

        let depth = result.tree.as_ref().map_or(0, |t| t.max_moves());
        let time_secs = elapsed.as_secs_f64();
        let nodes = solver.nodes_searched;

        println!(
            "問題{:2}: found={:<5} depth={:2} time={:8.3}s nodes={:>12} | {}",
            number, result.found, depth, time_secs, nodes, result.message,
        );

        results.push(Result {
            number,
            found: result.found,
            depth,
            time_secs,
            nodes,
        });
    }

    // サマリー
    let solved = results.iter().filter(|r| r.found).count();
    let total = results.len();
    let total_time: f64 = results.iter().map(|r| r.time_secs).sum();
    let total_nodes: u64 = results.iter().map(|r| r.nodes).sum();

    println!("\n=== サマリー ===");
    println!("解けた問題: {}/{}", solved, total);
    println!("合計時間: {:.3}s", total_time);
    println!("合計ノード数: {}", total_nodes);
    println!();
    println!("{:<8} {:<8} {:<6} {:<12} {:<14}", "問題", "結果", "手数", "時間(s)", "ノード数");
    println!("{}", "-".repeat(52));
    for r in &results {
        println!(
            "{:<8} {:<8} {:<6} {:<12.3} {:<14}",
            r.number,
            if r.found { "OK" } else { "NG" },
            if r.found { format!("{}", r.depth) } else { "-".to_string() },
            r.time_secs,
            r.nodes,
        );
    }

    // Markdown ファイル出力
    let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("benchmark-results.md");

    let now = chrono::Local::now();
    let mut md = String::new();
    md.push_str(&format!("# ベンチマーク結果\n\n"));
    md.push_str(&format!("- 実行日時: {}\n", now.format("%Y-%m-%d %H:%M:%S")));
    md.push_str(&format!("- 最大探索深さ: {}\n", default_max_depth));
    md.push_str(&format!("- 制限時間: {}秒/問\n", time_limit_secs));
    md.push_str(&format!("- 解けた問題: {}/{}\n", solved, total));
    md.push_str(&format!("- 合計時間: {:.3}秒\n", total_time));
    md.push_str(&format!("- 合計ノード数: {}\n\n", total_nodes));

    md.push_str("| 問題 | 結果 | 手数 | 時間(秒) | ノード数 |\n");
    md.push_str("|-----:|:----:|-----:|---------:|---------:|\n");
    for r in &results {
        md.push_str(&format!(
            "| {} | {} | {} | {:.3} | {} |\n",
            r.number,
            if r.found { "OK" } else { "NG" },
            if r.found { format!("{}", r.depth) } else { "-".to_string() },
            r.time_secs,
            r.nodes,
        ));
    }

    std::fs::write(&output_path, &md)
        .unwrap_or_else(|e| panic!("Failed to write {}: {}", output_path.display(), e));
    println!("\nベンチマーク結果を {} に出力しました", output_path.display());
}