//! 衝立詰将棋ソルバーの CLI。
//!
//! Web サイト（tsuitate）等の外部プロセスから、Tauri UI なしで検証を実行するためのバイナリ。
//! 入力は問題 JSON（docs/json-format.md の疎表現）、出力は stdout に JSON 1 行。
//!
//! 使い方:
//!   tsuitate-solver-cli <question.json> [--find-second] [--shortest]
//!                       [--node-limit N] [--timeout-secs N]
//!   tsuitate-solver-cli --solve-meta <request.json> [--node-limit N]
//!
//! --solve-meta は Webサイトの挑戦モード用。情報集合（局面リスト、全て先手番・
//! 持ち駒は両者とも明示）と深さ制限のクエリ列を受け取り、それぞれ
//! 「先手が制限内に詰みを強制できるか」を判定する。決定性を保つため
//! タイムアウトは使わない（--node-limit はクエリごとの上限。既定 2,000,000）。
//! 出力: {"ok":true,"results":[{"result":"proven|disproven|unknown",
//!        "provenDepth":N|null,"nodes":N}, ...]}
//!
//! 終了コード: 0 = 探索実行（found の真偽は JSON を参照）, 2 = 入力不正

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::json;
use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::defense::{solve_meta_query, MetaQueryResult};
use tsuitate_solver_lib::solver::metaposition::MetaPosition;
use tsuitate_solver_lib::solver::tsuitate_dfpn::TsuitateDfpnSolver;

/// 問題 JSON（疎表現）
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

fn parse_piece_kind(s: &str) -> Result<PieceKind, String> {
    Ok(match s {
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
        _ => return Err(format!("Unknown piece kind: {}", s)),
    })
}

fn parse_color(s: &str) -> Result<Color, String> {
    match s {
        "sente" => Ok(Color::Sente),
        "gote" => Ok(Color::Gote),
        _ => Err(format!("Unknown color: {}", s)),
    }
}

fn hand_kind_index(kind: PieceKind) -> Result<usize, String> {
    Ok(match kind {
        PieceKind::Rook => 0,
        PieceKind::Bishop => 1,
        PieceKind::Gold => 2,
        PieceKind::Silver => 3,
        PieceKind::Knight => 4,
        PieceKind::Lance => 5,
        PieceKind::Pawn => 6,
        _ => return Err(format!("Not a hand piece kind: {:?}", kind)),
    })
}

/// 疎 JSON → Position（後手持ち駒は残り駒から自動算出）
fn position_from_question(q: &QuestionJson) -> Result<Position, String> {
    let mut pos = Position::new();
    let mut used_counts = [0u8; 7];

    for bp in &q.board {
        if !(1..=9).contains(&bp.file) || !(1..=9).contains(&bp.rank) {
            return Err(format!("Square out of range: file={}, rank={}", bp.file, bp.rank));
        }
        let kind = parse_piece_kind(&bp.kind)?;
        let color = parse_color(&bp.color)?;
        let sq = Square::new(bp.file, bp.rank);
        if pos.piece_at(sq).is_some() {
            return Err(format!("Duplicate piece at file={}, rank={}", bp.file, bp.rank));
        }
        pos.set_piece(sq, Piece::new(color, kind));

        let base_kind = kind.unpromoted();
        if base_kind != PieceKind::King {
            let idx = hand_kind_index(base_kind)?;
            used_counts[idx] += 1;
        }
    }

    for hp in &q.sente_hand {
        let kind = parse_piece_kind(&hp.kind)?;
        if kind != kind.unpromoted() || kind == PieceKind::King {
            return Err(format!("Invalid hand piece: {}", hp.kind));
        }
        for _ in 0..hp.count {
            pos.sente_hand.add(kind);
        }
        let idx = hand_kind_index(kind)?;
        used_counts[idx] += hp.count;
    }

    // 枚数超過チェック＋後手持ち駒の自動算出
    for &(kind, max) in &MAX_PIECES {
        let idx = hand_kind_index(kind)?;
        if used_counts[idx] > max {
            return Err(format!("Too many pieces of kind {:?}: {} > {}", kind, used_counts[idx], max));
        }
        let remaining = max - used_counts[idx];
        for _ in 0..remaining {
            pos.gote_hand.add(kind);
        }
    }

    if pos.find_king(Color::Gote).is_none() {
        return Err("後手の玉が配置されていません".to_string());
    }

    pos.side_to_move = Color::Sente;
    Ok(pos)
}

