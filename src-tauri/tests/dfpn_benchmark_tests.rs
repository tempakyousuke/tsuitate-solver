use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::metaposition::MetaPosition;
use tsuitate_solver_lib::solver::tsuitate_dfpn::TsuitateDfpnSolver;

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

fn benchmark_output_dir() -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("benchmark");
    std::fs::create_dir_all(&dir).ok();
    dir
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
fn run_question(number: u32, node_limit: u64, time_limit_secs: u64) {
    run_question_with_options(number, node_limit, time_limit_secs, false);
}

fn run_question_shortest(number: u32, node_limit: u64, time_limit_secs: u64) {
    run_question_with_options(number, node_limit, time_limit_secs, true);
}

/// 1問を余詰めチェック付きで解いて結果を表示するヘルパー
fn run_question_second(number: u32, node_limit: u64, time_limit_secs: u64) {
    let path = sample_questions_dir().join(format!("{}.json", number));
    let pos = load_question(&path);

    println!("=== 問題 {} (余詰めチェック) ===", number);
    print_position(&pos);

    let meta = MetaPosition::new(pos);
    let cancel = cancel_after(time_limit_secs);

    let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

    let start = Instant::now();
    let result = solver.solve_to_solution_with_second(&meta, false);
    let elapsed = start.elapsed();

    let depth = result.tree.as_ref().map_or(0, |t| t.max_moves());
    let has_second = result.second_tree.is_some();
    let second_depth = result.second_tree.as_ref().map_or(0, |t| t.max_moves());

    let kizu_count = result.kizu_trees.len();
    println!(
        "問題{}: found={}, depth={}, has_second={}, second_depth={}, kizu={}, time={:.3}s, nodes={}, msg={}",
        number, result.found, depth, has_second, second_depth, kizu_count,
        elapsed.as_secs_f64(), solver.nodes_searched, result.message,
    );
    println!(
        "  singleton反証: 記録={} ヒット={}, dominance_hits={}",
        solver.singleton_disproof_stores, solver.singleton_disproof_hits, solver.dominance_hits,
    );
    if let Some(ref tree) = result.tree {
        println!("  主解:");
        print_solution_tree(tree, 2);
    }
    if let Some(ref second) = result.second_tree {
        println!("  余詰め:");
        print_solution_tree(second, 2);
    } else if result.found {
        if kizu_count > 0 {
            println!("  余詰めなし（完全作、キズ: {}件）", kizu_count);
            for (i, kizu) in result.kizu_trees.iter().enumerate() {
                println!("  キズ {}:", i + 1);
                print_solution_tree(kizu, 4);
            }
        } else {
            println!("  余詰めなし（完全作）");
        }
    }
    println!();
}

