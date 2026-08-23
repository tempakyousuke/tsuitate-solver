use super::bitboard::{
    nearest_on_ray, slide_targets, Bitboard, ALL_MASK, DIAG_STEPS, FILE_MASK, GOLD_STEPS,
    KING_STEPS, KNIGHT_STEPS, NEAR_ATTACKER_MASK, ORTHO_STEPS, PAWN_STEPS, RANK_MASK,
    SILVER_STEPS,
};
use super::position::Position;
use super::types::*;

#[inline]
const fn color_index(color: Color) -> usize {
    match color {
        Color::Sente => 0,
        Color::Gote => 1,
    }
}

/// 擬似合法手を生成（自玉の安全は確認しない、ビットボード版）
pub fn generate_pseudo_legal_moves(pos: &Position, color: Color) -> Vec<Move> {
    let mut moves = Vec::new();
    generate_board_moves(pos, color, &mut moves);
    generate_drop_moves(pos, color, &mut moves);
    moves
}

/// 盤上の駒の移動だけを生成する（打ちを含まない擬似合法手）
pub fn generate_board_moves(pos: &Position, color: Color, moves: &mut Vec<Move>) {
    // 占有ビットボードのビット走査
    let mut bb = pos.occupancy(color);
    while bb != 0 {
        let idx = bb.trailing_zeros() as usize;
        bb &= bb - 1;
        let sq = Square::from_index(idx);
        if let Some(piece) = pos.piece_at(sq) {
            generate_piece_moves(pos, sq, piece, moves);
        }
    }
}

/// 駒の移動手を生成（ビットボード版）
fn generate_piece_moves(pos: &Position, from: Square, piece: Piece, moves: &mut Vec<Move>) {
    let color = piece.color;
    let ci = color_index(color);
    let own = pos.occupancy(color);
    let occ_all = pos.occupancy_all();
    let idx = from.index();

    match piece.kind {
        PieceKind::King => {
            emit_step_moves(KING_STEPS[idx] & !own, from, piece, false, color, moves);
        }
        PieceKind::Gold
        | PieceKind::PromotedSilver
        | PieceKind::PromotedKnight
        | PieceKind::PromotedLance
        | PieceKind::PromotedPawn => {
            emit_step_moves(GOLD_STEPS[ci][idx] & !own, from, piece, false, color, moves);
        }
        PieceKind::Silver => {
            emit_step_moves(SILVER_STEPS[ci][idx] & !own, from, piece, true, color, moves);
        }
        PieceKind::Knight => {
            emit_step_moves(KNIGHT_STEPS[ci][idx] & !own, from, piece, true, color, moves);
        }
        PieceKind::Pawn => {
            emit_step_moves(PAWN_STEPS[ci][idx] & !own, from, piece, true, color, moves);
        }
        PieceKind::Lance => {
            // 先手は rank 減少方向 (0,-1)=dir3、後手は rank 増加方向 (0,+1)=dir2
            let dir = if color == Color::Sente { 3 } else { 2 };
            emit_step_moves(
                slide_targets(occ_all, dir, idx) & !own,
                from, piece, true, color, moves,
            );
        }
        PieceKind::Rook | PieceKind::PromotedRook => {
            let promoted = piece.kind == PieceKind::PromotedRook;
            let mut targets: Bitboard = 0;
            for dir in 0..4 {
                targets |= slide_targets(occ_all, dir, idx);
            }
            if promoted {
                targets |= DIAG_STEPS[idx];
            }
            emit_step_moves(targets & !own, from, piece, !promoted, color, moves);
        }
        PieceKind::Bishop | PieceKind::PromotedBishop => {
            let promoted = piece.kind == PieceKind::PromotedBishop;
            let mut targets: Bitboard = 0;
            for dir in 4..8 {
                targets |= slide_targets(occ_all, dir, idx);
            }
            if promoted {
                targets |= ORTHO_STEPS[idx];
            }
            emit_step_moves(targets & !own, from, piece, !promoted, color, moves);
        }
    }
}

