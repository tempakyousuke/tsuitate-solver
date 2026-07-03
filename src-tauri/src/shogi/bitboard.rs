//! 81マス盤面のビットボード（u128、bit = Square::index()）
//!
//! `is_square_attacked` の高速化に使用する。
//! - 近接駒（玉・金類・銀・桂・歩・竜の斜め1マス・馬の十字1マス）は
//!   NEAR_ATTACKER_MASK と色別占有の AND で候補を絞ってから駒種判定
//! - 飛び駒（飛/竜・角/馬・香）はレイマスクと全体占有の AND から
//!   最近接ブロッカーをビット演算で求めて駒種判定

pub type Bitboard = u128;

const fn sq_index(file: i8, rank: i8) -> usize {
    ((file - 1) * 9 + (rank - 1)) as usize
}

const fn in_board(file: i8, rank: i8) -> bool {
    1 <= file && file <= 9 && 1 <= rank && rank <= 9
}

/// 近接攻撃駒の候補マスク: 玉の8近傍 + 桂の攻撃元4マス（先手・後手両方）
pub static NEAR_ATTACKER_MASK: [Bitboard; 81] = build_near_masks();

const fn build_near_masks() -> [Bitboard; 81] {
    let mut masks = [0u128; 81];
    let offsets: [(i8, i8); 12] = [
        (-1, -1), (0, -1), (1, -1),
        (-1, 0), (1, 0),
        (-1, 1), (0, 1), (1, 1),
        (-1, -2), (1, -2), (-1, 2), (1, 2),
    ];
    let mut f = 1i8;
    while f <= 9 {
        let mut r = 1i8;
        while r <= 9 {
            let idx = sq_index(f, r);
            let mut i = 0;
            while i < 12 {
                let (df, dr) = offsets[i];
                if in_board(f + df, r + dr) {
                    masks[idx] |= 1u128 << sq_index(f + df, r + dr);
                }
                i += 1;
            }
            r += 1;
        }
        f += 1;
    }
    masks
}

/// レイ方向（df, dr）
/// 0:(+1,0) 1:(-1,0) 2:(0,+1) 3:(0,-1) 4:(+1,+1) 5:(-1,-1) 6:(+1,-1) 7:(-1,+1)
pub const DIRS: [(i8, i8); 8] = [
    (1, 0), (-1, 0), (0, 1), (0, -1),
    (1, 1), (-1, -1), (1, -1), (-1, 1),
];

/// 各方向がインデックス増加方向か
/// （増加方向なら trailing_zeros、減少方向なら leading_zeros で最近接を求める）
pub const DIR_POSITIVE: [bool; 8] = [true, false, true, false, true, false, true, false];

/// 各マスから各方向のレイマスク（そのマス自身を含まず、盤端まで）
pub static RAYS: [[Bitboard; 81]; 8] = build_rays();

const fn build_rays() -> [[Bitboard; 81]; 8] {
    let mut rays = [[0u128; 81]; 8];
    let mut d = 0;
    while d < 8 {
        let (df, dr) = DIRS[d];
        let mut f = 1i8;
        while f <= 9 {
            let mut r = 1i8;
            while r <= 9 {
                let idx = sq_index(f, r);
                let mut nf = f + df;
                let mut nr = r + dr;
                while in_board(nf, nr) {
                    rays[d][idx] |= 1u128 << sq_index(nf, nr);
                    nf += df;
                    nr += dr;
                }
                r += 1;
            }
            f += 1;
        }
        d += 1;
    }
    rays
}

/// occ 中で idx から dir 方向の最近接マスのインデックスを返す
#[inline]
pub fn nearest_on_ray(occ: Bitboard, dir: usize, idx: usize) -> Option<usize> {
    let blockers = RAYS[dir][idx] & occ;
    if blockers == 0 {
        None
    } else if DIR_POSITIVE[dir] {
        Some(blockers.trailing_zeros() as usize)
    } else {
        Some(127 - blockers.leading_zeros() as usize)
    }
}

