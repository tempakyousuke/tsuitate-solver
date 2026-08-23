use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::metaposition::MetaPosition;
use tsuitate_solver_lib::solver::tsuitate_dfpn::TsuitateDfpnSolver;

/// fail-questions の JSON 形式（sample-questions と同一）
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

fn fail_questions_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("fail-questions")
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
    println!(
        "先手持ち駒: {}",
        if sente_hand.is_empty() { "なし" } else { sente_hand.trim() }
    );
    println!(
        "後手持ち駒: {}",
        if gote_hand.is_empty() { "なし" } else { gote_hand.trim() }
    );
}

const NODE_LIMIT: u64 = 50_000_000;
const TIME_LIMIT: u64 = 120;

/// 1問を解いて結果を表示するヘルパー（不詰期待）
fn run_fail_question(number: u32, node_limit: u64, time_limit_secs: u64) {
    let path = fail_questions_dir().join(format!("{}.json", number));
    let pos = load_question(&path);

    println!("=== 不詰問題 {} ===", number);
    print_position(&pos);

    let meta = MetaPosition::new(pos);
    let cancel = cancel_after(time_limit_secs);

    let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

    let start = Instant::now();
    let result = solver.solve_to_solution(&meta, false);
    let elapsed = start.elapsed();

    let status = if !result.found && result.message.contains("存在しません") {
        "DISPROVEN"
    } else if !result.found {
        "UNKNOWN"
    } else {
        "PROVEN(unexpected!)"
    };

    println!(
        "不詰問題{}: status={}, time={:.3}s, nodes={}, msg={}",
        number,
        status,
        elapsed.as_secs_f64(),
        solver.nodes_searched,
        result.message,
    );
    println!();
}

// === 個別テスト ===

macro_rules! dfpn_fail_test {
    ($name:ident, $num:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_fail_question($num, NODE_LIMIT, TIME_LIMIT); }
    };
}

dfpn_fail_test!(dfpn_fail_question_01, 1);
dfpn_fail_test!(dfpn_fail_question_02, 2);
dfpn_fail_test!(dfpn_fail_question_03, 3);
dfpn_fail_test!(dfpn_fail_question_04, 4);

/// fail-questions/4.json 診断テスト
#[test]
#[ignore]
fn dfpn_fail_question_04_diag() {
    let path = fail_questions_dir().join("4.json");
    let pos = load_question(&path);

    println!("=== 不詰問題 4 診断 ===");
    print_position(&pos);

    let meta = MetaPosition::new(pos);
    let cancel = cancel_after(TIME_LIMIT);

    let mut solver = TsuitateDfpnSolver::new(NODE_LIMIT, cancel);

    let start = Instant::now();
    let result = solver.solve_to_solution(&meta, false);
    let elapsed = start.elapsed();

    println!(
        "Result: found={}, nodes={}, time={:.3}s",
        result.found, solver.nodes_searched, elapsed.as_secs_f64()
    );
    println!(
        "Diagnostics: mid_and_calls={}, expand_defense_calls={}, max_meta_size={}",
        solver.mid_and_calls, solver.expand_defense_calls, solver.max_meta_size,
    );
    println!(
        "Time breakdown: gen_candidates={:.3}s, expand_defense={:.3}s, all_eff_checkmate={:.3}s",
        solver.gen_candidates_nanos as f64 / 1e9,
        solver.expand_defense_nanos as f64 / 1e9,
        solver.aec_nanos as f64 / 1e9,
    );
    println!(
        "Replay: attempts={}, full={}, partial={}, fail={}",
        solver.proof_replay_attempts,
        solver.proof_replay_full_success,
        solver.proof_replay_partial,
        solver.proof_replay_fail,
    );
    println!("Dominance hits: {}", solver.dominance_hits);
    println!(
        "gen_candidates: calls={}, total_positions={}, avg_positions={:.1}",
        solver.gen_candidates_calls,
        solver.gen_candidates_total_positions,
        if solver.gen_candidates_calls > 0 {
            solver.gen_candidates_total_positions as f64 / solver.gen_candidates_calls as f64
        } else { 0.0 },
    );
    println!(
        "check_moves_cache: hits={}, misses={}, hit_rate={:.1}%",
        solver.check_moves_cache_hits,
        solver.check_moves_cache_misses,
        if solver.check_moves_cache_hits + solver.check_moves_cache_misses > 0 {
            100.0 * solver.check_moves_cache_hits as f64
                / (solver.check_moves_cache_hits + solver.check_moves_cache_misses) as f64
        } else { 0.0 },
    );
    println!("msg: {}", result.message);
}

