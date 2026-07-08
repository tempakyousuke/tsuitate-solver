//! 衝立詰将棋ソルバーの CLI。
//!
//! Web サイト（tsuitate）等の外部プロセスから、Tauri UI なしで検証を実行するためのバイナリ。
//! 入力は問題 JSON（docs/json-format.md の疎表現）、出力は stdout に JSON 1 行。
//!
//! 使い方:
//!   tsuitate-solver-cli <question.json> [--find-second] [--shortest]
//!                       [--node-limit N] [--timeout-secs N] [--memory-limit-mb N]
//!   tsuitate-solver-cli --solve-meta <request.json> [--node-limit N]
//!                       [--scan-node-limit N] [--parallel N]
//!   tsuitate-solver-cli --solve-meta-server [--node-limit N] [--scan-node-limit N]
//!
//! --memory-limit-mb は本プロセスのピーク RSS の上限（MB）。超過すると探索を
//! 打ち切り、出力の memoryLimited が true になる。本解の発見後（余詰め探索中）に
//! 超過した場合は found=true のまま memoryLimited=true が報告される。
//! 決定性が必要な --solve-meta には適用されない。
//!
//! --solve-meta は Webサイトの挑戦モード用。情報集合（局面リスト、全て先手番・
//! 持ち駒は両者とも明示）と深さ制限のクエリ列を受け取り、それぞれ
//! 「先手が制限内に詰みを強制できるか」を判定する。決定性を保つため
//! タイムアウトは使わない（--node-limit はクエリごとの上限。既定 2,000,000）。
//! --scan-node-limit は Proven 時の最小証明深さスキャン（タイブレーク用の
//! 精密化）専用の追加ノード予算（0 = 無制限）。予算に達したら「そこまでに
//! 証明できた深さ」で打ち切る。ノード数ベースなので決定的。
//! --parallel はクエリの並列実行数（既定 1）。クエリは独立なので結果は
//! 直列実行と同一。
//!
//! --solve-meta-server は常駐モード。stdin から行区切りJSON（--solve-meta と
//! 同じリクエスト形式）を受け、1行ずつ結果を返す。ソルバーの転置表
//! （証明・反証の恒久的事実）を手をまたいで温存するため、挑戦モードの
//! 2手目以降がほぼ即答になる。warm start の分だけ結果が一発実行と変わり得る
//! （unknown が proven/disproven になる方向のみ）ので、決定性は呼び出し側の
//! 選択の永続化で担保すること。stdin が閉じたら終了。
//! 出力: {"ok":true,"results":[{"result":"proven|disproven|unknown",
//!        "provenDepth":N|null,"nodes":N}, ...]}
//!
//! 終了コード: 0 = 探索実行（found の真偽は JSON を参照）, 2 = 入力不正

use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// 本プロセスのピーク RSS（bytes）。--memory-limit-mb 用。
/// ヒープの生存量ではなく OS が見る常駐量（= OOM killer の判定対象）を使う。
/// malloc は解放済みページをすぐには OS に返さないため、生存ヒープ量での
/// 制限では RSS を抑えられない（実測: 生存 1GB 制限で RSS 4.9GB）。
#[cfg(unix)]
fn peak_rss_bytes() -> u64 {
    let mut ru = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, ru.as_mut_ptr()) } != 0 {
        return 0;
    }
    let ru = unsafe { ru.assume_init() };
    let v = ru.ru_maxrss as u64;
    // ru_maxrss の単位: macOS は bytes、Linux は KB
    if cfg!(target_os = "macos") {
        v
    } else {
        v * 1024
    }
}

#[cfg(not(unix))]
fn peak_rss_bytes() -> u64 {
    0 // 非対応プラットフォームでは上限は発火しない
}