/// idx から dir 方向へのスライド到達マス（最近接ブロッカーを含む、味方マスは除外しない）
#[inline]
pub fn slide_targets(occ_all: Bitboard, dir: usize, idx: usize) -> Bitboard {
    let ray = RAYS[dir][idx];
    let blockers = ray & occ_all;
    if blockers == 0 {
        ray
    } else {
        let b_idx = if DIR_POSITIVE[dir] {
            blockers.trailing_zeros() as usize
        } else {
            127 - blockers.leading_zeros() as usize
        };
        // ブロッカーの先のマスを除外（ブロッカー自身は含む＝取りの候補）
        ray & !RAYS[dir][b_idx]
    }
}

/// 全マスのマスク
pub const ALL_MASK: Bitboard = (1u128 << 81) - 1;

/// 段マスク: RANK_MASK[r-1] = rank r の全マス
pub static RANK_MASK: [Bitboard; 9] = build_rank_masks();

const fn build_rank_masks() -> [Bitboard; 9] {
    let mut masks = [0u128; 9];
    let mut f = 1i8;
    while f <= 9 {
        let mut r = 1i8;
        while r <= 9 {
            masks[(r - 1) as usize] |= 1u128 << sq_index(f, r);
            r += 1;
        }
        f += 1;
    }
    masks
}

/// 筋マスク: FILE_MASK[f-1] = file f の全マス
pub static FILE_MASK: [Bitboard; 9] = build_file_masks();

const fn build_file_masks() -> [Bitboard; 9] {
    let mut masks = [0u128; 9];
    let mut f = 1i8;
    while f <= 9 {
        let mut r = 1i8;
        while r <= 9 {
            masks[(f - 1) as usize] |= 1u128 << sq_index(f, r);
            r += 1;
        }
        f += 1;
    }
    masks
}

// ========================================================================
// ステップ駒の移動先テーブル（from → 移動可能マスの集合、盤内のみ）
// 色インデックス: 0 = 先手, 1 = 後手
// ========================================================================

const fn build_step_table(offsets: &[(i8, i8)]) -> [Bitboard; 81] {
    let mut table = [0u128; 81];
    let mut f = 1i8;
    while f <= 9 {
        let mut r = 1i8;
        while r <= 9 {
            let idx = sq_index(f, r);
            let mut i = 0;
            while i < offsets.len() {
                let (df, dr) = offsets[i];
                if in_board(f + df, r + dr) {
                    table[idx] |= 1u128 << sq_index(f + df, r + dr);
                }
                i += 1;
            }
            r += 1;
        }
        f += 1;
    }
    table
}

/// 玉の移動先
pub static KING_STEPS: [Bitboard; 81] = build_step_table(&[
    (-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (-1, 1), (0, 1), (1, 1),
]);

/// 金（および成駒）の移動先 [先手, 後手]
pub static GOLD_STEPS: [[Bitboard; 81]; 2] = [
    build_step_table(&[(-1, -1), (0, -1), (1, -1), (-1, 0), (1, 0), (0, 1)]),
    build_step_table(&[(-1, 1), (0, 1), (1, 1), (-1, 0), (1, 0), (0, -1)]),
];

/// 銀の移動先 [先手, 後手]
pub static SILVER_STEPS: [[Bitboard; 81]; 2] = [
    build_step_table(&[(-1, -1), (0, -1), (1, -1), (-1, 1), (1, 1)]),
    build_step_table(&[(-1, 1), (0, 1), (1, 1), (-1, -1), (1, -1)]),
];

/// 桂の移動先 [先手, 後手]
pub static KNIGHT_STEPS: [[Bitboard; 81]; 2] = [
    build_step_table(&[(-1, -2), (1, -2)]),
    build_step_table(&[(-1, 2), (1, 2)]),
];

/// 歩の移動先 [先手, 後手]
pub static PAWN_STEPS: [[Bitboard; 81]; 2] = [
    build_step_table(&[(0, -1)]),
    build_step_table(&[(0, 1)]),
];

/// 竜の追加移動先（斜め1マス）
pub static DIAG_STEPS: [Bitboard; 81] =
    build_step_table(&[(-1, -1), (-1, 1), (1, -1), (1, 1)]);

/// 馬の追加移動先（十字1マス）
pub static ORTHO_STEPS: [Bitboard; 81] =
    build_step_table(&[(0, -1), (0, 1), (-1, 0), (1, 0)]);
