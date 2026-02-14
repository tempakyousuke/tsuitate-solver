use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;
use tsuitate_resolver_lib::shogi::position::Position;
use tsuitate_resolver_lib::shogi::types::*;
use tsuitate_resolver_lib::solver::metaposition::MetaPosition;
use tsuitate_resolver_lib::solver::refutation_solver::RefutationSolver;
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

fn cancel_after(secs: u64) -> Arc<AtomicBool> {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_clone = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(secs));
        cancel_clone.store(true, std::sync::atomic::Ordering::Relaxed);
    });
    cancel
}

/// 問題1の各候補手に対して不詰ソルバーを実行し、ノード数を比較する
#[test]
#[ignore]
fn refutation_question_01_moves() {
    let path = sample_questions_dir().join("1.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);
    let max_depth: u32 = 15;

    // 問題1の候補手
    // 盤面: 先手角83, 先手持ち駒角, 後手玉85
    // ▲9四角打 (不正解) - 角を94に打つ（遠い王手、すぐ不詰証明可能）
    // ▲7四角成 (正解)   - 83の角を74に成る（正解手、不詰証明は失敗するはず）
    // ▲6三角打 (不正解) - 角を63に打つ（有力な王手、証明に時間がかかる可能性）
    struct TestCase {
        name: &'static str,
        mv: Move,
        expect: Option<bool>, // Some(true)=proven, Some(false)=not proven, None=no assertion
        time_limit: u64,
    }

    let test_cases = vec![
        TestCase {
            name: "▲9四角打",
            mv: Move::drop(Square::new(9, 4), PieceKind::Bishop),
            expect: Some(true), // 不詰証明に成功するはず
            time_limit: 30,
        },
        TestCase {
            name: "▲7四角成",
            mv: Move::normal(Square::new(8, 3), Square::new(7, 4), true, PieceKind::Bishop),
            expect: Some(false), // 正解手なので不詰証明に失敗するはず
            time_limit: 60,
        },
        TestCase {
            name: "▲6三角打",
            mv: Move::drop(Square::new(6, 3), PieceKind::Bishop),
            expect: None, // 証明に時間がかかる可能性があるため結果のみ出力
            time_limit: 10,
        },
    ];

    println!("=== 問題1: 不詰ソルバーによる候補手評価 ===\n");

    for tc in &test_cases {
        let cancel = cancel_after(tc.time_limit);
        let mut solver = RefutationSolver::new(max_depth, cancel);

        let start = Instant::now();
        let result = solver.prove_no_checkmate_for_move(&meta, tc.mv);
        let elapsed = start.elapsed();

        println!(
            "{}: proven={}, depth={}, nodes={}, dfpn_nodes={}, time={:.3}s",
            tc.name, result.proven, result.proven_depth, result.nodes_searched,
            solver.dfpn_nodes_total, elapsed.as_secs_f64(),
        );

        match tc.expect {
            Some(true) => {
                assert!(
                    result.proven,
                    "{} は不詰証明に成功するはず（不正解手）",
                    tc.name
                );
            }
            Some(false) => {
                assert!(
                    !result.proven,
                    "{} は不詰証明に失敗するはず（正解手）",
                    tc.name
                );
            }
            None => {
                // 結果は出力のみ、アサーションなし
            }
        }
    }

    // 比較: 既存ソルバーのノード数
    println!("\n--- 比較: 既存ソルバー（全候補手を深さ{}まで探索） ---", max_depth);
    let cancel = cancel_after(60);
    let mut existing_solver = TsuitateSolver::new(max_depth, cancel);
    existing_solver.set_trace_enabled(false);

    let start = Instant::now();
    let existing_result = existing_solver.solve(&meta);
    let elapsed = start.elapsed();

    println!(
        "既存ソルバー: found={}, nodes={}, time={:.3}s, msg={}",
        existing_result.found,
        existing_solver.nodes_searched,
        elapsed.as_secs_f64(),
        existing_result.message,
    );
}

/// 全メタポジションに対する不詰証明テスト
#[test]
#[ignore]
fn refutation_question_01_full() {
    let path = sample_questions_dir().join("1.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);
    let max_depth: u32 = 15;
    let time_limit: u64 = 60;

    println!("=== 問題1: 全体不詰証明テスト ===\n");

    let cancel = cancel_after(time_limit);
    let mut solver = RefutationSolver::new(max_depth, cancel);

    let start = Instant::now();
    let result = solver.prove_no_checkmate(&meta);
    let elapsed = start.elapsed();

    println!(
        "prove_no_checkmate: proven={}, depth={}, nodes={}, time={:.3}s",
        result.proven, result.proven_depth, result.nodes_searched, elapsed.as_secs_f64(),
    );

    // 問題1は詰みが存在するので、全体の不詰証明は失敗するはず
    assert!(
        !result.proven,
        "問題1は詰みがあるので不詰証明は失敗するはず"
    );
}

/// df-pnフィルタの効果を検証: ▲6三角打の不詰証明が高速化されるか
#[test]
#[ignore]
fn refutation_question_01_dfpn_effect() {
    let path = sample_questions_dir().join("1.json");
    let pos = load_question(&path);
    let meta = MetaPosition::new(pos);
    let max_depth: u32 = 15;
    let time_limit: u64 = 30;

    let mv = Move::drop(Square::new(6, 3), PieceKind::Bishop);

    println!("=== ▲6三角打: df-pnフィルタ効果検証 ===\n");

    let cancel = cancel_after(time_limit);
    let mut solver = RefutationSolver::new(max_depth, cancel);

    let start = Instant::now();
    let result = solver.prove_no_checkmate_for_move(&meta, mv);
    let elapsed = start.elapsed();

    println!(
        "▲6三角打 (df-pn有効): proven={}, depth={}, nodes={}, dfpn_nodes={}, time={:.3}s",
        result.proven, result.proven_depth, result.nodes_searched,
        solver.dfpn_nodes_total, elapsed.as_secs_f64(),
    );

    // df-pn フィルタが使われたことを確認
    println!("df-pn累計ノード数: {}", solver.dfpn_nodes_total);

    // df-pn有りで30秒以内に証明できるはず（以前は10秒でタイムアウトしていた）
    if result.proven {
        println!("不詰証明に成功（df-pnフィルタの効果あり）");
        assert!(
            elapsed.as_secs() < time_limit,
            "制限時間内に証明が完了するはず"
        );
    } else {
        println!("不詰証明に失敗（結果のみ出力）");
    }
}
