//! 挑戦モード（Webサイト側）の応答時間ベンチマーク。
//!
//! サイトは1手ごとに「後手の各観測分岐について、先手は残り N 手以内に
//! 詰みを強制できるか」を問い合わせ（--solve-meta / --solve-meta-server）、
//! 最も粘れる分岐を選ぶ。本ベンチはその1手あたりの応答時間を測る。
//!
//! 実行:
//!   cargo test --release --test challenge_bench challenge_bench_all -- --ignored --nocapture

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::defense::{solve_meta_query_with, MetaQueryResult};
use tsuitate_solver_lib::solver::metaposition::MetaPosition;
use tsuitate_solver_lib::solver::solution::{Observation, SolutionNode};
use tsuitate_solver_lib::solver::tsuitate_dfpn::TsuitateDfpnSolver;

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
            used_counts[hand_kind_index(base_kind)] += 1;
        }
    }
    for hp in &q.sente_hand {
        let kind = parse_piece_kind(&hp.kind);
        for _ in 0..hp.count {
            pos.sente_hand.add(kind);
        }
        used_counts[hand_kind_index(kind)] += hp.count;
    }
    for &(kind, max) in &MAX_PIECES {
        let remaining = max.saturating_sub(used_counts[hand_kind_index(kind)]);
        for _ in 0..remaining {
            pos.gote_hand.add(kind);
        }
    }
    pos.side_to_move = Color::Sente;
    pos
}

/// 解の手順木の MoveData を、情報集合内で実際に指せる Move に対応づける
fn resolve_move(meta: &MetaPosition, md: &tsuitate_solver_lib::solver::solution::MoveData) -> Option<Move> {
    for pos in &meta.positions {
        for mv in pos.generate_legal_moves() {
            let from_ok = mv.from.map(|s| (s.file, s.rank)) == md.from_file.zip(md.from_rank);
            let drop_ok = mv.drop_piece.map(|k| k.to_kanji().to_string()) == md.drop_piece;
            if from_ok
                && drop_ok
                && mv.to.file == md.to_file
                && mv.to.rank == md.to_rank
                && mv.promotion == md.promotion
            {
                return Some(mv);
            }
        }
    }
    None
}

#[allow(dead_code)]
struct TurnStat {
    queries: usize,
    positions: usize,
    nodes: u64,
    elapsed: Duration,
}

/// 1問の挑戦モードを最後まで（あるいは max_turns まで）シミュレートし、
/// 1手ごとのサイト側応答時間を返す
fn simulate(path: &std::path::Path, node_limit: u64, scan_node_limit: u64) -> Vec<TurnStat> {
    let pos = load_question(path);
    let root = MetaPosition::new(pos);

    // 攻め方（＝挑戦者）の指し手は解の手順木から採る（計測対象外）
    let cancel = Arc::new(AtomicBool::new(false));
    let mut oracle = TsuitateDfpnSolver::new(50_000_000, cancel.clone());
    let data = oracle.solve_to_solution(&root, true);
    let Some(tree) = data.tree else {
        return Vec::new();
    };
    let total_depth = tree_depth(&tree);

    // サイト側（--solve-meta-server 相当）の常駐ソルバー
    let mut server = TsuitateDfpnSolver::new(u64::MAX, Arc::new(AtomicBool::new(false)));

    let mut stats = Vec::new();
    let mut meta = root.clone();
    let mut node = tree;
    let mut remaining = total_depth;

    loop {
        let SolutionNode::AttackMove { mv: md, branches, .. } = &node else {
            break;
        };
        let Some(mv) = resolve_move(&meta, md) else {
            break;
        };
        let (legal, illegal) = meta.apply_attack_move_split(mv);
        if legal.is_empty() {
            break;
        }

        // ---- ここからサイト側の応答（計測対象） ----
        let t0 = Instant::now();
        let mut groups = legal.expand_defense_moves(mv);
        // 反則枝（プローブ手が一部の盤面でだけ指せた場合）もサイトは同じように
        // 問い合わせる。玉方の手は進まないので深さ予算は据え置き
        if !illegal.is_empty() {
            groups.push((Observation::Illegal, illegal.clone()));
        }
        let mut queries = 0usize;
        let mut positions = 0usize;
        let start_nodes = server.nodes_searched;
        let d_aec = server.aec_nanos;
        let d_exp = server.expand_defense_nanos;
        let d_gen = server.gen_candidates_nanos;
        let d_rep = server.replay_nanos;
        let mut evaluated: Vec<(usize, Option<u32>)> = Vec::new();
        for (i, (obs, group)) in groups.iter().enumerate() {
            if matches!(obs, Observation::Checkmate) {
                continue;
            }
            if group.all_effectively_checkmate() {
                evaluated.push((i, Some(1)));
                continue;
            }
            queries += 1;
            positions += group.positions.len();
            // 反則なら玉方は指していないので残り手数は減らない
            let depth_limit = if matches!(obs, Observation::Illegal) {
                remaining
            } else {
                remaining.saturating_sub(2)
            };
            let outcome = solve_meta_query_with(
                &mut server,
                group.positions.clone(),
                depth_limit,
                node_limit,
                scan_node_limit,
            );
            let score = match outcome.result {
                MetaQueryResult::Proven => outcome.proven_depth,
                _ => None, // 逃れ = 最も粘れる
            };
            evaluated.push((i, score));
        }
        server.trim_transient_caches();
        let elapsed = t0.elapsed();
        // ---- ここまで ----

        if std::env::var("CHALLENGE_VERBOSE").is_ok() {
            eprintln!(
                "  turn: {:?} q={} nodes={} aec={:.1}ms exp={:.1}ms gen={:.1}ms replay={:.1}ms",
                elapsed,
                queries,
                server.nodes_searched - start_nodes,
                (server.aec_nanos - d_aec) as f64 / 1e6,
                (server.expand_defense_nanos - d_exp) as f64 / 1e6,
                (server.gen_candidates_nanos - d_gen) as f64 / 1e6,
                (server.replay_nanos - d_rep) as f64 / 1e6,
            );
        }
        stats.push(TurnStat {
            queries,
            positions,
            nodes: server.nodes_searched - start_nodes,
            elapsed,
        });

        // 最も粘れる分岐を選ぶ（逃れ > 深い証明）
        let Some(&(best_idx, _)) = evaluated.iter().max_by_key(|(_, score)| match score {
            None => u32::MAX,
            Some(d) => *d,
        }) else {
            break;
        };
        let chosen_obs = groups[best_idx].0.clone();
        meta = groups[best_idx].1.clone();
        if !matches!(chosen_obs, Observation::Illegal) {
            remaining = remaining.saturating_sub(2);
        }

        let Some(branch) = branches.iter().find(|b| b.observation == chosen_obs) else {
            break; // 解の木に無い分岐（サイトなら不正解判定）
        };
        node = (*branch.continuation).clone();
        if matches!(node, SolutionNode::Checkmate { .. }) {
            break;
        }
    }

    stats
}