struct Args {
    input: String,
    find_second: bool,
    shortest: bool,
    solve_meta: bool,
    node_limit: u64,
    node_limit_given: bool,
    timeout_secs: u64,
}

fn parse_args() -> Result<Args, String> {
    let mut input: Option<String> = None;
    let mut find_second = false;
    let mut shortest = false;
    let mut solve_meta = false;
    let mut node_limit: u64 = 50_000_000;
    let mut node_limit_given = false;
    let mut timeout_secs: u64 = 120;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--find-second" => find_second = true,
            "--shortest" => shortest = true,
            "--solve-meta" => solve_meta = true,
            "--node-limit" => {
                let v = iter.next().ok_or("--node-limit requires a value")?;
                node_limit = v.parse().map_err(|_| format!("Invalid --node-limit: {}", v))?;
                node_limit_given = true;
            }
            "--timeout-secs" => {
                let v = iter.next().ok_or("--timeout-secs requires a value")?;
                timeout_secs = v.parse().map_err(|_| format!("Invalid --timeout-secs: {}", v))?;
            }
            _ if arg.starts_with("--") => return Err(format!("Unknown option: {}", arg)),
            _ => {
                if input.is_some() {
                    return Err("Multiple input files specified".to_string());
                }
                input = Some(arg);
            }
        }
    }

    Ok(Args {
        input: input.ok_or("Usage: tsuitate-solver-cli <question.json> [--find-second] [--shortest] [--node-limit N] [--timeout-secs N] | --solve-meta <request.json> [--node-limit N]")?,
        find_second,
        shortest,
        solve_meta,
        node_limit,
        node_limit_given,
        timeout_secs,
    })
}

/// --solve-meta モードのリクエスト
#[derive(Debug, Deserialize)]
struct SolveMetaRequestJson {
    queries: Vec<MetaQueryJson>,
}

#[derive(Debug, Deserialize)]
struct MetaQueryJson {
    depth_limit: u32,
    positions: Vec<MetaPositionJson>,
}

/// 情報集合の1局面（先手番・持ち駒は両者とも明示）
#[derive(Debug, Deserialize)]
struct MetaPositionJson {
    board: Vec<BoardPiece>,
    #[serde(default)]
    sente_hand: Vec<HandPieceJson>,
    #[serde(default)]
    gote_hand: Vec<HandPieceJson>,
}

/// 明示された持ち駒つきの局面 JSON → Position（自動算出はしない）
fn position_from_meta_json(p: &MetaPositionJson) -> Result<Position, String> {
    let mut pos = Position::new();
    let mut used_counts = [0u8; 7];

    for bp in &p.board {
        if !(1..=9).contains(&bp.file) || !(1..=9).contains(&bp.rank) {
            return Err(format!("Square out of range: file={}, rank={}", bp.file, bp.rank));
        }
        let kind = parse_piece_kind(&bp.kind)?;
        let color = parse_color(&bp.color)?;
        let sq = Square::new(bp.file, bp.rank);
        if pos.piece_at(sq).is_some() {
            return Err(format!("Duplicate piece at file={}, rank={}", bp.file, bp.rank));
        }
        pos.set_piece(sq, Piece::new(color, kind));
        let base_kind = kind.unpromoted();
        if base_kind != PieceKind::King {
            let idx = hand_kind_index(base_kind)?;
            used_counts[idx] += 1;
        }
    }
    for (hand_json, color) in [(&p.sente_hand, Color::Sente), (&p.gote_hand, Color::Gote)] {
        for hp in hand_json.iter() {
            let kind = parse_piece_kind(&hp.kind)?;
            if kind != kind.unpromoted() || kind == PieceKind::King {
                return Err(format!("Invalid hand piece: {}", hp.kind));
            }
            for _ in 0..hp.count {
                match color {
                    Color::Sente => pos.sente_hand.add(kind),
                    Color::Gote => pos.gote_hand.add(kind),
                }
            }
            let idx = hand_kind_index(kind)?;
            used_counts[idx] += hp.count;
        }
    }
    for &(kind, max) in &MAX_PIECES {
        let idx = hand_kind_index(kind)?;
        if used_counts[idx] > max {
            return Err(format!("Too many pieces of kind {:?}: {} > {}", kind, used_counts[idx], max));
        }
    }
    if pos.find_king(Color::Gote).is_none() {
        return Err("後手の玉が配置されていません".to_string());
    }
    pos.side_to_move = Color::Sente;
    Ok(pos)
}