/// 移動先ビットボードの各マスに対して手を生成
#[inline]
fn emit_step_moves(
    mut targets: Bitboard,
    from: Square,
    piece: Piece,
    can_promote: bool,
    color: Color,
    moves: &mut Vec<Move>,
) {
    while targets != 0 {
        let to_idx = targets.trailing_zeros() as usize;
        targets &= targets - 1;
        add_move_with_promotion(from, Square::from_index(to_idx), color, can_promote, piece, moves);
    }
}

/// 成り/不成の判定を含めて手を追加
fn add_move_with_promotion(
    from: Square,
    to: Square,
    color: Color,
    can_promote: bool,
    piece: Piece,
    moves: &mut Vec<Move>,
) {
    let promo_zone_start = if color == Color::Sente { 1 } else { 7 };
    let promo_zone_end = if color == Color::Sente { 3 } else { 9 };
    let in_promo_zone =
        (from.rank >= promo_zone_start && from.rank <= promo_zone_end)
            || (to.rank >= promo_zone_start && to.rank <= promo_zone_end);

    if can_promote && piece.kind.can_promote() && in_promo_zone {
        // 成る手
        moves.push(Move::normal(from, to, true, piece.kind));

        // 不成が合法な場合のみ追加
        if !must_promote(piece.kind, to, color) {
            moves.push(Move::normal(from, to, false, piece.kind));
        }
    } else {
        moves.push(Move::normal(from, to, false, piece.kind));
    }
}

/// 成りが強制かどうか（行き所のない駒の判定）
fn must_promote(kind: PieceKind, to: Square, color: Color) -> bool {
    match kind {
        PieceKind::Pawn | PieceKind::Lance => {
            if color == Color::Sente {
                to.rank == 1
            } else {
                to.rank == 9
            }
        }
        PieceKind::Knight => {
            if color == Color::Sente {
                to.rank <= 2
            } else {
                to.rank >= 8
            }
        }
        _ => false,
    }
}

/// 持ち駒を打つ手を生成（ビットボード版）
fn generate_drop_moves(pos: &Position, color: Color, moves: &mut Vec<Move>) {
    generate_drop_moves_masked(pos, color, |_| ALL_MASK, moves);
}

/// 打つ手を生成（打てるマスを kind ごとのマスクで絞り込む）。
///
/// `allowed(kind)` が返すビットボードとの積だけを列挙する。王手候補手だけが
/// 欲しい攻め方の手生成では、駒種ごとに「玉に利きうるマス」へ絞ることで、
/// 空きマス全部（1駒種あたり最大80マス）の列挙を避けられる。
/// 二歩・行き所のない駒・打ち歩詰めの除外は `drop_targets` が担うため、
/// マスクの与え方によらず生成される手は常に擬似合法。
pub fn generate_drop_moves_masked(
    pos: &Position,
    color: Color,
    allowed: impl Fn(PieceKind) -> Bitboard,
    moves: &mut Vec<Move>,
) {
    let hand = pos.hand(color);
    for &kind in &PieceKind::HAND_PIECES {
        if !hand.has(kind) {
            continue;
        }
        let mask = allowed(kind);
        if mask == 0 {
            continue;
        }
        let mut targets = drop_targets(pos, color, kind) & mask;
        while targets != 0 {
            let idx = targets.trailing_zeros() as usize;
            targets &= targets - 1;
            moves.push(Move::drop(Square::from_index(idx), kind));
        }
    }
}

/// color が kind を打てるマスの集合。
/// 空きマス・行き所のない段・二歩・打ち歩詰めを全て除外済み。
/// 単独の手の合法性判定（`Position::is_legal_move`）と手生成の双方から使い、
/// 打ちのルールを1か所に集約する。
pub fn drop_targets(pos: &Position, color: Color, kind: PieceKind) -> Bitboard {
    drop_targets_impl(pos, color, kind, None)
}

