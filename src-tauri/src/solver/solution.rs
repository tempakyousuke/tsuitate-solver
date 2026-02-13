use serde::{Deserialize, Serialize};

use crate::shogi::types::Move;

/// 攻め方が指した手に対する観測結果
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Observation {
    /// 駒が取られなかった
    NoCapture,
    /// 駒が取られた（取られた駒の種類は分からない - 衝立のルール）
    Captured,
    /// 反則（打ち歩詰め等で手が無効だった場合）
    /// 衝立詰将棋では通常使わないが、不正局面の検出用
    Illegal,
    /// 詰み（手番側に合法手がない）
    Checkmate,
}

/// 解の手順木のノード
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SolutionNode {
    /// 詰み（この局面で詰んでいる）
    Checkmate {
        /// 詰みまでの手数
        depth: u32,
    },
    /// 攻め方の手
    AttackMove {
        /// 指し手
        mv: MoveData,
        /// 観測結果による分岐
        branches: Vec<SolutionBranch>,
    },
}

/// 観測結果による分岐
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionBranch {
    /// 観測結果
    pub observation: Observation,
    /// この観測の後の継続
    pub continuation: Box<SolutionNode>,
}

/// フロントエンドに送る指し手データ
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MoveData {
    pub from_file: Option<u8>,
    pub from_rank: Option<u8>,
    pub to_file: u8,
    pub to_rank: u8,
    pub promotion: bool,
    pub drop_piece: Option<String>,
    pub notation: String, // 日本語棋譜表記
}

impl MoveData {
    pub fn from_move(mv: Move, color: crate::shogi::types::Color) -> Self {
        Self {
            from_file: mv.from.map(|s| s.file),
            from_rank: mv.from.map(|s| s.rank),
            to_file: mv.to.file,
            to_rank: mv.to.rank,
            promotion: mv.promotion,
            drop_piece: mv.drop_piece.map(|k| k.to_kanji().to_string()),
            notation: mv.to_japanese(color),
        }
    }
}

/// 解のデータ（フロントエンドへの送信用）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SolutionData {
    /// 解が見つかったか
    pub found: bool,
    /// 解の手順木
    pub tree: Option<SolutionNode>,
    /// メッセージ
    pub message: String,
    /// 探索ログ
    #[serde(default)]
    pub trace: Vec<String>,
}