fn run_solve_meta(json_str: &str, node_limit: u64) -> ExitCode {
    let request: SolveMetaRequestJson = match serde_json::from_str(json_str) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to parse solve-meta request: {}", e);
            return ExitCode::from(2);
        }
    };

    let mut results = Vec::new();
    for query in &request.queries {
        let positions: Result<Vec<Position>, String> =
            query.positions.iter().map(position_from_meta_json).collect();
        let positions = match positions {
            Ok(ps) if !ps.is_empty() => ps,
            Ok(_) => {
                eprintln!("Empty positions in query");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("Invalid position: {}", e);
                return ExitCode::from(2);
            }
        };
        let outcome = solve_meta_query(positions, query.depth_limit, node_limit);
        results.push(json!({
            "result": match outcome.result {
                MetaQueryResult::Proven => "proven",
                MetaQueryResult::Disproven => "disproven",
                MetaQueryResult::Unknown => "unknown",
            },
            "provenDepth": outcome.proven_depth,
            "nodes": outcome.nodes,
        }));
    }

    println!("{}", json!({ "ok": true, "results": results }));
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(e) => {
            eprintln!("{}", e);
            return ExitCode::from(2);
        }
    };

    let json_str = match std::fs::read_to_string(&args.input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", args.input, e);
            return ExitCode::from(2);
        }
    };
    if args.solve_meta {
        // 挑戦モードの判定は決定性が必要なためタイムアウトなし・ノード上限のみ。
        // 既定の 50M は求解用なので、未指定なら応答時間を抑えた上限にする
        let node_limit = if args.node_limit_given { args.node_limit } else { 2_000_000 };
        return run_solve_meta(&json_str, node_limit);
    }
    let question: QuestionJson = match serde_json::from_str(&json_str) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Failed to parse {}: {}", args.input, e);
            return ExitCode::from(2);
        }
    };
    let pos = match position_from_question(&question) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("Invalid position: {}", e);
            return ExitCode::from(2);
        }
    };

    // タイムアウト用キャンセルフラグ
    let cancel = Arc::new(AtomicBool::new(false));
    if args.timeout_secs > 0 {
        let cancel_clone = cancel.clone();
        let secs = args.timeout_secs;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            cancel_clone.store(true, Ordering::Relaxed);
        });
    }

    let meta = MetaPosition::new(pos);
    let mut solver = TsuitateDfpnSolver::new(args.node_limit, cancel.clone());
    let result = if args.find_second {
        solver.solve_to_solution_with_second(&meta, args.shortest)
    } else {
        solver.solve_to_solution(&meta, args.shortest)
    };

    let depth = result.tree.as_ref().map(|t| t.max_moves());
    let second_depth = result.second_tree.as_ref().map(|t| t.max_moves());
    // found=false のうち「詰み不存在の証明」ではなく打ち切りだったか
    let unknown = !result.found && result.message.starts_with("探索を打ち切りました");
    let timed_out = unknown && cancel.load(Ordering::Relaxed);

    let output = json!({
        "found": result.found,
        "depth": depth,
        "hasSecondSolution": result.second_tree.is_some(),
        "secondDepth": second_depth,
        "kizuCount": result.kizu_trees.len(),
        "unknown": unknown,
        "timedOut": timed_out,
        "nodesSearched": solver.nodes_searched,
        "message": result.message,
        "tree": result.tree,
        "secondTree": result.second_tree,
    });
    println!("{}", output);

    ExitCode::SUCCESS
}
