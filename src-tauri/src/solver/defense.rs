//! 挑戦モード（Webサイト側）向けの情報集合クエリ（--solve-meta モード）。
//!
//! Webサイトの詰将棋挑戦モードは、後手の観測分岐（応手でどの駒が取られたか等）を
//! 敵対的に選ぶために「この情報集合から先手は残り N 手以内に詰みを強制できるか」を
//! 問い合わせる。本モジュールはその1クエリ（局面集合 + 深さ制限）を評価する。
//!
//! 決定性が必要なためタイムアウトは使わず、ノード上限のみで打ち切る
//! （打ち切りは Unknown として返し、呼び出し側がフォールバックする）。

use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use super::metaposition::MetaPosition;
use super::tsuitate_dfpn::{TsuitateDfpnResult, TsuitateDfpnSolver};
use crate::shogi::position::Position;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MetaQueryResult {
    /// 先手は深さ制限内に詰みを強制できる
    Proven,
    /// 先手は深さ制限内に詰みを強制できない
    Disproven,
    /// ノード上限内に判定できなかった
    Unknown,
}

#[derive(Debug)]
pub struct MetaQueryOutcome {
    pub result: MetaQueryResult,
    /// Proven の場合、証明できた最小の深さ制限（最短詰み手数の上界）
    pub proven_depth: Option<u32>,
    pub nodes: u64,
}

/// 情報集合（全局面が先手番）に対する深さ制限付きの詰み判定。
/// scan_node_limit は最小証明深さスキャン専用の追加ノード予算
/// （0 = 自動、u64::MAX = 無制限。詳細は TsuitateDfpnSolver::solve_bounded）
pub fn solve_meta_query(
    positions: Vec<Position>,
    depth_limit: u32,
    node_limit: u64,
    scan_node_limit: u64,
) -> MetaQueryOutcome {
    let cancel = Arc::new(AtomicBool::new(false));
    let mut solver = TsuitateDfpnSolver::new(node_limit, cancel);
    solve_meta_query_with(&mut solver, positions, depth_limit, node_limit, scan_node_limit)
}

/// 既存のソルバー（温存された転置表つき）でクエリを判定する常駐モード用。
/// ノード予算は現在の累計 nodes_searched を起点にクエリ単位で適用する
pub fn solve_meta_query_with(
    solver: &mut TsuitateDfpnSolver,
    positions: Vec<Position>,
    depth_limit: u32,
    node_limit: u64,
    scan_node_limit: u64,
) -> MetaQueryOutcome {
    let mut inited: Vec<Position> = positions;
    for pos in &mut inited {
        pos.init_zobrist_hash();
    }
    let meta = MetaPosition { positions: inited };

    let start_nodes = solver.nodes_searched;
    solver.set_node_limit(start_nodes.saturating_add(node_limit));
    let (result, proven_depth) = solver.solve_bounded(&meta, depth_limit, scan_node_limit);

    MetaQueryOutcome {
        result: match result {
            TsuitateDfpnResult::Proven => MetaQueryResult::Proven,
            TsuitateDfpnResult::Disproven => MetaQueryResult::Disproven,
            TsuitateDfpnResult::Unknown => MetaQueryResult::Unknown,
        },
        proven_depth,
        nodes: solver.nodes_searched - start_nodes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shogi::types::*;

    /// 1手詰め: 後手玉5一、先手金4三、持ち駒金1 → G*5b で詰み
    fn mate_in_one() -> Position {
        let mut pos = Position::new();
        pos.set_piece(Square::new(5, 1), Piece::new(Color::Gote, PieceKind::King));
        pos.set_piece(Square::new(4, 3), Piece::new(Color::Sente, PieceKind::Gold));
        pos.sente_hand.add(PieceKind::Gold);
        // 後手持ち駒: 残り全部
        pos.gote_hand.add(PieceKind::Rook);
        pos.gote_hand.add(PieceKind::Rook);
        pos.gote_hand.add(PieceKind::Bishop);
        pos.gote_hand.add(PieceKind::Bishop);
        pos.gote_hand.add(PieceKind::Gold);
        pos.gote_hand.add(PieceKind::Gold);
        for _ in 0..4 {
            pos.gote_hand.add(PieceKind::Silver);
            pos.gote_hand.add(PieceKind::Knight);
            pos.gote_hand.add(PieceKind::Lance);
        }
        for _ in 0..18 {
            pos.gote_hand.add(PieceKind::Pawn);
        }
        pos.side_to_move = Color::Sente;
        pos
    }

    #[test]
    fn proven_within_budget() {
        let outcome = solve_meta_query(vec![mate_in_one()], 5, 1_000_000, u64::MAX);
        assert_eq!(outcome.result, MetaQueryResult::Proven);
        assert_eq!(outcome.proven_depth, Some(1));
    }

    #[test]
    fn disproven_with_zero_material() {
        // 攻め駒が金1枚だけでは詰まない
        let mut pos = mate_in_one();
        pos.sente_hand.remove(PieceKind::Gold);
        pos.gote_hand.add(PieceKind::Gold);
        let outcome = solve_meta_query(vec![pos], 5, 1_000_000, u64::MAX);
        assert_eq!(outcome.result, MetaQueryResult::Disproven);
    }
}
