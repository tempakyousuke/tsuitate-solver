use std::sync::atomic::Ordering;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::shogi::position::Position;
use crate::shogi::types::*;
use crate::solver::metaposition::MetaPosition;
use crate::solver::solution::SolutionData;
use crate::solver::tsuitate_dfpn::TsuitateDfpnSolver;
use crate::CancelFlag;

/// フロントエンドから受け取る盤面データ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionData {
    /// 盤面 (9x9, board[file-1][rank-1])
    /// None = 空きマス, Some = 駒情報
    pub board: Vec<Vec<Option<PieceData>>>,
    /// 先手（攻め方）の持ち駒
    pub sente_hand: Vec<HandPieceData>,
    /// 後手（玉方）の持ち駒
    pub gote_hand: Vec<HandPieceData>,
}

/// 駒データ（フロントエンド用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PieceData {
    pub color: String,   // "sente" or "gote"
    pub kind: String,    // "king", "rook", "bishop", etc.
}

/// 持ち駒データ（フロントエンド用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HandPieceData {
    pub kind: String,
    pub count: u8,
}

fn parse_color(s: &str) -> Result<Color, String> {
    match s {
        "sente" => Ok(Color::Sente),
        "gote" => Ok(Color::Gote),
        _ => Err(format!("Invalid color: {}", s)),
    }
}

fn parse_piece_kind(s: &str) -> Result<PieceKind, String> {
    match s {
        "king" => Ok(PieceKind::King),
        "rook" => Ok(PieceKind::Rook),
        "bishop" => Ok(PieceKind::Bishop),
        "gold" => Ok(PieceKind::Gold),
        "silver" => Ok(PieceKind::Silver),
        "knight" => Ok(PieceKind::Knight),
        "lance" => Ok(PieceKind::Lance),
        "pawn" => Ok(PieceKind::Pawn),
        "promoted_rook" => Ok(PieceKind::PromotedRook),
        "promoted_bishop" => Ok(PieceKind::PromotedBishop),
        "promoted_silver" => Ok(PieceKind::PromotedSilver),
        "promoted_knight" => Ok(PieceKind::PromotedKnight),
        "promoted_lance" => Ok(PieceKind::PromotedLance),
        "promoted_pawn" => Ok(PieceKind::PromotedPawn),
        _ => Err(format!("Invalid piece kind: {}", s)),
    }
}

fn position_from_data(data: &PositionData) -> Result<Position, String> {
    let mut pos = Position::new();

    // 盤面の駒を配置
    if data.board.len() != 9 {
        return Err("Board must have 9 files".to_string());
    }
    for (file_idx, file) in data.board.iter().enumerate() {
        if file.len() != 9 {
            return Err(format!("File {} must have 9 ranks", file_idx + 1));
        }
        for (rank_idx, cell) in file.iter().enumerate() {
            if let Some(piece_data) = cell {
                let color = parse_color(&piece_data.color)?;
                let kind = parse_piece_kind(&piece_data.kind)?;
                let sq = Square::new((file_idx + 1) as u8, (rank_idx + 1) as u8);
                pos.set_piece(sq, Piece::new(color, kind));
            }
        }
    }

    // 持ち駒を設定
    for hand_piece in &data.sente_hand {
        let kind = parse_piece_kind(&hand_piece.kind)?;
        for _ in 0..hand_piece.count {
            pos.sente_hand.add(kind);
        }
    }
    for hand_piece in &data.gote_hand {
        let kind = parse_piece_kind(&hand_piece.kind)?;
        for _ in 0..hand_piece.count {
            pos.gote_hand.add(kind);
        }
    }

    pos.side_to_move = Color::Sente;
    Ok(pos)
}

/// 盤面のバリデーション
#[tauri::command]
pub fn validate_position(position: PositionData) -> Result<bool, String> {
    let pos = position_from_data(&position)?;

    // 後手の玉があるか確認
    if pos.find_king(Color::Gote).is_none() {
        return Err("後手の玉が配置されていません".to_string());
    }

    // 先手の玉は詰将棋では不要（ただしあってもよい）

    // 先手番で王手がかかっていないことは必須ではない（衝立詰将棋の初期局面は先手番）

    Ok(true)
}

/// 衝立詰将棋を解く
#[tauri::command]
pub async fn solve(
    position: PositionData,
    find_second_solution: Option<bool>,
    cancel_flag: tauri::State<'_, CancelFlag>,
) -> Result<SolutionData, String> {
    let pos = position_from_data(&position)?;

    // 後手の玉があるか確認
    if pos.find_king(Color::Gote).is_none() {
        return Err("後手の玉が配置されていません".to_string());
    }

    let find_second = find_second_solution.unwrap_or(false);

    // キャンセルフラグをリセット
    cancel_flag.store(false, Ordering::Relaxed);
    let cancelled = cancel_flag.inner().clone();

    // ブロッキング処理を別スレッドで実行
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let meta = MetaPosition::new(pos);
        let node_limit = 50_000_000; // df-pnのノード上限
        let mut solver = TsuitateDfpnSolver::new(node_limit, cancelled);
        let result = if find_second {
            solver.solve_to_solution_with_second(&meta)
        } else {
            solver.solve_to_solution(&meta)
        };
        let _ = tx.send(result);
    });

    let result = rx.recv().map_err(|e| format!("Solver error: {}", e))?;

    Ok(result)
}

/// 探索を中止する
#[tauri::command]
pub fn cancel_solve(cancel_flag: tauri::State<'_, CancelFlag>) {
    cancel_flag.store(true, Ordering::Relaxed);
}

/// sample-questions ディレクトリのパスを取得する
fn get_sample_questions_dir(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    // 本番ビルド: リソースディレクトリから探す
    if let Ok(resource_dir) = app.path().resource_dir() {
        let dir = resource_dir.join("sample-questions");
        if dir.is_dir() {
            return Some(dir);
        }
    }
    // 開発時: プロジェクトルートの sample-questions/
    let dev_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("sample-questions");
    if dev_dir.is_dir() {
        return Some(dev_dir);
    }
    None
}

/// サンプル問題の一覧を取得する
#[tauri::command]
pub fn list_sample_questions(app: tauri::AppHandle) -> Result<Vec<u32>, String> {
    let dir = get_sample_questions_dir(&app)
        .ok_or_else(|| "サンプル問題ディレクトリが見つかりません".to_string())?;

    let mut numbers: Vec<u32> = std::fs::read_dir(&dir)
        .map_err(|e| format!("ディレクトリの読み込みに失敗: {}", e))?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name().to_string_lossy().to_string();
            let num = name.strip_suffix(".json")?.parse::<u32>().ok()?;
            Some(num)
        })
        .collect();

    numbers.sort();
    Ok(numbers)
}

/// 指定番号のサンプル問題を読み込む
#[tauri::command]
pub fn load_sample_question(app: tauri::AppHandle, number: u32) -> Result<String, String> {
    let dir = get_sample_questions_dir(&app)
        .ok_or_else(|| "サンプル問題ディレクトリが見つかりません".to_string())?;

    let path = dir.join(format!("{}.json", number));
    std::fs::read_to_string(&path)
        .map_err(|e| format!("問題ファイルの読み込みに失敗: {}", e))
}