use serde::Deserialize;
use serde_json::json;
use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::defense::{
    solve_meta_query, solve_meta_query_with, MetaQueryResult,
};
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
    /// 入力ファイル。--solve-meta-server では不要（stdin を使う）
    input: Option<String>,
    find_second: bool,
    shortest: bool,
    solve_meta: bool,
    /// 常駐モード: stdin から行区切りJSONでクエリを受け続ける
    solve_meta_server: bool,
    node_limit: u64,
    node_limit_given: bool,
    timeout_secs: u64,
    /// ヒープ確保量の上限（MB）。0 なら無制限
    memory_limit_mb: u64,
    /// --solve-meta: 最小証明深さスキャン専用の追加ノード予算。0 なら無制限
    scan_node_limit: u64,
    /// --solve-meta: クエリの並列実行数。既定 1（直列）
    parallel: usize,
}

fn parse_args() -> Result<Args, String> {
    let mut input: Option<String> = None;
    let mut find_second = false;
    let mut shortest = false;
    let mut solve_meta = false;
    let mut solve_meta_server = false;
    let mut node_limit: u64 = 50_000_000;
    let mut node_limit_given = false;
    let mut timeout_secs: u64 = 120;
    let mut memory_limit_mb: u64 = 0;
    let mut scan_node_limit: u64 = 0;
    let mut parallel: usize = 1;

    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--find-second" => find_second = true,
            "--shortest" => shortest = true,
            "--solve-meta" => solve_meta = true,
            "--solve-meta-server" => solve_meta_server = true,
            "--node-limit" => {
                let v = iter.next().ok_or("--node-limit requires a value")?;
                node_limit = v.parse().map_err(|_| format!("Invalid --node-limit: {}", v))?;
                node_limit_given = true;
            }
            "--timeout-secs" => {
                let v = iter.next().ok_or("--timeout-secs requires a value")?;
                timeout_secs = v.parse().map_err(|_| format!("Invalid --timeout-secs: {}", v))?;
            }
            "--memory-limit-mb" => {
                let v = iter.next().ok_or("--memory-limit-mb requires a value")?;
                memory_limit_mb =
                    v.parse().map_err(|_| format!("Invalid --memory-limit-mb: {}", v))?;
            }
            "--scan-node-limit" => {
                let v = iter.next().ok_or("--scan-node-limit requires a value")?;
                scan_node_limit =
                    v.parse().map_err(|_| format!("Invalid --scan-node-limit: {}", v))?;
            }
            "--parallel" => {
                let v = iter.next().ok_or("--parallel requires a value")?;
                parallel = v.parse().map_err(|_| format!("Invalid --parallel: {}", v))?;
                if parallel == 0 {
                    return Err("--parallel must be >= 1".to_string());
                }
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

    if input.is_none() && !solve_meta_server {
        return Err("Usage: tsuitate-solver-cli <question.json> [--find-second] [--shortest] [--node-limit N] [--timeout-secs N] [--memory-limit-mb N] | --solve-meta <request.json> [--node-limit N] [--scan-node-limit N] [--parallel N] | --solve-meta-server [--node-limit N] [--scan-node-limit N]".to_string());
    }
    Ok(Args {
        input,
        find_second,
        shortest,
        solve_meta,
        solve_meta_server,
        node_limit,
        node_limit_given,
        timeout_secs,
        memory_limit_mb,
        scan_node_limit,
        parallel,
    })
}

/// --solve-meta モードのリクエスト
#[derive(Debug, Deserialize)]
struct SolveMetaRequestJson {
    #[serde(default)]
    queries: Vec<MetaQueryJson>,
    /// 深さ制限探索を伴わない軽量クエリ: 情報集合が実質詰み（無駄合いのみ）か。
    /// 挑戦モード（challenge.ts）が「後手番・王手された直後」の局面集合について、
    /// これ以上の応手展開が無意味かどうかを判定するのに使う
    #[serde(default)]
    effectively_checkmate_queries: Vec<EffectivelyCheckmateQueryJson>,
}

#[derive(Debug, Deserialize)]
struct MetaQueryJson {
    depth_limit: u32,
    positions: Vec<MetaPositionJson>,
}

#[derive(Debug, Deserialize)]
struct EffectivelyCheckmateQueryJson {
    positions: Vec<MetaPositionJson>,
}

/// 情報集合の1局面（持ち駒は両者とも明示）。
/// 手番は既定で先手（--solve-meta の従来クエリ）。
/// effectively_checkmate_queries では後手番（王手された直後）を指定する
#[derive(Debug, Deserialize)]
struct MetaPositionJson {
    board: Vec<BoardPiece>,
    #[serde(default)]
    sente_hand: Vec<HandPieceJson>,
    #[serde(default)]
    gote_hand: Vec<HandPieceJson>,
    #[serde(default)]
    side_to_move: Option<String>,
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
    pos.side_to_move = match &p.side_to_move {
        Some(s) => parse_color(s)?,
        None => Color::Sente,
    };
    Ok(pos)
}

/// 情報集合が実質詰み（詰みまたは全応手が無駄合い）かどうかを判定する。
/// `all_effectively_checkmate` は zobrist ハッシュを参照しないため
/// init_zobrist_hash は不要だが、Position の他の不変条件と揃えるため呼んでおく
fn compute_effectively_checkmate(query: &EffectivelyCheckmateQueryJson) -> Result<bool, String> {
    let mut positions = Vec::with_capacity(query.positions.len());
    for p in &query.positions {
        let mut pos = position_from_meta_json(p)?;
        pos.init_zobrist_hash();
        positions.push(pos);
    }
    Ok(MetaPosition { positions }.all_effectively_checkmate())
}

/// 常駐モード: stdin から行区切りJSON（--solve-meta と同じリクエスト形式）を
/// 受け続け、1行ずつ結果を返す。ソルバーは使い回し、証明・反証の事実
/// （転置表）を温存して後続クエリを warm start する。挑戦モードは1手ごとに
/// 「前の手の証明の部分木」を問い直すため、これが支配的なコストを消す。
/// リクエストの合間に再開用キャッシュを trim してメモリを頭打ちにする。
/// クエリは直列実行（--parallel は無視。温存された転置表の共有が優先）
fn run_solve_meta_server(node_limit: u64, scan_node_limit: u64) -> ExitCode {
    use std::io::{BufRead, Write};

    let scan_node_limit = if scan_node_limit == 0 { u64::MAX } else { scan_node_limit };
    let cancel = Arc::new(AtomicBool::new(false));
    let mut solver = TsuitateDfpnSolver::new(u64::MAX, cancel);

    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<SolveMetaRequestJson>(&line) {
            Err(e) => json!({ "ok": false, "error": format!("parse: {}", e) }),
            Ok(request) => {
                let mut results = Vec::new();
                let mut error: Option<String> = None;
                for query in &request.queries {
                    let positions: Result<Vec<Position>, String> =
                        query.positions.iter().map(position_from_meta_json).collect();
                    match positions {
                        Ok(ps) if !ps.is_empty() => {
                            let outcome = solve_meta_query_with(
                                &mut solver,
                                ps,
                                query.depth_limit,
                                node_limit,
                                scan_node_limit,
                            );
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
                        Ok(_) => {
                            error = Some("empty positions in query".to_string());
                            break;
                        }
                        Err(e) => {
                            error = Some(format!("invalid position: {}", e));
                            break;
                        }
                    }
                }
                let mut effectively_checkmate_results = Vec::new();
                if error.is_none() {
                    for query in &request.effectively_checkmate_queries {
                        match compute_effectively_checkmate(query) {
                            Ok(b) => effectively_checkmate_results.push(b),
                            Err(e) => {
                                error = Some(format!("invalid position: {}", e));
                                break;
                            }
                        }
                    }
                }
                solver.trim_transient_caches();
                match error {
                    Some(e) => json!({ "ok": false, "error": e }),
                    None => json!({
                        "ok": true,
                        "results": results,
                        "effectivelyCheckmate": effectively_checkmate_results,
                    }),
                }
            }
        };
        let mut out = std::io::stdout().lock();
        if writeln!(out, "{}", response).and_then(|_| out.flush()).is_err() {
            break; // 親プロセスが死んだら終了
        }
    }
    ExitCode::SUCCESS
}

fn run_solve_meta(
    json_str: &str,
    node_limit: u64,
    scan_node_limit: u64,
    parallel: usize,
) -> ExitCode {
    let request: SolveMetaRequestJson = match serde_json::from_str(json_str) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Failed to parse solve-meta request: {}", e);
            return ExitCode::from(2);
        }
    };
    // 0 は無制限
    let scan_node_limit = if scan_node_limit == 0 { u64::MAX } else { scan_node_limit };

    // 入力の検証は先に直列で行う（不正入力は探索前に exit 2）
    let mut parsed_queries: Vec<(Vec<Position>, u32)> = Vec::new();
    for query in &request.queries {
        let positions: Result<Vec<Position>, String> =
            query.positions.iter().map(position_from_meta_json).collect();
        match positions {
            Ok(ps) if !ps.is_empty() => parsed_queries.push((ps, query.depth_limit)),
            Ok(_) => {
                eprintln!("Empty positions in query");
                return ExitCode::from(2);
            }
            Err(e) => {
                eprintln!("Invalid position: {}", e);
                return ExitCode::from(2);
            }
        }
    }

    // クエリは互いに独立で、各クエリの探索はノード上限のみで打ち切るため、
    // 並列に実行しても結果は直列実行と同一（決定性は保たれる）
    let workers = parallel.min(parsed_queries.len()).max(1);
    let queries: Vec<Option<(Vec<Position>, u32)>> =
        parsed_queries.into_iter().map(Some).collect();
    let queries = std::sync::Mutex::new(queries.into_iter().enumerate().collect::<Vec<_>>());
    let outcomes: Vec<std::sync::Mutex<Option<serde_json::Value>>> =
        (0..request.queries.len()).map(|_| std::sync::Mutex::new(None)).collect();

    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let next = queries.lock().unwrap().pop();
                let Some((idx, slot)) = next else { break };
                let (positions, depth_limit) = slot.expect("query taken twice");
                let outcome = solve_meta_query(positions, depth_limit, node_limit, scan_node_limit);
                *outcomes[idx].lock().unwrap() = Some(json!({
                    "result": match outcome.result {
                        MetaQueryResult::Proven => "proven",
                        MetaQueryResult::Disproven => "disproven",
                        MetaQueryResult::Unknown => "unknown",
                    },
                    "provenDepth": outcome.proven_depth,
                    "nodes": outcome.nodes,
                }));
            });
        }
    });

    let results: Vec<serde_json::Value> = outcomes
        .into_iter()
        .map(|m| m.into_inner().unwrap().expect("query not solved"))
        .collect();

    let mut effectively_checkmate_results = Vec::new();
    for query in &request.effectively_checkmate_queries {
        match compute_effectively_checkmate(query) {
            Ok(b) => effectively_checkmate_results.push(b),
            Err(e) => {
                eprintln!("Invalid position: {}", e);
                return ExitCode::from(2);
            }
        }
    }

    println!(
        "{}",
        json!({
            "ok": true,
            "results": results,
            "effectivelyCheckmate": effectively_checkmate_results,
        })
    );
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

    // 挑戦モードの判定は決定性が必要なためタイムアウトなし・ノード上限のみ。
    // 既定の 50M は求解用なので、未指定なら応答時間を抑えた上限にする
    let meta_node_limit = if args.node_limit_given { args.node_limit } else { 2_000_000 };
    if args.solve_meta_server {
        return run_solve_meta_server(meta_node_limit, args.scan_node_limit);
    }

    let input = args.input.as_deref().expect("parse_args で検証済み");
    let json_str = match std::fs::read_to_string(input) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to read {}: {}", input, e);
            return ExitCode::from(2);
        }
    };
    if args.solve_meta {
        return run_solve_meta(&json_str, meta_node_limit, args.scan_node_limit, args.parallel);
    }
    let question: QuestionJson = match serde_json::from_str(&json_str) {
        Ok(q) => q,
        Err(e) => {
            eprintln!("Failed to parse {}: {}", input, e);
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

    // タイムアウト・メモリ上限用キャンセルフラグ（どちらが発火したかは個別フラグで区別）
    let cancel = Arc::new(AtomicBool::new(false));
    let timeout_fired = Arc::new(AtomicBool::new(false));
    let memory_fired = Arc::new(AtomicBool::new(false));
    // メインスレッドが結果 JSON の出力を開始したら true。ハードキルとの
    // stdout 書き込み競合（出力の破損）を防ぐ
    let output_started = Arc::new(AtomicBool::new(false));
    // 巨大な情報集合の展開（expand_defense_moves）を途中で打ち切れるようにする
    tsuitate_solver_lib::solver::metaposition::set_expansion_cancel(cancel.clone());
    if args.timeout_secs > 0 {
        let cancel_clone = cancel.clone();
        let fired = timeout_fired.clone();
        let secs = args.timeout_secs;
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(secs));
            fired.store(true, Ordering::Relaxed);
            cancel_clone.store(true, Ordering::Relaxed);
        });
    }
    if args.memory_limit_mb > 0 {
        let cancel_clone = cancel.clone();
        let fired = memory_fired.clone();
        let soft_limit = args.memory_limit_mb.saturating_mul(1024 * 1024);
        // ソフト上限で協調キャンセル。それでも成長が止まらない経路（キャンセル
        // チェックのない大確保など）への最終防衛として、1.5倍でフォールバック
        // JSON を出力して正常終了する（OOM killer に殺されて出力ゼロになるより良い）
        let hard_limit = soft_limit.saturating_add(soft_limit / 2);
        let output_started = output_started.clone();
        std::thread::spawn(move || loop {
            let rss = peak_rss_bytes();
            if output_started.load(Ordering::Relaxed) {
                // 探索は終了済みでメインが結果を出力中。killせず任せる
                return;
            }
            if rss >= hard_limit && fired.load(Ordering::Relaxed) {
                println!(
                    "{}",
                    json!({
                        "found": false,
                        "depth": null,
                        "hasSecondSolution": false,
                        "secondDepth": null,
                        "hasPieceSurplus": false,
                        "kizuCount": 0,
                        "unknown": true,
                        "timedOut": false,
                        "memoryLimited": true,
                        "nodesSearched": null,
                        "message": "メモリ上限に達したため探索を強制終了しました",
                        "tree": null,
                        "secondTree": null,
                    })
                );
                std::process::exit(0);
            }
            if rss >= soft_limit {
                fired.store(true, Ordering::Relaxed);
                cancel_clone.store(true, Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
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
    // memoryLimited は found=true でも報告する（余詰め探索がメモリ上限で
    // 打ち切られた場合、サイト側が大きい予算で再検証できるように）
    let memory_limited = memory_fired.load(Ordering::Relaxed);
    let timed_out = unknown && !memory_limited && timeout_fired.load(Ordering::Relaxed);

    // 駒余り: 最長手順の詰め上がりで攻め方の持ち駒が余るか（変化での駒余りは不問）
    let has_piece_surplus = result
        .tree
        .as_ref()
        .map_or(false, |t| t.has_piece_surplus());

    // 以降はハードキル禁止（結果確定済み。出力の競合・破損を防ぐ）
    output_started.store(true, Ordering::Relaxed);
    let output = json!({
        "found": result.found,
        "depth": depth,
        "hasSecondSolution": result.second_tree.is_some(),
        "secondDepth": second_depth,
        "hasPieceSurplus": has_piece_surplus,
        "kizuCount": result.kizu_trees.len(),
        "unknown": unknown,
        "timedOut": timed_out,
        "memoryLimited": memory_limited,
        "nodesSearched": solver.nodes_searched,
        "message": result.message,
        "tree": result.tree,
        "secondTree": result.second_tree,
    });
    println!("{}", output);

    ExitCode::SUCCESS
}