fn run_question_with_options(number: u32, node_limit: u64, time_limit_secs: u64, find_shortest: bool) {
    let path = sample_questions_dir().join(format!("{}.json", number));
    let pos = load_question(&path);

    println!("=== 問題 {} {}===", number, if find_shortest { "(最短経路) " } else { "" });
    print_position(&pos);

    let meta = MetaPosition::new(pos);
    let cancel = cancel_after(time_limit_secs);

    let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

    let start = Instant::now();
    let result = solver.solve_to_solution(&meta, find_shortest);
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

/// 解の手順木を再帰的に表示
fn print_solution_tree(
    node: &tsuitate_solver_lib::solver::solution::SolutionNode,
    indent: usize,
) {
    use tsuitate_solver_lib::solver::solution::{Observation, SolutionNode};
    let pad = " ".repeat(indent);
    match node {
        SolutionNode::Checkmate { depth, .. } => {
            println!("{}詰み (depth={})", pad, depth);
        }
        SolutionNode::AttackMove { mv, branches, .. } => {
            println!("{}{}", pad, mv.notation);
            for branch in branches {
                let obs_str = match &branch.observation {
                    Observation::Checkmate => "詰み".to_string(),
                    Observation::Captured { file, rank } => format!("{}{}駒取り", file, rank),
                    Observation::NoCapture => "駒取りなし".to_string(),
                    Observation::Illegal => "反則".to_string(),
                };
                println!("{}  [{}]:", pad, obs_str);
                print_solution_tree(&branch.continuation, indent + 4);
            }
        }
    }
}

// === 個別テスト（各問題ごと） ===

const NODE_LIMIT: u64 = 50_000_000;
const TIME_LIMIT: u64 = 120;

#[test]
#[ignore]
fn dfpn_bench_question_01() { run_question(1, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_02() { run_question(2, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_03() { run_question(3, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_04() { run_question(4, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_05() { run_question(5, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_06() { run_question(6, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_07() { run_question(7, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_08() { run_question(8, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_09() { run_question(9, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_10() { run_question(10, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_11() { run_question(11, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_12() { run_question(12, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_13() { run_question(13, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_14() { run_question(14, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_15() { run_question(15, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_16() { run_question(16, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_17() { run_question(17, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_18() { run_question(18, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_19() { run_question(19, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_20() { run_question(20, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_21() { run_question(21, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_22() { run_question(22, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_23() { run_question(23, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_24() { run_question(24, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_25() { run_question(25, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_26() { run_question(26, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_27() { run_question(27, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_28() { run_question(28, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_29() { run_question(29, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_30() { run_question(30, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_31() { run_question(31, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_32() { run_question(32, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_33() { run_question(33, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_34() { run_question(34, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_35() { run_question(35, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_36() { run_question(36, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_37() { run_question(37, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_38() { run_question(38, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_39() { run_question(39, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_40() { run_question(40, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_41() { run_question(41, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_42() { run_question(42, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_43() { run_question(43, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_44() { run_question(44, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_45() { run_question(45, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_46() { run_question(46, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_47() { run_question(47, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_48() { run_question(48, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_49() { run_question(49, NODE_LIMIT, TIME_LIMIT); }

#[test]
#[ignore]
fn dfpn_bench_question_50() { run_question(50, NODE_LIMIT, TIME_LIMIT); }

macro_rules! dfpn_test {
    ($name:ident, $num:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_question($num, NODE_LIMIT, TIME_LIMIT); }
    };
}

dfpn_test!(dfpn_bench_question_51, 51);
dfpn_test!(dfpn_bench_question_52, 52);
dfpn_test!(dfpn_bench_question_53, 53);
dfpn_test!(dfpn_bench_question_54, 54);
dfpn_test!(dfpn_bench_question_55, 55);
dfpn_test!(dfpn_bench_question_56, 56);
dfpn_test!(dfpn_bench_question_57, 57);
dfpn_test!(dfpn_bench_question_58, 58);
dfpn_test!(dfpn_bench_question_59, 59);
dfpn_test!(dfpn_bench_question_60, 60);
dfpn_test!(dfpn_bench_question_61, 61);
dfpn_test!(dfpn_bench_question_62, 62);
dfpn_test!(dfpn_bench_question_63, 63);
dfpn_test!(dfpn_bench_question_64, 64);
dfpn_test!(dfpn_bench_question_65, 65);
dfpn_test!(dfpn_bench_question_66, 66);
dfpn_test!(dfpn_bench_question_67, 67);
dfpn_test!(dfpn_bench_question_68, 68);
dfpn_test!(dfpn_bench_question_69, 69);
dfpn_test!(dfpn_bench_question_70, 70);
dfpn_test!(dfpn_bench_question_71, 71);
dfpn_test!(dfpn_bench_question_72, 72);
dfpn_test!(dfpn_bench_question_73, 73);
dfpn_test!(dfpn_bench_question_74, 74);
dfpn_test!(dfpn_bench_question_75, 75);
dfpn_test!(dfpn_bench_question_76, 76);
dfpn_test!(dfpn_bench_question_77, 77);
dfpn_test!(dfpn_bench_question_78, 78);
dfpn_test!(dfpn_bench_question_79, 79);
dfpn_test!(dfpn_bench_question_80, 80);
dfpn_test!(dfpn_bench_question_81, 81);
dfpn_test!(dfpn_bench_question_82, 82);
dfpn_test!(dfpn_bench_question_83, 83);
dfpn_test!(dfpn_bench_question_84, 84);
dfpn_test!(dfpn_bench_question_85, 85);
dfpn_test!(dfpn_bench_question_86, 86);
dfpn_test!(dfpn_bench_question_87, 87);
dfpn_test!(dfpn_bench_question_88, 88);
dfpn_test!(dfpn_bench_question_89, 89);
dfpn_test!(dfpn_bench_question_90, 90);
dfpn_test!(dfpn_bench_question_91, 91);
dfpn_test!(dfpn_bench_question_92, 92);
dfpn_test!(dfpn_bench_question_93, 93);
dfpn_test!(dfpn_bench_question_94, 94);
dfpn_test!(dfpn_bench_question_95, 95);
dfpn_test!(dfpn_bench_question_96, 96);
dfpn_test!(dfpn_bench_question_97, 97);
dfpn_test!(dfpn_bench_question_98, 98);
dfpn_test!(dfpn_bench_question_99, 99);
dfpn_test!(dfpn_bench_question_100, 100);
dfpn_test!(dfpn_bench_question_101, 101);
dfpn_test!(dfpn_bench_question_102, 102);
dfpn_test!(dfpn_bench_question_103, 103);
dfpn_test!(dfpn_bench_question_104, 104);
dfpn_test!(dfpn_bench_question_105, 105);
dfpn_test!(dfpn_bench_question_106, 106);
dfpn_test!(dfpn_bench_question_107, 107);
dfpn_test!(dfpn_bench_question_108, 108);
dfpn_test!(dfpn_bench_question_109, 109);
dfpn_test!(dfpn_bench_question_110, 110);
dfpn_test!(dfpn_bench_question_111, 111);
dfpn_test!(dfpn_bench_question_112, 112);
dfpn_test!(dfpn_bench_question_113, 113);
dfpn_test!(dfpn_bench_question_114, 114);
dfpn_test!(dfpn_bench_question_115, 115);
dfpn_test!(dfpn_bench_question_116, 116);
dfpn_test!(dfpn_bench_question_117, 117);
dfpn_test!(dfpn_bench_question_118, 118);
dfpn_test!(dfpn_bench_question_119, 119);
dfpn_test!(dfpn_bench_question_120, 120);
dfpn_test!(dfpn_bench_question_121, 121);
dfpn_test!(dfpn_bench_question_122, 122);
dfpn_test!(dfpn_bench_question_123, 123);
dfpn_test!(dfpn_bench_question_124, 124);
dfpn_test!(dfpn_bench_question_125, 125);
dfpn_test!(dfpn_bench_question_126, 126);
dfpn_test!(dfpn_bench_question_127, 127);
dfpn_test!(dfpn_bench_question_128, 128);
dfpn_test!(dfpn_bench_question_129, 129);
dfpn_test!(dfpn_bench_question_130, 130);
dfpn_test!(dfpn_bench_question_131, 131);
dfpn_test!(dfpn_bench_question_132, 132);
dfpn_test!(dfpn_bench_question_133, 133);
dfpn_test!(dfpn_bench_question_134, 134);
dfpn_test!(dfpn_bench_question_135, 135);
dfpn_test!(dfpn_bench_question_136, 136);
dfpn_test!(dfpn_bench_question_137, 137);
dfpn_test!(dfpn_bench_question_138, 138);
dfpn_test!(dfpn_bench_question_139, 139);
dfpn_test!(dfpn_bench_question_140, 140);
dfpn_test!(dfpn_bench_question_141, 141);
dfpn_test!(dfpn_bench_question_142, 142);
dfpn_test!(dfpn_bench_question_143, 143);
dfpn_test!(dfpn_bench_question_144, 144);
dfpn_test!(dfpn_bench_question_145, 145);
dfpn_test!(dfpn_bench_question_146, 146);
dfpn_test!(dfpn_bench_question_147, 147);

// === 個別テスト（最短経路探索あり） ===

macro_rules! dfpn_shortest_test {
    ($name:ident, $num:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_question_shortest($num, NODE_LIMIT, TIME_LIMIT); }
    };
}

dfpn_shortest_test!(dfpn_bench_shortest_question_01, 1);
dfpn_shortest_test!(dfpn_bench_shortest_question_02, 2);
dfpn_shortest_test!(dfpn_bench_shortest_question_03, 3);
dfpn_shortest_test!(dfpn_bench_shortest_question_04, 4);
dfpn_shortest_test!(dfpn_bench_shortest_question_05, 5);
dfpn_shortest_test!(dfpn_bench_shortest_question_06, 6);
dfpn_shortest_test!(dfpn_bench_shortest_question_07, 7);
dfpn_shortest_test!(dfpn_bench_shortest_question_08, 8);
dfpn_shortest_test!(dfpn_bench_shortest_question_09, 9);
dfpn_shortest_test!(dfpn_bench_shortest_question_10, 10);
dfpn_shortest_test!(dfpn_bench_shortest_question_11, 11);
dfpn_shortest_test!(dfpn_bench_shortest_question_12, 12);
dfpn_shortest_test!(dfpn_bench_shortest_question_13, 13);
dfpn_shortest_test!(dfpn_bench_shortest_question_14, 14);
dfpn_shortest_test!(dfpn_bench_shortest_question_15, 15);
dfpn_shortest_test!(dfpn_bench_shortest_question_16, 16);
dfpn_shortest_test!(dfpn_bench_shortest_question_17, 17);
dfpn_shortest_test!(dfpn_bench_shortest_question_18, 18);
dfpn_shortest_test!(dfpn_bench_shortest_question_19, 19);
dfpn_shortest_test!(dfpn_bench_shortest_question_20, 20);
dfpn_shortest_test!(dfpn_bench_shortest_question_21, 21);
dfpn_shortest_test!(dfpn_bench_shortest_question_22, 22);
dfpn_shortest_test!(dfpn_bench_shortest_question_23, 23);
dfpn_shortest_test!(dfpn_bench_shortest_question_24, 24);
dfpn_shortest_test!(dfpn_bench_shortest_question_25, 25);
dfpn_shortest_test!(dfpn_bench_shortest_question_26, 26);
dfpn_shortest_test!(dfpn_bench_shortest_question_27, 27);
dfpn_shortest_test!(dfpn_bench_shortest_question_28, 28);
dfpn_shortest_test!(dfpn_bench_shortest_question_29, 29);
dfpn_shortest_test!(dfpn_bench_shortest_question_30, 30);
dfpn_shortest_test!(dfpn_bench_shortest_question_31, 31);
dfpn_shortest_test!(dfpn_bench_shortest_question_32, 32);
dfpn_shortest_test!(dfpn_bench_shortest_question_33, 33);
dfpn_shortest_test!(dfpn_bench_shortest_question_34, 34);
dfpn_shortest_test!(dfpn_bench_shortest_question_35, 35);
dfpn_shortest_test!(dfpn_bench_shortest_question_36, 36);
dfpn_shortest_test!(dfpn_bench_shortest_question_37, 37);
dfpn_shortest_test!(dfpn_bench_shortest_question_38, 38);
dfpn_shortest_test!(dfpn_bench_shortest_question_39, 39);
dfpn_shortest_test!(dfpn_bench_shortest_question_40, 40);
dfpn_shortest_test!(dfpn_bench_shortest_question_41, 41);
dfpn_shortest_test!(dfpn_bench_shortest_question_42, 42);
dfpn_shortest_test!(dfpn_bench_shortest_question_43, 43);
dfpn_shortest_test!(dfpn_bench_shortest_question_44, 44);
dfpn_shortest_test!(dfpn_bench_shortest_question_45, 45);
dfpn_shortest_test!(dfpn_bench_shortest_question_46, 46);
dfpn_shortest_test!(dfpn_bench_shortest_question_47, 47);
dfpn_shortest_test!(dfpn_bench_shortest_question_48, 48);
dfpn_shortest_test!(dfpn_bench_shortest_question_49, 49);
dfpn_shortest_test!(dfpn_bench_shortest_question_50, 50);
dfpn_shortest_test!(dfpn_bench_shortest_question_51, 51);
dfpn_shortest_test!(dfpn_bench_shortest_question_52, 52);
dfpn_shortest_test!(dfpn_bench_shortest_question_53, 53);
dfpn_shortest_test!(dfpn_bench_shortest_question_54, 54);
dfpn_shortest_test!(dfpn_bench_shortest_question_55, 55);
dfpn_shortest_test!(dfpn_bench_shortest_question_56, 56);
dfpn_shortest_test!(dfpn_bench_shortest_question_57, 57);
dfpn_shortest_test!(dfpn_bench_shortest_question_58, 58);
dfpn_shortest_test!(dfpn_bench_shortest_question_59, 59);
dfpn_shortest_test!(dfpn_bench_shortest_question_60, 60);
dfpn_shortest_test!(dfpn_bench_shortest_question_61, 61);
dfpn_shortest_test!(dfpn_bench_shortest_question_62, 62);
dfpn_shortest_test!(dfpn_bench_shortest_question_63, 63);
dfpn_shortest_test!(dfpn_bench_shortest_question_64, 64);
dfpn_shortest_test!(dfpn_bench_shortest_question_65, 65);
dfpn_shortest_test!(dfpn_bench_shortest_question_66, 66);
dfpn_shortest_test!(dfpn_bench_shortest_question_67, 67);
dfpn_shortest_test!(dfpn_bench_shortest_question_68, 68);
dfpn_shortest_test!(dfpn_bench_shortest_question_69, 69);
dfpn_shortest_test!(dfpn_bench_shortest_question_70, 70);
dfpn_shortest_test!(dfpn_bench_shortest_question_71, 71);
dfpn_shortest_test!(dfpn_bench_shortest_question_72, 72);
dfpn_shortest_test!(dfpn_bench_shortest_question_73, 73);
dfpn_shortest_test!(dfpn_bench_shortest_question_74, 74);
dfpn_shortest_test!(dfpn_bench_shortest_question_75, 75);
dfpn_shortest_test!(dfpn_bench_shortest_question_76, 76);
dfpn_shortest_test!(dfpn_bench_shortest_question_77, 77);
dfpn_shortest_test!(dfpn_bench_shortest_question_78, 78);
dfpn_shortest_test!(dfpn_bench_shortest_question_79, 79);
dfpn_shortest_test!(dfpn_bench_shortest_question_80, 80);
dfpn_shortest_test!(dfpn_bench_shortest_question_81, 81);
dfpn_shortest_test!(dfpn_bench_shortest_question_82, 82);
dfpn_shortest_test!(dfpn_bench_shortest_question_83, 83);
dfpn_shortest_test!(dfpn_bench_shortest_question_84, 84);
dfpn_shortest_test!(dfpn_bench_shortest_question_85, 85);
dfpn_shortest_test!(dfpn_bench_shortest_question_86, 86);
dfpn_shortest_test!(dfpn_bench_shortest_question_87, 87);
dfpn_shortest_test!(dfpn_bench_shortest_question_88, 88);
dfpn_shortest_test!(dfpn_bench_shortest_question_89, 89);
dfpn_shortest_test!(dfpn_bench_shortest_question_90, 90);
dfpn_shortest_test!(dfpn_bench_shortest_question_91, 91);
dfpn_shortest_test!(dfpn_bench_shortest_question_92, 92);
dfpn_shortest_test!(dfpn_bench_shortest_question_93, 93);
dfpn_shortest_test!(dfpn_bench_shortest_question_94, 94);
dfpn_shortest_test!(dfpn_bench_shortest_question_95, 95);
dfpn_shortest_test!(dfpn_bench_shortest_question_96, 96);
dfpn_shortest_test!(dfpn_bench_shortest_question_97, 97);
dfpn_shortest_test!(dfpn_bench_shortest_question_98, 98);
dfpn_shortest_test!(dfpn_bench_shortest_question_99, 99);
dfpn_shortest_test!(dfpn_bench_shortest_question_100, 100);
dfpn_shortest_test!(dfpn_bench_shortest_question_101, 101);
dfpn_shortest_test!(dfpn_bench_shortest_question_102, 102);
dfpn_shortest_test!(dfpn_bench_shortest_question_103, 103);
dfpn_shortest_test!(dfpn_bench_shortest_question_104, 104);
dfpn_shortest_test!(dfpn_bench_shortest_question_105, 105);
dfpn_shortest_test!(dfpn_bench_shortest_question_106, 106);
dfpn_shortest_test!(dfpn_bench_shortest_question_107, 107);
dfpn_shortest_test!(dfpn_bench_shortest_question_108, 108);
dfpn_shortest_test!(dfpn_bench_shortest_question_109, 109);
dfpn_shortest_test!(dfpn_bench_shortest_question_110, 110);
dfpn_shortest_test!(dfpn_bench_shortest_question_111, 111);
dfpn_shortest_test!(dfpn_bench_shortest_question_112, 112);
dfpn_shortest_test!(dfpn_bench_shortest_question_113, 113);
dfpn_shortest_test!(dfpn_bench_shortest_question_114, 114);
dfpn_shortest_test!(dfpn_bench_shortest_question_115, 115);
dfpn_shortest_test!(dfpn_bench_shortest_question_116, 116);
dfpn_shortest_test!(dfpn_bench_shortest_question_117, 117);
dfpn_shortest_test!(dfpn_bench_shortest_question_118, 118);
dfpn_shortest_test!(dfpn_bench_shortest_question_119, 119);
dfpn_shortest_test!(dfpn_bench_shortest_question_120, 120);
dfpn_shortest_test!(dfpn_bench_shortest_question_121, 121);
dfpn_shortest_test!(dfpn_bench_shortest_question_122, 122);
dfpn_shortest_test!(dfpn_bench_shortest_question_123, 123);
dfpn_shortest_test!(dfpn_bench_shortest_question_124, 124);
dfpn_shortest_test!(dfpn_bench_shortest_question_125, 125);
dfpn_shortest_test!(dfpn_bench_shortest_question_126, 126);
dfpn_shortest_test!(dfpn_bench_shortest_question_127, 127);
dfpn_shortest_test!(dfpn_bench_shortest_question_128, 128);
dfpn_shortest_test!(dfpn_bench_shortest_question_129, 129);
dfpn_shortest_test!(dfpn_bench_shortest_question_130, 130);
dfpn_shortest_test!(dfpn_bench_shortest_question_131, 131);
dfpn_shortest_test!(dfpn_bench_shortest_question_132, 132);
dfpn_shortest_test!(dfpn_bench_shortest_question_133, 133);
dfpn_shortest_test!(dfpn_bench_shortest_question_134, 134);
dfpn_shortest_test!(dfpn_bench_shortest_question_135, 135);
dfpn_shortest_test!(dfpn_bench_shortest_question_136, 136);
dfpn_shortest_test!(dfpn_bench_shortest_question_137, 137);
dfpn_shortest_test!(dfpn_bench_shortest_question_138, 138);
dfpn_shortest_test!(dfpn_bench_shortest_question_139, 139);
dfpn_shortest_test!(dfpn_bench_shortest_question_140, 140);
dfpn_shortest_test!(dfpn_bench_shortest_question_141, 141);
dfpn_shortest_test!(dfpn_bench_shortest_question_142, 142);
dfpn_shortest_test!(dfpn_bench_shortest_question_143, 143);
dfpn_shortest_test!(dfpn_bench_shortest_question_144, 144);
dfpn_shortest_test!(dfpn_bench_shortest_question_145, 145);
dfpn_shortest_test!(dfpn_bench_shortest_question_146, 146);
dfpn_shortest_test!(dfpn_bench_shortest_question_147, 147);

// === 個別テスト（余詰めチェック） ===

macro_rules! dfpn_second_test {
    ($name:ident, $num:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_question_second($num, NODE_LIMIT, TIME_LIMIT); }
    };
}

dfpn_second_test!(dfpn_bench_second_question_01, 1);
dfpn_second_test!(dfpn_bench_second_question_02, 2);
dfpn_second_test!(dfpn_bench_second_question_03, 3);
dfpn_second_test!(dfpn_bench_second_question_04, 4);
dfpn_second_test!(dfpn_bench_second_question_05, 5);
dfpn_second_test!(dfpn_bench_second_question_06, 6);
dfpn_second_test!(dfpn_bench_second_question_07, 7);
dfpn_second_test!(dfpn_bench_second_question_08, 8);
dfpn_second_test!(dfpn_bench_second_question_09, 9);
dfpn_second_test!(dfpn_bench_second_question_10, 10);
dfpn_second_test!(dfpn_bench_second_question_11, 11);
dfpn_second_test!(dfpn_bench_second_question_12, 12);
dfpn_second_test!(dfpn_bench_second_question_13, 13);
dfpn_second_test!(dfpn_bench_second_question_14, 14);
dfpn_second_test!(dfpn_bench_second_question_15, 15);
dfpn_second_test!(dfpn_bench_second_question_16, 16);
dfpn_second_test!(dfpn_bench_second_question_17, 17);
dfpn_second_test!(dfpn_bench_second_question_18, 18);
dfpn_second_test!(dfpn_bench_second_question_19, 19);
dfpn_second_test!(dfpn_bench_second_question_20, 20);
dfpn_second_test!(dfpn_bench_second_question_21, 21);
dfpn_second_test!(dfpn_bench_second_question_22, 22);
dfpn_second_test!(dfpn_bench_second_question_23, 23);
dfpn_second_test!(dfpn_bench_second_question_24, 24);
dfpn_second_test!(dfpn_bench_second_question_25, 25);
dfpn_second_test!(dfpn_bench_second_question_26, 26);
dfpn_second_test!(dfpn_bench_second_question_27, 27);
dfpn_second_test!(dfpn_bench_second_question_28, 28);
dfpn_second_test!(dfpn_bench_second_question_29, 29);
dfpn_second_test!(dfpn_bench_second_question_30, 30);
dfpn_second_test!(dfpn_bench_second_question_31, 31);
dfpn_second_test!(dfpn_bench_second_question_32, 32);
dfpn_second_test!(dfpn_bench_second_question_33, 33);
dfpn_second_test!(dfpn_bench_second_question_34, 34);
dfpn_second_test!(dfpn_bench_second_question_35, 35);
dfpn_second_test!(dfpn_bench_second_question_36, 36);
dfpn_second_test!(dfpn_bench_second_question_37, 37);
dfpn_second_test!(dfpn_bench_second_question_38, 38);
dfpn_second_test!(dfpn_bench_second_question_39, 39);
dfpn_second_test!(dfpn_bench_second_question_40, 40);
dfpn_second_test!(dfpn_bench_second_question_41, 41);
dfpn_second_test!(dfpn_bench_second_question_42, 42);
dfpn_second_test!(dfpn_bench_second_question_43, 43);
dfpn_second_test!(dfpn_bench_second_question_44, 44);
dfpn_second_test!(dfpn_bench_second_question_45, 45);
dfpn_second_test!(dfpn_bench_second_question_46, 46);
dfpn_second_test!(dfpn_bench_second_question_47, 47);
dfpn_second_test!(dfpn_bench_second_question_48, 48);
dfpn_second_test!(dfpn_bench_second_question_49, 49);
dfpn_second_test!(dfpn_bench_second_question_50, 50);
dfpn_second_test!(dfpn_bench_second_question_51, 51);
dfpn_second_test!(dfpn_bench_second_question_52, 52);
dfpn_second_test!(dfpn_bench_second_question_53, 53);
dfpn_second_test!(dfpn_bench_second_question_54, 54);
dfpn_second_test!(dfpn_bench_second_question_55, 55);
dfpn_second_test!(dfpn_bench_second_question_56, 56);
dfpn_second_test!(dfpn_bench_second_question_57, 57);
dfpn_second_test!(dfpn_bench_second_question_58, 58);
dfpn_second_test!(dfpn_bench_second_question_59, 59);
dfpn_second_test!(dfpn_bench_second_question_60, 60);
dfpn_second_test!(dfpn_bench_second_question_61, 61);
dfpn_second_test!(dfpn_bench_second_question_62, 62);
dfpn_second_test!(dfpn_bench_second_question_63, 63);
dfpn_second_test!(dfpn_bench_second_question_64, 64);
dfpn_second_test!(dfpn_bench_second_question_65, 65);
dfpn_second_test!(dfpn_bench_second_question_66, 66);
dfpn_second_test!(dfpn_bench_second_question_67, 67);
dfpn_second_test!(dfpn_bench_second_question_68, 68);
dfpn_second_test!(dfpn_bench_second_question_69, 69);
dfpn_second_test!(dfpn_bench_second_question_70, 70);
dfpn_second_test!(dfpn_bench_second_question_71, 71);
dfpn_second_test!(dfpn_bench_second_question_72, 72);
dfpn_second_test!(dfpn_bench_second_question_73, 73);
dfpn_second_test!(dfpn_bench_second_question_74, 74);
dfpn_second_test!(dfpn_bench_second_question_75, 75);
dfpn_second_test!(dfpn_bench_second_question_76, 76);
dfpn_second_test!(dfpn_bench_second_question_77, 77);
dfpn_second_test!(dfpn_bench_second_question_78, 78);
dfpn_second_test!(dfpn_bench_second_question_79, 79);
dfpn_second_test!(dfpn_bench_second_question_80, 80);
dfpn_second_test!(dfpn_bench_second_question_81, 81);
dfpn_second_test!(dfpn_bench_second_question_82, 82);
dfpn_second_test!(dfpn_bench_second_question_83, 83);
dfpn_second_test!(dfpn_bench_second_question_84, 84);
dfpn_second_test!(dfpn_bench_second_question_85, 85);
dfpn_second_test!(dfpn_bench_second_question_86, 86);
dfpn_second_test!(dfpn_bench_second_question_87, 87);
dfpn_second_test!(dfpn_bench_second_question_88, 88);
dfpn_second_test!(dfpn_bench_second_question_89, 89);
dfpn_second_test!(dfpn_bench_second_question_90, 90);
dfpn_second_test!(dfpn_bench_second_question_91, 91);
dfpn_second_test!(dfpn_bench_second_question_92, 92);
dfpn_second_test!(dfpn_bench_second_question_93, 93);
dfpn_second_test!(dfpn_bench_second_question_94, 94);
dfpn_second_test!(dfpn_bench_second_question_95, 95);
dfpn_second_test!(dfpn_bench_second_question_96, 96);
dfpn_second_test!(dfpn_bench_second_question_97, 97);
dfpn_second_test!(dfpn_bench_second_question_98, 98);
dfpn_second_test!(dfpn_bench_second_question_99, 99);
dfpn_second_test!(dfpn_bench_second_question_100, 100);
dfpn_second_test!(dfpn_bench_second_question_101, 101);
dfpn_second_test!(dfpn_bench_second_question_102, 102);
dfpn_second_test!(dfpn_bench_second_question_103, 103);
dfpn_second_test!(dfpn_bench_second_question_104, 104);
dfpn_second_test!(dfpn_bench_second_question_105, 105);
dfpn_second_test!(dfpn_bench_second_question_106, 106);
dfpn_second_test!(dfpn_bench_second_question_107, 107);
dfpn_second_test!(dfpn_bench_second_question_108, 108);
dfpn_second_test!(dfpn_bench_second_question_109, 109);
dfpn_second_test!(dfpn_bench_second_question_110, 110);
dfpn_second_test!(dfpn_bench_second_question_111, 111);
dfpn_second_test!(dfpn_bench_second_question_112, 112);
dfpn_second_test!(dfpn_bench_second_question_113, 113);
dfpn_second_test!(dfpn_bench_second_question_114, 114);
dfpn_second_test!(dfpn_bench_second_question_115, 115);
dfpn_second_test!(dfpn_bench_second_question_116, 116);
dfpn_second_test!(dfpn_bench_second_question_117, 117);
dfpn_second_test!(dfpn_bench_second_question_118, 118);
dfpn_second_test!(dfpn_bench_second_question_119, 119);
dfpn_second_test!(dfpn_bench_second_question_120, 120);
dfpn_second_test!(dfpn_bench_second_question_121, 121);
dfpn_second_test!(dfpn_bench_second_question_122, 122);
dfpn_second_test!(dfpn_bench_second_question_123, 123);
dfpn_second_test!(dfpn_bench_second_question_124, 124);
dfpn_second_test!(dfpn_bench_second_question_125, 125);
dfpn_second_test!(dfpn_bench_second_question_126, 126);
dfpn_second_test!(dfpn_bench_second_question_127, 127);
dfpn_second_test!(dfpn_bench_second_question_128, 128);
dfpn_second_test!(dfpn_bench_second_question_129, 129);
dfpn_second_test!(dfpn_bench_second_question_130, 130);
dfpn_second_test!(dfpn_bench_second_question_131, 131);
dfpn_second_test!(dfpn_bench_second_question_132, 132);
dfpn_second_test!(dfpn_bench_second_question_133, 133);
dfpn_second_test!(dfpn_bench_second_question_134, 134);
dfpn_second_test!(dfpn_bench_second_question_135, 135);
dfpn_second_test!(dfpn_bench_second_question_136, 136);
dfpn_second_test!(dfpn_bench_second_question_137, 137);
dfpn_second_test!(dfpn_bench_second_question_138, 138);
dfpn_second_test!(dfpn_bench_second_question_139, 139);
dfpn_second_test!(dfpn_bench_second_question_140, 140);
dfpn_second_test!(dfpn_bench_second_question_141, 141);
dfpn_second_test!(dfpn_bench_second_question_142, 142);
dfpn_second_test!(dfpn_bench_second_question_143, 143);
dfpn_second_test!(dfpn_bench_second_question_144, 144);
dfpn_second_test!(dfpn_bench_second_question_145, 145);
dfpn_second_test!(dfpn_bench_second_question_146, 146);
dfpn_second_test!(dfpn_bench_second_question_147, 147);

/// aigoma.json デバッグテスト
#[test]
#[ignore]
fn dfpn_debug_aigoma() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("aigoma.json");
    let pos = load_question(&path);

    println!("=== aigoma.json デバッグ ===");
    print_position(&pos);

    // ▲5一飛成 を指す（飛車は5三にある）
    let rook_move = Move::normal(
        Square::new(5, 3), // from: 5三
        Square::new(5, 1), // to: 5一
        true,              // 成り
        PieceKind::Rook,
    );

    let mut pos_after = pos.clone();
    pos_after.make_move(rook_move);

    println!("\n=== ▲4二竜 後の盤面 ===");
    print_position(&pos_after);

    // 王手されているか
    let in_check = pos_after.is_in_check(Color::Gote);
    println!("後手王手されている: {}", in_check);

    // 王手回避手を列挙
    let evasions = pos_after.generate_check_evasions();
    println!("王手回避手の数: {}", evasions.len());

    for ev in &evasions {
        let is_king_move = if let Some(from) = ev.from {
            pos_after.piece_at(from).map_or(false, |p| p.kind == PieceKind::King)
        } else {
            false
        };

        let futile = if is_king_move {
            false
        } else {
            pos_after.is_futile_interposition(ev)
        };

        let move_desc = if let Some(drop_kind) = ev.drop_piece {
            format!("△{}{}打", ev.to.to_japanese(), drop_kind.to_kanji())
        } else if let Some(from) = ev.from {
            let kind = pos_after.piece_at(from).map(|p| p.kind.to_kanji()).unwrap_or("?");
            let promo = if ev.promotion { "成" } else { "" };
            format!("△{}{}{} (from {})", ev.to.to_japanese(), kind, promo, from.to_japanese())
        } else {
            format!("?")
        };

        println!(
            "  {} | king_move={} futile={}",
            move_desc, is_king_move, futile
        );

        // 非無駄合いの場合、詳細をダンプ
        if !is_king_move && !futile {
            println!("    *** NOT FUTILE - investigating ***");

            let mut test_pos = pos_after.clone();
            test_pos.make_move(*ev);
            println!("    合駒後の盤面:");
            print_position(&test_pos);

            // 攻め方が取り返せるか
            let capture_sq = ev.to;
            let is_attacked = test_pos.is_attacked(capture_sq, Color::Sente);
            println!("    {}に先手の利き: {}", capture_sq.to_japanese(), is_attacked);

            // 取り返し手を列挙
            let legal_moves = test_pos.generate_legal_moves();
            let recaptures: Vec<_> = legal_moves.iter().filter(|m| m.to == capture_sq).collect();
            println!("    取り返し手: {} 個", recaptures.len());
            for rc in &recaptures {
                let mut test2 = test_pos.clone();
                test2.make_move(**rc);
                let gives_check = test2.is_in_check(Color::Gote);
                let is_mate = if gives_check { test2.is_checkmate() } else { false };
                let rc_desc = if let Some(from) = rc.from {
                    let kind = test_pos.piece_at(from).map(|p| p.kind.to_kanji()).unwrap_or("?");
                    format!("▲{}{}", rc.to.to_japanese(), kind)
                } else {
                    format!("▲{}打", rc.to.to_japanese())
                };
                println!(
                    "      {} -> check={} mate={}",
                    rc_desc, gives_check, is_mate
                );
                if gives_check && !is_mate {
                    let ev2 = test2.generate_check_evasions();
                    println!("        回避手: {} 個", ev2.len());
                    for e2 in &ev2 {
                        let e2_desc = if let Some(dk) = e2.drop_piece {
                            format!("△{}{}打", e2.to.to_japanese(), dk.to_kanji())
                        } else if let Some(from) = e2.from {
                            let kind = test2.piece_at(from).map(|p| p.kind.to_kanji()).unwrap_or("?");
                            format!("△{}{}", e2.to.to_japanese(), kind)
                        } else {
                            "?".to_string()
                        };
                        let is_king2 = if let Some(from) = e2.from {
                            test2.piece_at(from).map_or(false, |p| p.kind == PieceKind::King)
                        } else {
                            false
                        };
                        let futile2 = if is_king2 { false } else { test2.is_futile_interposition(e2) };
                        println!("          {} king={} futile={}", e2_desc, is_king2, futile2);
                    }
                }
            }
        }
    }

    // all_effectively_checkmate のチェック
    let meta_after = MetaPosition { positions: vec![pos_after.clone()] };
    let aec = meta_after.all_effectively_checkmate();
    println!("\nall_effectively_checkmate: {}", aec);

    // ソルバーの結果
    println!("\n=== ソルバー実行 ===");
    let meta = MetaPosition::new(pos.clone());
    let cancel = cancel_after(120);
    let mut solver = TsuitateDfpnSolver::new(50_000_000, cancel);
    let result = solver.solve_to_solution(&meta, false);
    println!(
        "Result: found={}, nodes={}, dominance_hits={}, msg={}",
        result.found,
        solver.nodes_searched,
        solver.dominance_hits,
        result.message,
    );
    println!(
        "  Diagnostics: mid_and_calls={}, expand_defense_calls={}, replay_attempts={}, replay_full={}, replay_partial={}, replay_fail={}",
        solver.mid_and_calls,
        solver.expand_defense_calls,
        solver.proof_replay_attempts,
        solver.proof_replay_full_success,
        solver.proof_replay_partial,
        solver.proof_replay_fail,
    );
    if let Some(ref tree) = result.tree {
        println!("  解の手順木:");
        print_solution_tree(tree, 2);
    }
}

/// aigoma2.json デバッグテスト（取った駒を使わないと詰まない問題）
#[test]
#[ignore]
fn dfpn_debug_aigoma2() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("aigoma2.json");
    let pos = load_question(&path);

    println!("=== aigoma2.json デバッグ ===");
    print_position(&pos);

    let meta = MetaPosition::new(pos.clone());
    let cancel = cancel_after(120);
    let mut solver = TsuitateDfpnSolver::new(50_000_000, cancel);
    let result = solver.solve_to_solution(&meta, false);
    println!(
        "Result: found={}, nodes={}, dominance_hits={}, msg={}",
        result.found,
        solver.nodes_searched,
        solver.dominance_hits,
        result.message,
    );
    if let Some(ref tree) = result.tree {
        println!("  解の手順木:");
        print_solution_tree(tree, 2);
    }
}

/// aigoma_93.json デバッグテスト（飛車を9三に配置、合駒マスが増加）
#[test]
#[ignore]
fn dfpn_debug_aigoma_93() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("aigoma_93.json");
    let pos = load_question(&path);

    println!("=== aigoma_93.json デバッグ ===");
    print_position(&pos);

    let meta = MetaPosition::new(pos.clone());
    let cancel = cancel_after(100);
    let mut solver = TsuitateDfpnSolver::new(50_000_000, cancel);
    let result = solver.solve_to_solution(&meta, false);
    println!(
        "Result: found={}, nodes={}, dominance_hits={}, msg={}",
        result.found,
        solver.nodes_searched,
        solver.dominance_hits,
        result.message,
    );
    if let Some(ref tree) = result.tree {
        println!("  解の手順木:");
        print_solution_tree(tree, 2);
    }
}

/// aigoma_63.json デバッグテスト（飛車を6三に配置、合駒マスが1つ増加）
#[test]
#[ignore]
fn dfpn_debug_aigoma_63() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("aigoma_63.json");
    let pos = load_question(&path);

    println!("=== aigoma_63.json デバッグ ===");
    print_position(&pos);

    let meta = MetaPosition::new(pos.clone());
    let cancel = cancel_after(120);
    let mut solver = TsuitateDfpnSolver::new(50_000_000, cancel);
    let result = solver.solve_to_solution(&meta, false);
    println!(
        "Result: found={}, nodes={}, dominance_hits={}, msg={}",
        result.found,
        solver.nodes_searched,
        solver.dominance_hits,
        result.message,
    );
    if let Some(ref tree) = result.tree {
        println!("  解の手順木:");
        print_solution_tree(tree, 2);
    }
}

/// aigoma_73.json デバッグテスト（飛車を7三に配置、合駒マスが2つ増加）
#[test]
#[ignore]
fn dfpn_debug_aigoma_73() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("aigoma_73.json");
    let pos = load_question(&path);

    println!("=== aigoma_73.json デバッグ ===");
    print_position(&pos);

    let meta = MetaPosition::new(pos.clone());
    let cancel = cancel_after(120);
    let mut solver = TsuitateDfpnSolver::new(50_000_000, cancel);
    let result = solver.solve_to_solution(&meta, false);
    println!(
        "Result: found={}, nodes={}, dominance_hits={}, msg={}",
        result.found,
        solver.nodes_searched,
        solver.dominance_hits,
        result.message,
    );
    if let Some(ref tree) = result.tree {
        println!("  解の手順木:");
        print_solution_tree(tree, 2);
    }
}

/// 全問一括ベンチマーク（サマリー表付き）
/// cargo test --release --test dfpn_benchmark_tests dfpn_bench_all -- --ignored --nocapture
#[test]
#[ignore]
fn dfpn_bench_all_questions() {
    run_all_questions(1, 147, false);
}

/// 全問一括ベンチマーク（最短経路探索あり）
/// cargo test --release --test dfpn_benchmark_tests dfpn_bench_all_shortest -- --ignored --nocapture
#[test]
#[ignore]
fn dfpn_bench_all_shortest_questions() {
    run_all_questions(1, 147, true);
}

// === 分割ベンチマーク（通常） ===

macro_rules! dfpn_bench_part {
    ($name:ident, $start:expr, $end:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_all_questions($start, $end, false); }
    };
}

dfpn_bench_part!(dfpn_bench_part1_questions, 1, 30);
dfpn_bench_part!(dfpn_bench_part2_questions, 31, 60);
dfpn_bench_part!(dfpn_bench_part3_questions, 61, 90);
dfpn_bench_part!(dfpn_bench_part4_questions, 91, 120);
dfpn_bench_part!(dfpn_bench_part5_questions, 121, 147);

// === 分割ベンチマーク（最短経路探索あり） ===

macro_rules! dfpn_bench_shortest_part {
    ($name:ident, $start:expr, $end:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_all_questions($start, $end, true); }
    };
}

dfpn_bench_shortest_part!(dfpn_bench_shortest_part1_questions, 1, 30);
dfpn_bench_shortest_part!(dfpn_bench_shortest_part2_questions, 31, 60);
dfpn_bench_shortest_part!(dfpn_bench_shortest_part3_questions, 61, 90);
dfpn_bench_shortest_part!(dfpn_bench_shortest_part4_questions, 91, 120);
dfpn_bench_shortest_part!(dfpn_bench_shortest_part5_questions, 121, 147);

fn run_all_questions(from: u32, to: u32, find_shortest: bool) {
    let dir = sample_questions_dir();
    let node_limit: u64 = NODE_LIMIT;
    let time_limit_secs: u64 = TIME_LIMIT;

    let mode_label = if find_shortest { "最短経路あり" } else { "通常" };
    let range_label = if from == 1 && to == 147 {
        "全問".to_string()
    } else {
        format!("問題{}-{}", from, to)
    };
    println!(
        "=== 衝立df-pn ベンチマーク [{}] {} (node_limit={}, time_limit={}s) ===\n",
        mode_label, range_label, node_limit, time_limit_secs
    );

    struct Result {
        number: u32,
        found: bool,
        depth: u32,
        time_secs: f64,
        nodes: u64,
    }

    let mut results = Vec::new();

    for number in from..=to {
        let path = dir.join(format!("{}.json", number));
        if !path.exists() {
            println!("問題{}: ファイルが見つかりません", number);
            continue;
        }

        let pos = load_question(&path);
        let meta = MetaPosition::new(pos);
        let cancel = cancel_after(time_limit_secs);

        let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

        let start = Instant::now();
        let result = solver.solve_to_solution(&meta, find_shortest);
        let elapsed = start.elapsed();

        let depth = result.tree.as_ref().map_or(0, |t| t.max_moves());
        let time_secs = elapsed.as_secs_f64();
        let nodes = solver.nodes_searched;

        println!(
            "問題{:3}: found={:<5} depth={:2} time={:8.3}s nodes={:>12} | {}",
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

    println!("\n=== サマリー [{}] {} ===", mode_label, range_label);
    println!("解けた問題: {}/{}", solved, total);
    println!("合計時間: {:.3}s", total_time);
    println!("合計ノード数: {}", total_nodes);
    println!();
    println!(
        "{:<8} {:<8} {:<6} {:<12} {:<14}",
        "問題", "結果", "手数", "時間(s)", "ノード数"
    );
    println!("{}", "-".repeat(52));
    for r in &results {
        println!(
            "{:<8} {:<8} {:<6} {:<12.3} {:<14}",
            r.number,
            if r.found { "OK" } else { "NG" },
            if r.found {
                format!("{}", r.depth)
            } else {
                "-".to_string()
            },
            r.time_secs,
            r.nodes,
        );
    }

    // Markdown ファイル出力
    let base = if find_shortest {
        "dfpn-benchmark-results-shortest"
    } else {
        "dfpn-benchmark-results"
    };
    let filename = if from == 1 && to == 147 {
        format!("{}.md", base)
    } else {
        format!("{}-{}-{}.md", base, from, to)
    };
    let output_path = benchmark_output_dir().join(&filename);

    let now = chrono::Local::now();
    let mut md = String::new();
    md.push_str(&format!("# 衝立df-pn ベンチマーク結果 [{}] {}\n\n", mode_label, range_label));
    md.push_str(&format!(
        "- 実行日時: {}\n",
        now.format("%Y-%m-%d %H:%M:%S")
    ));
    md.push_str(&format!("- モード: {}\n", mode_label));
    md.push_str(&format!("- 範囲: {}\n", range_label));
    md.push_str(&format!("- ノード上限: {}\n", node_limit));
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
            if r.found {
                format!("{}", r.depth)
            } else {
                "-".to_string()
            },
            r.time_secs,
            r.nodes,
        ));
    }

    std::fs::write(&output_path, &md)
        .unwrap_or_else(|e| panic!("Failed to write {}: {}", output_path.display(), e));
    println!(
        "\nベンチマーク結果を {} に出力しました",
        output_path.display()
    );
}

/// 全問一括ベンチマーク（余詰めチェック）
/// cargo test --release --test dfpn_benchmark_tests dfpn_bench_all_second -- --ignored --nocapture
#[test]
#[ignore]
fn dfpn_bench_all_second_questions() {
    run_all_second_questions(1, 147);
}

// === 分割ベンチマーク（余詰めチェック） ===

macro_rules! dfpn_bench_second_part {
    ($name:ident, $start:expr, $end:expr) => {
        #[test]
        #[ignore]
        fn $name() { run_all_second_questions($start, $end); }
    };
}

dfpn_bench_second_part!(dfpn_bench_second_part1_questions, 1, 30);
dfpn_bench_second_part!(dfpn_bench_second_part2_questions, 31, 60);
dfpn_bench_second_part!(dfpn_bench_second_part3_questions, 61, 90);
dfpn_bench_second_part!(dfpn_bench_second_part4_questions, 91, 120);
dfpn_bench_second_part!(dfpn_bench_second_part5_questions, 121, 147);

fn run_all_second_questions(from: u32, to: u32) {
    let dir = sample_questions_dir();
    let node_limit: u64 = NODE_LIMIT;
    let time_limit_secs: u64 = TIME_LIMIT;

    let range_label = if from == 1 && to == 147 {
        "全問".to_string()
    } else {
        format!("問題{}-{}", from, to)
    };
    println!(
        "=== 衝立df-pn ベンチマーク [余詰めチェック] {} (node_limit={}, time_limit={}s) ===\n",
        range_label, node_limit, time_limit_secs
    );

    #[allow(dead_code)]
    struct Result {
        number: u32,
        found: bool,
        depth: u32,
        has_second: bool,
        second_depth: u32,
        kizu_count: usize,
        time_secs: f64,
        nodes: u64,
        message: String,
    }

    let mut results = Vec::new();

    for number in from..=to {
        let path = dir.join(format!("{}.json", number));
        if !path.exists() {
            continue;
        }

        let pos = match std::panic::catch_unwind(|| load_question(&path)) {
            Ok(pos) => pos,
            Err(_) => {
                println!("問題{:3}: ファイル読み込みエラー（スキップ）", number);
                continue;
            }
        };
        let meta = MetaPosition::new(pos);
        let cancel = cancel_after(time_limit_secs);

        let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);

        let start = Instant::now();
        let result = solver.solve_to_solution_with_second(&meta, false);
        let elapsed = start.elapsed();

        let depth = result.tree.as_ref().map_or(0, |t| t.max_moves());
        let has_second = result.second_tree.is_some();
        let second_depth = result.second_tree.as_ref().map_or(0, |t| t.max_moves());
        let kizu_count = result.kizu_trees.len();
        let time_secs = elapsed.as_secs_f64();
        let nodes = solver.nodes_searched;

        let second_info = if has_second {
            format!("余詰め{}手", second_depth)
        } else if result.found {
            if kizu_count > 0 {
                format!("完全作(キズ{})", kizu_count)
            } else {
                "完全作".to_string()
            }
        } else {
            "-".to_string()
        };

        println!(
            "問題{:3}: found={:<5} depth={:2} second={:<14} time={:8.3}s nodes={:>12} | {}",
            number, result.found, depth, second_info, time_secs, nodes, result.message,
        );

        results.push(Result {
            number,
            found: result.found,
            depth,
            has_second,
            second_depth,
            kizu_count,
            time_secs,
            nodes,
            message: result.message.clone(),
        });
    }

    // サマリー
    let total = results.len();
    let solved = results.iter().filter(|r| r.found).count();
    let perfect = results.iter().filter(|r| r.found && !r.has_second).count();
    let has_second = results.iter().filter(|r| r.has_second).count();
    let has_kizu = results.iter().filter(|r| r.kizu_count > 0).count();
    let total_time: f64 = results.iter().map(|r| r.time_secs).sum();
    let total_nodes: u64 = results.iter().map(|r| r.nodes).sum();

    println!("\n=== サマリー [余詰めチェック] {} ===", range_label);
    println!("解けた問題: {}/{}", solved, total);
    println!("完全作: {}", perfect);
    println!("余詰めあり: {}", has_second);
    println!("キズあり: {}", has_kizu);
    println!("合計時間: {:.3}s", total_time);
    println!("合計ノード数: {}", total_nodes);
    println!();
    println!(
        "{:<8} {:<8} {:<6} {:<10} {:<10} {:<6} {:<12} {:<14}",
        "問題", "結果", "手数", "余詰め", "余詰手数", "キズ", "時間(s)", "ノード数"
    );
    println!("{}", "-".repeat(80));
    for r in &results {
        let result_str = if r.found { "OK" } else { "NG" };
        let depth_str = if r.found { format!("{}", r.depth) } else { "-".to_string() };
        let second_str = if r.has_second {
            "あり"
        } else if r.found {
            "なし"
        } else {
            "-"
        };
        let second_depth_str = if r.has_second {
            format!("{}", r.second_depth)
        } else {
            "-".to_string()
        };
        let kizu_str = if r.kizu_count > 0 {
            format!("{}", r.kizu_count)
        } else {
            "-".to_string()
        };
        println!(
            "{:<8} {:<8} {:<6} {:<10} {:<10} {:<6} {:<12.3} {:<14}",
            r.number, result_str, depth_str, second_str, second_depth_str,
            kizu_str, r.time_secs, r.nodes,
        );
    }

    // Markdown ファイル出力
    let filename = if from == 1 && to == 147 {
        "dfpn-benchmark-results-second.md".to_string()
    } else {
        format!("dfpn-benchmark-results-second-{}-{}.md", from, to)
    };
    let output_path = benchmark_output_dir().join(&filename);

    let now = chrono::Local::now();
    let mut md = String::new();
    md.push_str(&format!("# 衝立df-pn ベンチマーク結果 [余詰めチェック] {}\n\n", range_label));
    md.push_str(&format!(
        "- 実行日時: {}\n",
        now.format("%Y-%m-%d %H:%M:%S")
    ));
    md.push_str(&format!("- 範囲: {}\n", range_label));
    md.push_str(&format!("- ノード上限: {}\n", node_limit));
    md.push_str(&format!("- 制限時間: {}秒/問\n", time_limit_secs));
    md.push_str(&format!("- 解けた問題: {}/{}\n", solved, total));
    md.push_str(&format!("- 完全作: {}\n", perfect));
    md.push_str(&format!("- 余詰めあり: {}\n", has_second));
    md.push_str(&format!("- キズあり: {}\n", has_kizu));
    md.push_str(&format!("- 合計時間: {:.3}秒\n", total_time));
    md.push_str(&format!("- 合計ノード数: {}\n\n", total_nodes));

    md.push_str("| 問題 | 結果 | 手数 | 余詰め | 余詰手数 | キズ | 時間(秒) | ノード数 |\n");
    md.push_str("|-----:|:----:|-----:|:------:|---------:|-----:|---------:|---------:|\n");
    for r in &results {
        let result_str = if r.found { "OK" } else { "NG" };
        let depth_str = if r.found { format!("{}", r.depth) } else { "-".to_string() };
        let second_str = if r.has_second {
            "あり"
        } else if r.found {
            "なし"
        } else {
            "-"
        };
        let second_depth_str = if r.has_second {
            format!("{}", r.second_depth)
        } else {
            "-".to_string()
        };
        let kizu_str = if r.kizu_count > 0 {
            format!("{}", r.kizu_count)
        } else {
            "-".to_string()
        };
        md.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {:.3} | {} |\n",
            r.number, result_str, depth_str, second_str, second_depth_str,
            kizu_str, r.time_secs, r.nodes,
        ));
    }

    std::fs::write(&output_path, &md)
        .unwrap_or_else(|e| panic!("Failed to write {}: {}", output_path.display(), e));
    println!(
        "\nベンチマーク結果を {} に出力しました",
        output_path.display()
    );
}

/// 単手数で遅い問題の診断テスト
/// cargo test --release --test dfpn_benchmark_tests dfpn_diag_slow -- --ignored --nocapture
#[test]
#[ignore]
fn dfpn_diag_slow_short_problems() {
    let slow_problems = [4, 14, 25, 27, 31, 35, 38];
    let dir = sample_questions_dir();

    println!("{:<6} {:<6} {:<10} {:<10} {:<10} {:<10} {:<12} {:<12} {:<12} {:<12}",
        "問題", "手数", "時間(ms)", "nodes", "and_calls", "exp_def",
        "max_meta", "gen_cand(ms)", "exp_def(ms)", "aec(ms)");
    println!("{}", "-".repeat(120));

    for &num in &slow_problems {
        let path = dir.join(format!("{}.json", num));
        let pos = load_question(&path);
        let meta = MetaPosition::new(pos);
        let cancel = cancel_after(120);
        let mut solver = TsuitateDfpnSolver::new(50_000_000, cancel);

        let start = std::time::Instant::now();
        let result = solver.solve_to_solution(&meta, false);
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;

        let depth = result.tree.as_ref().map_or(0, |t| t.max_moves());

        println!("{:<6} {:<6} {:<10.1} {:<10} {:<10} {:<10} {:<12} {:<12.1} {:<12.1} {:<12.1}",
            num,
            depth,
            elapsed_ms,
            solver.nodes_searched,
            solver.mid_and_calls,
            solver.expand_defense_calls,
            solver.max_meta_size,
            solver.gen_candidates_nanos as f64 / 1_000_000.0,
            solver.expand_defense_nanos as f64 / 1_000_000.0,
            solver.aec_nanos as f64 / 1_000_000.0,
        );
    }
}