/// `drop_targets` の1マス問い合わせ版。
///
/// `only` を与えると、そのマスに関係しない重い判定（打ち歩詰めの詰み探索、
/// 盤面全体の二歩マスク作成）を省く。返るビットボードは `only` のビットの
/// 正しさだけを保証する（他のビットは信用してはいけない）。
fn drop_targets_one(pos: &Position, color: Color, kind: PieceKind, only: Square) -> Bitboard {
    drop_targets_impl(pos, color, kind, Some(only))
}

fn drop_targets_impl(
    pos: &Position,
    color: Color,
    kind: PieceKind,
    only: Option<Square>,
) -> Bitboard {
    let hand = pos.hand(color);
    if !hand.has(kind) {
        return 0;
    }
    let empties = ALL_MASK & !pos.occupancy_all();

    // 行き所のない段を除外
    let zone: Bitboard = match kind {
        PieceKind::Pawn | PieceKind::Lance => {
            if color == Color::Sente {
                !RANK_MASK[0]
            } else {
                !RANK_MASK[8]
            }
        }
        PieceKind::Knight => {
            if color == Color::Sente {
                !(RANK_MASK[0] | RANK_MASK[1])
            } else {
                !(RANK_MASK[7] | RANK_MASK[8])
            }
        }
        _ => ALL_MASK,
    };

    let mut targets = empties & zone;
    if let Some(sq) = only {
        // 問い合わせ対象のマス以外は見ないので、ここで絞ってから重い判定に入る
        targets &= 1u128 << sq.index();
        if targets == 0 {
            return 0;
        }
    }
    if kind != PieceKind::Pawn {
        return targets;
    }

    // 二歩の筋を除外（1マス問い合わせならその筋だけ調べる）
    let mut pawn_file_mask: Bitboard = 0;
    let mut bb = pos.occupancy(color);
    if let Some(sq) = only {
        bb &= FILE_MASK[(sq.file - 1) as usize];
    }
    while bb != 0 {
        let idx = bb.trailing_zeros() as usize;
        bb &= bb - 1;
        if let Some(p) = pos.piece_at(Square::from_index(idx)) {
            if p.kind == PieceKind::Pawn {
                pawn_file_mask |= FILE_MASK[idx / 9];
            }
        }
    }
    targets &= !pawn_file_mask;

    // 打ち歩詰めチェック: 歩打ちで王手になるのは敵玉の直前マスのみ
    // なので、そのマスだけ精査すればよい
    if let Some(ksq) = pos.find_king(color.opponent()) {
        let front_rank = if color == Color::Sente {
            ksq.rank as i8 + 1
        } else {
            ksq.rank as i8 - 1
        };
        if (1..=9).contains(&front_rank) {
            let front_sq = Square::new(ksq.file, front_rank as u8);
            let front_bit = 1u128 << front_sq.index();
            // targets は only で既に絞ってあるので、無関係なマスの
            // 問い合わせでは詰み探索（is_pawn_drop_mate）が走らない
            if targets & front_bit != 0 && is_pawn_drop_mate(pos, front_sq, color) {
                targets &= !front_bit;
            }
        }
    }

    targets
}

