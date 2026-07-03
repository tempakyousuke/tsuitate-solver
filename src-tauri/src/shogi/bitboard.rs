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