fn tree_depth(node: &SolutionNode) -> u32 {
    match node {
        SolutionNode::Checkmate { depth, .. } => *depth,
        SolutionNode::AttackMove { branches, .. } => branches
            .iter()
            .map(|b| tree_depth(&b.continuation))
            .max()
            .unwrap_or(0),
    }
}

fn question_path(n: u32) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("sample-questions")
        .join(format!("{}.json", n))
}

fn run(questions: &[u32]) {
    let node_limit: u64 = std::env::var("CHALLENGE_NODE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000_000);
    // 0 = 自動（CLI の既定と同じ）。u64::MAX で無制限
    let scan_node_limit: u64 = std::env::var("CHALLENGE_SCAN_NODE_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    println!("{:>4} {:>6} {:>10} {:>10} {:>12} {:>12}", "問題", "手数", "合計(ms)", "最大(ms)", "総ノード", "最大局面数");
    let mut grand_total = Duration::ZERO;
    let mut grand_max = Duration::ZERO;
    for &q in questions {
        let path = question_path(q);
        if !path.exists() {
            continue;
        }
        let stats = simulate(&path, node_limit, scan_node_limit);
        if stats.is_empty() {
            println!("{:>4} {:>6}", q, "解なし");
            continue;
        }
        let total: Duration = stats.iter().map(|s| s.elapsed).sum();
        let max = stats.iter().map(|s| s.elapsed).max().unwrap();
        let nodes: u64 = stats.iter().map(|s| s.nodes).sum();
        let maxpos = stats.iter().map(|s| s.positions).max().unwrap();
        println!(
            "{:>4} {:>6} {:>10.1} {:>10.1} {:>12} {:>12}",
            q,
            stats.len(),
            total.as_secs_f64() * 1000.0,
            max.as_secs_f64() * 1000.0,
            nodes,
            maxpos
        );
        grand_total += total;
        if max > grand_max {
            grand_max = max;
        }
    }
    println!(
        "合計 {:.1}ms / 1手最大 {:.1}ms",
        grand_total.as_secs_f64() * 1000.0,
        grand_max.as_secs_f64() * 1000.0
    );
}

#[test]
#[ignore]
fn challenge_bench_all() {
    let qs: Vec<u32> = (1..=40).collect();
    run(&qs);
}

#[test]
#[ignore]
fn challenge_bench_pick() {
    let qs: Vec<u32> = std::env::var("CHALLENGE_QUESTIONS")
        .expect("CHALLENGE_QUESTIONS=1,2,3")
        .split(',')
        .map(|s| s.trim().parse().unwrap())
        .collect();
    run(&qs);
}
