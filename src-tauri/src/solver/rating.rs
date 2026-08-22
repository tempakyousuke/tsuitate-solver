//! 問題ごとの難易度レート推定（サイトの「詰めチャレ」の初期レート用）。
//!
//! サイト側（tsuitate）の詰めチャレでは、問題とプレイヤーの双方に Elo レートを
//! 持たせて対戦させる。プレイヤーが解けば問題のレートは下がり、解けなければ
//! 上がる。つまり**問題レートは実戦で自己補正される**ので、ここで求めるのは
//! 「事前分布（初期値）」であって真の難易度そのものではない。
//!
//! ## 特徴量
//!
//! いずれもソルバーが解答を求める過程で得られる決定的な量で、乱数も実測時間も
//! 使わない（同じ問題なら常に同じレートになる）。
//!
//! - `depth` … 詰み手数。人間の体感難易度に最も効く
//! - `root_tries` … 初期局面で攻め方が**指せてしまう**手の数。ついたて詰将棋では
//!   反則にならない手＝情報集合のどれかの盤面で合法な手で、初期局面は
//!   盤面が1つに確定しているので単純に合法手の数になる。候補が広いほど迷う
//! - `root_checks` … そのうち「王手が保証される」手の数。ついたて詰将棋では
//!   王手が保証されない手を指した時点で詰め失敗なので、解き手が本気で読むのは
//!   この集合。難易度の分母にはこちらを使う（`root_tries` はどの問題でも
//!   100 前後でほぼ定数になり、識別力がない。記録は校正用に残す）
//! - `root_solutions` … そのうち実際に詰みに繋がる手の数。1 手しかない問題は
//!   当てずっぽうが通らない
//! - `solution_branches` … 解答手順木の観測分岐の総数。読む量そのもの
//! - `max_branch` … 1 ノードあたりの最大分岐数
//! - `nodes` … 本解を求めるまでに探索したノード数。ソルバーにとっての重さ
//! - `has_second` … 余詰めあり。別解が通るぶん当たりやすい
//!
//! ## 重み
//!
//! v1 の係数は**実測データのない暫定値**で、詰めチャレの実戦結果（問題レートの
//! 収束先）と突き合わせて校正する前提で置いている。特徴量は出力にそのまま
//! 載せてあるので、後から重みだけを引き直せる。

use serde::Serialize;

use super::solution::{Observation, SolutionNode};

/// 推定式のバージョン（サイト側が「どの式で付いたレートか」を記録する）
pub const FORMULA_VERSION: &str = "v1";

const BASE: f64 = 700.0;
/// 攻め方の手が 1 手増えるごと（＝手数 +2）
const W_DEPTH: f64 = 130.0;
/// log2(root_checks / root_solutions)
const W_NARROW: f64 = 110.0;
/// log2(1 + solution_branches)
const W_BRANCH: f64 = 35.0;
/// log10(1 + nodes)
const W_NODES: f64 = 25.0;
/// 余詰めあり
const W_SECOND: f64 = -60.0;

pub const RATING_MIN: i32 = 600;
pub const RATING_MAX: i32 = 2800;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingFeatures {
    pub depth: u32,
    pub root_tries: u32,
    pub root_checks: u32,
    pub root_solutions: u32,
    pub solution_branches: u32,
    pub max_branch: u32,
    pub nodes: u64,
    pub has_second: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RatingEstimate {
    pub value: i32,
    pub formula: &'static str,
    pub features: RatingFeatures,
}

/// 特徴量から初期レートを求める（純関数）
pub fn estimate_rating(f: &RatingFeatures) -> i32 {
    // 手数は奇数（1, 3, 5, ...）。攻め方の手数 - 1 を効かせる
    let attacker_moves = (f.depth.max(1) + 1) / 2;
    let depth_term = W_DEPTH * (attacker_moves.saturating_sub(1)) as f64;

    // 正解手が 0 と報告されることは本来ないが（本解があるから解けている）、
    // ノード上限で root_solutions を数え切れなかった場合に 0 になり得る。
    // その場合は「1 手だけ正解」とみなす（最も絞りにくい側に倒す）
    let solutions = f.root_solutions.max(1) as f64;
    let checks = (f.root_checks.max(1) as f64).max(solutions);
    let narrow_term = W_NARROW * (checks / solutions).log2();

    let branch_term = W_BRANCH * (1.0 + f.solution_branches as f64).log2();
    let node_term = W_NODES * (1.0 + f.nodes as f64).log10();
    let second_term = if f.has_second { W_SECOND } else { 0.0 };

    let raw = BASE + depth_term + narrow_term + branch_term + node_term + second_term;
    (raw.round() as i32).clamp(RATING_MIN, RATING_MAX)
}

/// 解答手順木の分岐統計（観測分岐の総数と 1 ノードの最大分岐数）
pub fn tree_branch_stats(tree: &SolutionNode) -> (u32, u32) {
    let mut total = 0u32;
    let mut max = 0u32;
    walk(tree, &mut total, &mut max);
    (total, max)
}

fn walk(node: &SolutionNode, total: &mut u32, max: &mut u32) {
    let SolutionNode::AttackMove { branches, .. } = node else {
        return;
    };
    // 詰み・反則の葉は「読む分岐」に数えない（応手の分岐だけを数える）
    let real = branches
        .iter()
        .filter(|b| matches!(b.observation, Observation::NoCapture | Observation::Captured { .. }))
        .count() as u32;
    *total += real;
    *max = (*max).max(real);
    for b in branches {
        walk(&b.continuation, total, max);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn features() -> RatingFeatures {
        RatingFeatures {
            depth: 1,
            root_tries: 60,
            root_checks: 8,
            root_solutions: 1,
            solution_branches: 0,
            max_branch: 0,
            nodes: 500,
            has_second: false,
        }
    }

    #[test]
    fn longer_mates_rate_higher() {
        let short = estimate_rating(&features());
        let long = estimate_rating(&RatingFeatures { depth: 7, ..features() });
        assert!(long > short, "{long} > {short}");
    }

    #[test]
    fn many_correct_first_moves_rate_lower() {
        let narrow = estimate_rating(&features());
        let wide = estimate_rating(&RatingFeatures { root_solutions: 8, ..features() });
        assert!(wide < narrow, "{wide} < {narrow}");
    }

    #[test]
    fn second_solution_rates_lower() {
        let clean = estimate_rating(&features());
        let flawed = estimate_rating(&RatingFeatures { has_second: true, ..features() });
        assert_eq!(flawed, clean + W_SECOND as i32);
    }

    #[test]
    fn stays_in_range() {
        let tiny = estimate_rating(&RatingFeatures {
            depth: 1,
            root_tries: 1,
            root_checks: 1,
            root_solutions: 1,
            solution_branches: 0,
            max_branch: 0,
            nodes: 0,
            has_second: true,
        });
        assert!((RATING_MIN..RATING_MAX).contains(&tiny), "{tiny}");
        let huge = estimate_rating(&RatingFeatures {
            depth: 99,
            root_tries: 200,
            root_checks: 100,
            root_solutions: 1,
            solution_branches: 100_000,
            max_branch: 50,
            nodes: 10_000_000_000,
            has_second: false,
        });
        assert_eq!(huge, RATING_MAX);
    }

    #[test]
    fn zero_solutions_is_treated_as_one() {
        let a = estimate_rating(&RatingFeatures { root_solutions: 0, ..features() });
        let b = estimate_rating(&RatingFeatures { root_solutions: 1, ..features() });
        assert_eq!(a, b);
    }
}