/// 全問一括ベンチマーク（不詰問題）
/// cargo test --release --test dfpn_fail_benchmark_tests dfpn_fail_bench_all -- --ignored --nocapture
#[test]
#[ignore]
fn dfpn_fail_bench_all_questions() {
    let dir = fail_questions_dir();
    let node_limit: u64 = NODE_LIMIT;
    let time_limit_secs: u64 = TIME_LIMIT;

    println!(
        "=== 衝立df-pn 不詰問題ベンチマーク (node_limit={}, time_limit={}s) ===\n",
        node_limit, time_limit_secs
    );

    struct Result {
        number: u32,
        status: String,
        time_secs: f64,
        nodes: u64,
    }

    let mut results = Vec::new();

    // fail-questions ディレクトリ内の番号付き JSON ファイルを収集
    let mut question_numbers: Vec<u32> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(stem) = name_str.strip_suffix(".json") {
                if let Ok(num) = stem.parse::<u32>() {
                    question_numbers.push(num);
                }
            }
        }
    }
    question_numbers.sort();

    if question_numbers.is_empty() {
        println!("fail-questions ディレクトリに問題ファイルがありません");
        return;
    }

    for &number in &question_numbers {
        let path = dir.join(format!("{}.json", number));
        let pos = load_question(&path);

        println!("--- 不詰問題 {} ---", number);
        print_position(&pos);

        let meta = MetaPosition::new(pos);
        let cancel = cancel_after(time_limit_secs);

        let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

        let start = Instant::now();
        let result = solver.solve_to_solution(&meta, false);
        let elapsed = start.elapsed();

        let time_secs = elapsed.as_secs_f64();
        let nodes = solver.nodes_searched;

        let status = if !result.found && result.message.contains("存在しません") {
            "DISPROVEN".to_string()
        } else if !result.found {
            "UNKNOWN".to_string()
        } else {
            let depth = result.tree.as_ref().map_or(0, |t| t.max_moves());
            format!("PROVEN({}手)", depth)
        };

        println!(
            "不詰問題{:2}: status={:<20} time={:8.3}s nodes={:>12} | {}",
            number, status, time_secs, nodes, result.message,
        );

        results.push(Result {
            number,
            status,
            time_secs,
            nodes,
        });
    }

    // サマリー
    let total = results.len();
    let disproven = results.iter().filter(|r| r.status == "DISPROVEN").count();
    let unknown = results.iter().filter(|r| r.status == "UNKNOWN").count();
    let proven = results.iter().filter(|r| r.status.starts_with("PROVEN")).count();
    let total_time: f64 = results.iter().map(|r| r.time_secs).sum();
    let total_nodes: u64 = results.iter().map(|r| r.nodes).sum();

    println!("\n=== サマリー [不詰問題] ===");
    println!("合計問題数: {}", total);
    println!("不詰証明: {} (DISPROVEN)", disproven);
    println!("打ち切り: {} (UNKNOWN)", unknown);
    if proven > 0 {
        println!("予期しない詰み: {} (PROVEN) ← 要確認!", proven);
    }
    println!("合計時間: {:.3}s", total_time);
    println!("合計ノード数: {}", total_nodes);
    println!();
    println!(
        "{:<8} {:<20} {:<12} {:<14}",
        "問題", "結果", "時間(s)", "ノード数"
    );
    println!("{}", "-".repeat(58));
    for r in &results {
        println!(
            "{:<8} {:<20} {:<12.3} {:<14}",
            r.number, r.status, r.time_secs, r.nodes,
        );
    }

    // Markdown ファイル出力
    let output_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("dfpn-fail-benchmark-results.md");

    let now = chrono::Local::now();
    let mut md = String::new();
    md.push_str("# 衝立df-pn 不詰問題ベンチマーク結果\n\n");
    md.push_str(&format!(
        "- 実行日時: {}\n",
        now.format("%Y-%m-%d %H:%M:%S")
    ));
    md.push_str(&format!("- ノード上限: {}\n", node_limit));
    md.push_str(&format!("- 制限時間: {}秒/問\n", time_limit_secs));
    md.push_str(&format!("- 合計問題数: {}\n", total));
    md.push_str(&format!("- 不詰証明 (DISPROVEN): {}\n", disproven));
    md.push_str(&format!("- 打ち切り (UNKNOWN): {}\n", unknown));
    if proven > 0 {
        md.push_str(&format!("- 予期しない詰み (PROVEN): {} ← 要確認!\n", proven));
    }
    md.push_str(&format!("- 合計時間: {:.3}秒\n", total_time));
    md.push_str(&format!("- 合計ノード数: {}\n\n", total_nodes));

    md.push_str("| 問題 | 結果 | 時間(秒) | ノード数 |\n");
    md.push_str("|-----:|:-----|--------:|---------:|\n");
    for r in &results {
        md.push_str(&format!(
            "| {} | {} | {:.3} | {} |\n",
            r.number, r.status, r.time_secs, r.nodes,
        ));
    }

    std::fs::write(&output_path, &md)
        .unwrap_or_else(|e| panic!("Failed to write {}: {}", output_path.display(), e));
    println!(
        "\nベンチマーク結果を {} に出力しました",
        output_path.display()
    );
}