/// 単独の手が擬似合法か（自玉の安全は見ない）。
///
/// 手を1つだけ検証したいときに、全擬似合法手を生成して照合するのを避ける。
/// 打ちは `drop_targets`、盤上の駒の移動は `generate_piece_moves` という
/// 生成側と同じ関数を使って判定するので、ルールが二重管理にならない。
pub fn is_pseudo_legal_move(pos: &Position, mv: &Move, color: Color) -> bool {
    if let Some(kind) = mv.drop_piece {
        // Move::drop が作る形と一致しない手は生成されえない
        if mv.from.is_some() || mv.promotion || mv.moved_piece_kind.is_some() {
            return false;
        }
        // 持ち駒になりえない駒種（玉・成駒）の打ちは存在しない
        if !PieceKind::HAND_PIECES.contains(&kind) {
            return false;
        }
        return drop_targets_one(pos, color, kind, mv.to) & (1u128 << mv.to.index()) != 0;
    }

    let Some(from) = mv.from else {
        return false;
    };
    let Some(piece) = pos.piece_at(from) else {
        return false;
    };
    if piece.color != color {
        return false;
    }
    MOVE_SCRATCH.with(|cell| {
        let mut buf = cell.borrow_mut();
        buf.clear();
        generate_piece_moves(pos, from, piece, &mut buf);
        buf.contains(mv)
    })
}

thread_local! {
    /// is_pseudo_legal_move の作業用バッファ（1駒分の手なので数十手）
    static MOVE_SCRATCH: std::cell::RefCell<Vec<Move>> =
        std::cell::RefCell::new(Vec::with_capacity(32));
}

// 打ち歩詰め判定の再帰ガード
// is_pawn_drop_mate → generate_legal_moves → generate_drop_moves → is_pawn_drop_mate の
// 相互再帰を防止するため、スレッドローカルフラグで既にチェック中かを管理する。
// ネストした呼び出しでは打ち歩詰めチェックをスキップし、基本合法性のみ判定する。
thread_local! {
    static IN_PAWN_DROP_MATE_CHECK: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// 打ち歩詰め判定
fn is_pawn_drop_mate(pos: &Position, sq: Square, color: Color) -> bool {
    // 既に打ち歩詰めチェック中なら再帰しない
    if IN_PAWN_DROP_MATE_CHECK.with(|f| f.get()) {
        return false;
    }

    // 打った歩が敵玉に王手でなければ打ち歩詰めはあり得ない（クローン不要の事前判定）。
    // 歩の利きは1マス前のみなので、敵玉の直前マスへの打ちだけが対象
    let Some(ksq) = pos.find_king(color.opponent()) else {
        return false;
    };
    let front_rank = match color {
        Color::Sente => ksq.rank + 1,
        Color::Gote => ksq.rank.wrapping_sub(1),
    };
    if sq.file != ksq.file || sq.rank != front_rank {
        return false;
    }

    let mut test_pos = pos.clone();
    test_pos.set_piece(sq, Piece::new(color, PieceKind::Pawn));
    test_pos.side_to_move = color.opponent();

    // 歩の王手は接触王手なので、王手回避専用ジェネレータで詰み判定する。
    // generate_legal_moves は持ち駒全種の打ち込みまで列挙して1手ずつ検証するため、
    // 持ち駒が多い局面では極端に遅い（プロファイルで探索時間の約4割を占めていた）
    IN_PAWN_DROP_MATE_CHECK.with(|f| f.set(true));
    let result = test_pos.generate_check_evasions().is_empty();
    IN_PAWN_DROP_MATE_CHECK.with(|f| f.set(false));

    result
}

/// マスが特定の色の駒に攻撃されているか（ビットボード版）
///
/// - 近接駒（玉・金類・銀・桂・歩・竜の斜め1マス・馬の十字1マス）:
///   NEAR_ATTACKER_MASK と色別占有の AND で候補マスを絞り、駒種ごとの
///   オフセット判定で確定する
/// - 飛び駒（飛/竜・角/馬・香）: レイマスクと全体占有の AND から
///   最近接ブロッカーをビット演算で求めて駒種判定
pub fn is_square_attacked(pos: &Position, sq: Square, by_color: Color) -> bool {
    let idx = sq.index();

    // 近接駒からの攻撃
    let mut near = NEAR_ATTACKER_MASK[idx] & pos.occupancy(by_color);
    while near != 0 {
        let a_idx = near.trailing_zeros() as usize;
        near &= near - 1;
        if let Some(piece) = pos.piece_at(Square::from_index(a_idx)) {
            if step_piece_attacks(piece.kind, by_color, a_idx, idx) {
                return true;
            }
        }
    }

    // 飛び駒からの攻撃（8方向の最近接ブロッカーのみ判定）
    // 方向インデックス: 0:(+1,0) 1:(-1,0) 2:(0,+1) 3:(0,-1) 4..7: 斜め
    let occ_all = pos.occupancy_all();
    for dir in 0..8 {
        let Some(b_idx) = nearest_on_ray(occ_all, dir, idx) else {
            continue;
        };
        let Some(piece) = pos.piece_at(Square::from_index(b_idx)) else {
            continue;
        };
        if piece.color != by_color {
            continue;
        }
        let hit = match dir {
            // 横方向: 飛/竜
            0 | 1 => matches!(piece.kind, PieceKind::Rook | PieceKind::PromotedRook),
            // 縦 (0,+1): 飛/竜 + 先手香（先手香は rank 増加側から rank 減少方向に利く）
            2 => matches!(piece.kind, PieceKind::Rook | PieceKind::PromotedRook)
                || (piece.kind == PieceKind::Lance && by_color == Color::Sente),
            // 縦 (0,-1): 飛/竜 + 後手香
            3 => matches!(piece.kind, PieceKind::Rook | PieceKind::PromotedRook)
                || (piece.kind == PieceKind::Lance && by_color == Color::Gote),
            // 斜め方向: 角/馬
            _ => matches!(piece.kind, PieceKind::Bishop | PieceKind::PromotedBishop),
        };
        if hit {
            return true;
        }
    }

    false
}

/// 近接駒 kind（color 側）が from_idx から to_idx のマスに利いているか
/// 飛び利き（飛/角/香、竜/馬のスライド部分）はレイ判定側で処理するため対象外
#[inline]
fn step_piece_attacks(kind: PieceKind, color: Color, from_idx: usize, to_idx: usize) -> bool {
    let df = (to_idx / 9) as i8 - (from_idx / 9) as i8;
    let dr = (to_idx % 9) as i8 - (from_idx % 9) as i8;
    match kind {
        PieceKind::King => df.abs() <= 1 && dr.abs() <= 1,
        PieceKind::Gold
        | PieceKind::PromotedSilver
        | PieceKind::PromotedKnight
        | PieceKind::PromotedLance
        | PieceKind::PromotedPawn => gold_offsets(color).contains(&(df, dr)),
        PieceKind::Silver => silver_offsets(color).contains(&(df, dr)),
        PieceKind::Knight => knight_offsets(color).contains(&(df, dr)),
        PieceKind::Pawn => pawn_offsets(color).contains(&(df, dr)),
        // 竜の斜め1マス（十字スライドはレイ判定で処理）
        PieceKind::PromotedRook => df.abs() == 1 && dr.abs() == 1,
        // 馬の十字1マス（斜めスライドはレイ判定で処理）
        PieceKind::PromotedBishop => df.abs() + dr.abs() == 1,
        // 飛/角/香はレイ判定で処理
        _ => false,
    }
}

/// 王手回避手を生成（王手されている局面専用の高効率合法手生成）
/// 通常の generate_legal_moves() の代わりに使用する。
/// 王手に対する合法応手のみを生成するため、持ち駒が多い局面で大幅に高速。
pub fn generate_check_evasions(pos: &Position) -> Vec<Move> {
    let color = pos.side_to_move;
    let king_sq = match pos.find_king(color) {
        Some(sq) => sq,
        None => return Vec::new(),
    };
    let opponent = color.opponent();

    let checkers = find_checkers(pos, king_sq, opponent);
    if checkers.is_empty() {
        return pos.generate_legal_moves();
    }

    let mut evasions = Vec::new();
    let mut test = pos.clone();

    // 1. 玉の移動（常に候補）
    for &(df, dr) in &king_offsets() {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let to = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(to) {
            if p.color == color {
                continue;
            }
        }
        let mv = Move::normal(king_sq, to, false, PieceKind::King);
        let undo = test.make_move(mv);
        if !test.is_in_check(color) {
            evasions.push(mv);
        }
        test.unmake_move(&undo);
    }

    // 両王手の場合は玉の移動のみ
    if checkers.len() > 1 {
        return evasions;
    }

    let checker_sq = checkers[0];

    // 2. 王手している駒を取る（玉以外の駒で）
    let capture_moves = generate_moves_to_square(pos, checker_sq, color, false);
    for mv in capture_moves {
        let undo = test.make_move(mv);
        if !test.is_in_check(color) {
            evasions.push(mv);
        }
        test.unmake_move(&undo);
    }

    // 3. 合駒（スライド攻撃の場合のみ）
    let interpose_sqs = compute_interposition_squares(king_sq, checker_sq);
    for sq in &interpose_sqs {
        let interpose_moves = generate_moves_to_square(pos, *sq, color, true);
        for mv in interpose_moves {
            let undo = test.make_move(mv);
            if !test.is_in_check(color) {
                evasions.push(mv);
            }
            test.unmake_move(&undo);
        }
    }

    evasions
}

/// 指定マスに到達可能な指し手を生成（玉以外、合法性チェックなし）
pub fn generate_moves_to_square(
    pos: &Position,
    to: Square,
    color: Color,
    include_drops: bool,
) -> Vec<Move> {
    let mut moves = Vec::new();

    for idx in 0..81 {
        let from = Square::from_index(idx);
        if let Some(piece) = pos.piece_at(from) {
            if piece.color != color || piece.kind == PieceKind::King {
                continue;
            }
            if can_piece_reach(pos, from, piece, to) {
                add_move_with_promotion(from, to, color, piece.kind.can_promote(), piece, &mut moves);
            }
        }
    }

    if include_drops && pos.piece_at(to).is_none() {
        let hand = pos.hand(color);
        for &kind in &PieceKind::HAND_PIECES {
            if !hand.has(kind) {
                continue;
            }
            if !can_drop_to(pos, kind, to, color) {
                continue;
            }
            moves.push(Move::drop(to, kind));
        }
    }

    moves
}

/// 駒がfromからtoに到達可能かチェック（盤上の障害物を考慮）
fn can_piece_reach(pos: &Position, from: Square, piece: Piece, to: Square) -> bool {
    let color = piece.color;
    let df = to.file as i8 - from.file as i8;
    let dr = to.rank as i8 - from.rank as i8;

    match piece.kind {
        PieceKind::King => false,
        PieceKind::Gold
        | PieceKind::PromotedSilver
        | PieceKind::PromotedKnight
        | PieceKind::PromotedLance
        | PieceKind::PromotedPawn => gold_offsets(color).contains(&(df, dr)),
        PieceKind::Silver => silver_offsets(color).contains(&(df, dr)),
        PieceKind::Knight => knight_offsets(color).contains(&(df, dr)),
        PieceKind::Pawn => pawn_offsets(color).contains(&(df, dr)),
        PieceKind::Lance => {
            if df != 0 {
                return false;
            }
            let expected_dr = if color == Color::Sente { -1 } else { 1 };
            if dr.signum() != expected_dr {
                return false;
            }
            is_path_clear(pos, from, 0, expected_dr, to)
        }
        PieceKind::Rook => {
            if (df != 0 && dr != 0) || (df == 0 && dr == 0) {
                return false;
            }
            is_path_clear(pos, from, df.signum(), dr.signum(), to)
        }
        PieceKind::Bishop => {
            if df.abs() != dr.abs() || df == 0 {
                return false;
            }
            is_path_clear(pos, from, df.signum(), dr.signum(), to)
        }
        PieceKind::PromotedRook => {
            if df == 0 && dr == 0 {
                return false;
            }
            if df == 0 || dr == 0 {
                is_path_clear(pos, from, df.signum(), dr.signum(), to)
            } else {
                df.abs() <= 1 && dr.abs() <= 1
            }
        }
        PieceKind::PromotedBishop => {
            if df == 0 && dr == 0 {
                return false;
            }
            if df.abs() == dr.abs() {
                is_path_clear(pos, from, df.signum(), dr.signum(), to)
            } else {
                (df == 0 || dr == 0) && df.abs() + dr.abs() == 1
            }
        }
    }
}

/// fromからtoまでの直線上に障害物がないか（toは含まない）
fn is_path_clear(pos: &Position, from: Square, df: i8, dr: i8, to: Square) -> bool {
    let mut f = from.file as i8 + df;
    let mut r = from.rank as i8 + dr;
    while f != to.file as i8 || r != to.rank as i8 {
        if !Square::is_valid(f, r) {
            return false;
        }
        if pos.piece_at(Square::new(f as u8, r as u8)).is_some() {
            return false;
        }
        f += df;
        r += dr;
    }
    true
}

/// 駒が指定マスに打てるかチェック
fn can_drop_to(pos: &Position, kind: PieceKind, sq: Square, color: Color) -> bool {
    match kind {
        PieceKind::Pawn => {
            if (color == Color::Sente && sq.rank == 1) || (color == Color::Gote && sq.rank == 9) {
                return false;
            }
            if pos.has_pawn_on_file(color, sq.file) {
                return false;
            }
            if is_pawn_drop_mate(pos, sq, color) {
                return false;
            }
            true
        }
        PieceKind::Lance => {
            !((color == Color::Sente && sq.rank == 1) || (color == Color::Gote && sq.rank == 9))
        }
        PieceKind::Knight => {
            !((color == Color::Sente && sq.rank <= 2) || (color == Color::Gote && sq.rank >= 8))
        }
        _ => true,
    }
}

/// 王手している駒の位置を全て見つける
fn find_checkers(pos: &Position, king_sq: Square, attacker_color: Color) -> Vec<Square> {
    let mut checkers = Vec::new();

    // 金型
    for &(df, dr) in &gold_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color
                && matches!(
                    p.kind,
                    PieceKind::Gold
                        | PieceKind::PromotedSilver
                        | PieceKind::PromotedKnight
                        | PieceKind::PromotedLance
                        | PieceKind::PromotedPawn
                )
            {
                checkers.push(sq);
            }
        }
    }

    // 銀
    for &(df, dr) in &silver_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color && p.kind == PieceKind::Silver {
                checkers.push(sq);
            }
        }
    }

    // 桂
    for &(df, dr) in &knight_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color && p.kind == PieceKind::Knight {
                checkers.push(sq);
            }
        }
    }

    // 歩
    for &(df, dr) in &pawn_offsets(attacker_color.opponent()) {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if let Some(p) = pos.piece_at(sq) {
            if p.color == attacker_color && p.kind == PieceKind::Pawn {
                checkers.push(sq);
            }
        }
    }

    // 香
    let lance_dr: i8 = if attacker_color == Color::Sente { 1 } else { -1 };
    if let Some(sq) = find_slide_checker(pos, king_sq, 0, lance_dr, attacker_color, &[PieceKind::Lance]) {
        checkers.push(sq);
    }

    // 飛車/竜（十字スライド）
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        if let Some(sq) = find_slide_checker(
            pos, king_sq, df, dr, attacker_color,
            &[PieceKind::Rook, PieceKind::PromotedRook],
        ) {
            checkers.push(sq);
        }
    }

    // 角/馬（斜めスライド）
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        if let Some(sq) = find_slide_checker(
            pos, king_sq, df, dr, attacker_color,
            &[PieceKind::Bishop, PieceKind::PromotedBishop],
        ) {
            checkers.push(sq);
        }
    }

    // 竜の斜め1マス
    for &(df, dr) in &[(-1i8, -1i8), (-1, 1), (1, -1), (1, 1)] {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if !checkers.contains(&sq) {
            if let Some(p) = pos.piece_at(sq) {
                if p.color == attacker_color && p.kind == PieceKind::PromotedRook {
                    checkers.push(sq);
                }
            }
        }
    }

    // 馬の十字1マス
    for &(df, dr) in &[(0i8, -1i8), (0, 1), (-1, 0), (1, 0)] {
        let nf = king_sq.file as i8 + df;
        let nr = king_sq.rank as i8 + dr;
        if !Square::is_valid(nf, nr) {
            continue;
        }
        let sq = Square::new(nf as u8, nr as u8);
        if !checkers.contains(&sq) {
            if let Some(p) = pos.piece_at(sq) {
                if p.color == attacker_color && p.kind == PieceKind::PromotedBishop {
                    checkers.push(sq);
                }
            }
        }
    }

    checkers
}

/// スライド方向に攻撃駒を探す（位置を返す）
fn find_slide_checker(
    pos: &Position,
    sq: Square,
    df: i8,
    dr: i8,
    color: Color,
    kinds: &[PieceKind],
) -> Option<Square> {
    let mut f = sq.file as i8 + df;
    let mut r = sq.rank as i8 + dr;
    while Square::is_valid(f, r) {
        let check_sq = Square::new(f as u8, r as u8);
        if let Some(piece) = pos.piece_at(check_sq) {
            if piece.color == color && kinds.contains(&piece.kind) {
                return Some(check_sq);
            }
            return None;
        }
        f += df;
        r += dr;
    }
    None
}

/// 王手回避マスの列挙（玉と王手駒の間のマス）
fn compute_interposition_squares(king_sq: Square, checker_sq: Square) -> Vec<Square> {
    let df = checker_sq.file as i8 - king_sq.file as i8;
    let dr = checker_sq.rank as i8 - king_sq.rank as i8;

    let is_line = df == 0 || dr == 0 || df.abs() == dr.abs();
    if !is_line {
        return Vec::new();
    }

    let distance = df.abs().max(dr.abs());
    if distance <= 1 {
        return Vec::new();
    }

    let step_f = df.signum();
    let step_r = dr.signum();

    let mut squares = Vec::new();
    let mut f = king_sq.file as i8 + step_f;
    let mut r = king_sq.rank as i8 + step_r;
    while f != checker_sq.file as i8 || r != checker_sq.rank as i8 {
        squares.push(Square::new(f as u8, r as u8));
        f += step_f;
        r += step_r;
    }

    squares
}

// 各駒の移動オフセット

fn king_offsets() -> [(i8, i8); 8] {
    [
        (-1, -1),
        (0, -1),
        (1, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (0, 1),
        (1, 1),
    ]
}

fn gold_offsets(color: Color) -> [(i8, i8); 6] {
    if color == Color::Sente {
        [(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (0, 1)]
    } else {
        [(-1, 1), (0, 1), (1, 1), (-1, 0), (1, 0), (0, -1)]
    }
}

fn silver_offsets(color: Color) -> [(i8, i8); 5] {
    if color == Color::Sente {
        [(-1, -1), (0, -1), (1, -1), (-1, 1), (1, 1)]
    } else {
        [(-1, 1), (0, 1), (1, 1), (-1, -1), (1, -1)]
    }
}

fn knight_offsets(color: Color) -> [(i8, i8); 2] {
    if color == Color::Sente {
        [(-1, -2), (1, -2)]
    } else {
        [(-1, 2), (1, 2)]
    }
}

fn pawn_offsets(color: Color) -> [(i8, i8); 1] {
    if color == Color::Sente {
        [(0, -1)]
    } else {
        [(0, 1)]
    }
}
