// 無駄合い研究用の非破壊診断（8_kanryaku.json）。
// ▲１一飛成 直前の情報集合を復元し、反則(無駄合い)枝の合駒構造を調べる。
//   cargo test --release --test futile_research -- --ignored --nocapture

use std::collections::HashSet;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;

use tsuitate_solver_lib::shogi::position::Position;
use tsuitate_solver_lib::shogi::types::*;
use tsuitate_solver_lib::solver::metaposition::MetaPosition;
use tsuitate_solver_lib::solver::solution::{MoveData, Observation, SolutionNode};
use tsuitate_solver_lib::solver::tsuitate_dfpn::{TsuitateDfpnResult, TsuitateDfpnSolver};

/// スライダー（合駒を取って王手を続けられる駒）か
fn is_slider(k: PieceKind) -> bool {
    matches!(
        k,
        PieceKind::Rook | PieceKind::Bishop | PieceKind::Lance
            | PieceKind::PromotedRook | PieceKind::PromotedBishop
    )
}

/// pos（先手＝攻め方の手番）から先手が詰ませられるか（完全情報の詰み検証）
fn sente_can_mate(pos: &Position) -> bool {
    let meta = MetaPosition::new(pos.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    let mut solver = TsuitateDfpnSolver::new(2_000_000, cancel);
    matches!(solver.solve(&meta), TsuitateDfpnResult::Proven)
}

/// q（後手手番・王手されている）から先手が詰ませ切れるか
/// （全ての後手王手回避に対して先手が詰ませられる ＝ AND ノード検証）
fn sente_mates_after(q: &Position) -> bool {
    let evasions = q.generate_check_evasions();
    if evasions.is_empty() {
        return true; // 合法手なし＝詰み
    }
    for ev in evasions {
        let mut r = q.clone();
        r.make_move(ev); // 先手手番
        if !sente_can_mate(&r) {
            return false;
        }
    }
    true
}

/// 無駄合い判定（正しい定義, 条件1-3）
/// pos: 後手手番・王手されている（開き王手など）。def_mv: 後手の合駒。
/// 合駒 S が無駄合い ⟺ ①スライダーで S を取れて王手継続 ②後手が取り返せない
/// ③取った後（スライダー@S）から先手が依然として詰ませられる。
fn muda_ai(pos: &Position, def_mv: &Move) -> bool {
    let s = def_mv.to;
    let mut p = pos.clone();
    p.make_move(*def_mv); // 先手手番

    // 条件1: スライダーで S を取って王手継続する手を探す
    let caps = tsuitate_solver_lib::shogi::movegen::generate_moves_to_square(
        &p, s, Color::Sente, false,
    );
    for cap in caps {
        let Some(from) = cap.from else { continue };
        let Some(piece) = p.piece_at(from) else { continue };
        if !is_slider(piece.kind) {
            continue;
        }
        let mut q = p.clone();
        q.make_move(cap); // 後手手番
        if q.is_in_check(Color::Sente) {
            continue; // 先手玉が王手されるなら不可
        }
        if !q.is_in_check(Color::Gote) {
            continue; // 取った後に王手でなければ対象外
        }
        // 条件2: 後手が S を取り返せない
        let evs = q.generate_check_evasions();
        if evs.iter().any(|m| m.to == s) {
            continue;
        }
        // 条件3: スライダー@S から先手が詰ませられる
        if sente_mates_after(&q) {
            return true;
        }
    }
    false
}


#[derive(serde::Deserialize)]
struct BoardPiece { file: u8, rank: u8, color: String, kind: String }
#[derive(serde::Deserialize)]
struct HandPieceJson { kind: String, count: u8 }
#[derive(serde::Deserialize)]
struct QuestionJson {
    board: Vec<BoardPiece>,
    #[serde(default)]
    sente_hand: Vec<HandPieceJson>,
}

const MAX_PIECES: [(PieceKind, u8); 7] = [
    (PieceKind::Rook, 2), (PieceKind::Bishop, 2), (PieceKind::Gold, 4),
    (PieceKind::Silver, 4), (PieceKind::Knight, 4), (PieceKind::Lance, 4), (PieceKind::Pawn, 18),
];

fn parse_kind(s: &str) -> PieceKind {
    match s {
        "king" => PieceKind::King, "rook" => PieceKind::Rook, "bishop" => PieceKind::Bishop,
        "gold" => PieceKind::Gold, "silver" => PieceKind::Silver, "knight" => PieceKind::Knight,
        "lance" => PieceKind::Lance, "pawn" => PieceKind::Pawn,
        "promoted_rook" => PieceKind::PromotedRook, "promoted_bishop" => PieceKind::PromotedBishop,
        "promoted_silver" => PieceKind::PromotedSilver, "promoted_knight" => PieceKind::PromotedKnight,
        "promoted_lance" => PieceKind::PromotedLance, "promoted_pawn" => PieceKind::PromotedPawn,
        _ => panic!("unknown kind {s}"),
    }
}
fn hand_index(k: PieceKind) -> usize {
    match k.unpromoted() {
        PieceKind::Rook => 0, PieceKind::Bishop => 1, PieceKind::Gold => 2, PieceKind::Silver => 3,
        PieceKind::Knight => 4, PieceKind::Lance => 5, PieceKind::Pawn => 6, _ => panic!("not hand"),
    }
}
fn build_position(q: &QuestionJson) -> Position {
    let mut pos = Position::new();
    let mut used = [0u8; 7];
    for bp in &q.board {
        let kind = parse_kind(&bp.kind);
        let color = if bp.color == "sente" { Color::Sente } else { Color::Gote };
        pos.set_piece(Square::new(bp.file, bp.rank), Piece::new(color, kind));
        if kind.unpromoted() != PieceKind::King { used[hand_index(kind)] += 1; }
    }
    for hp in &q.sente_hand {
        let kind = parse_kind(&hp.kind);
        for _ in 0..hp.count { pos.sente_hand.add(kind); }
        used[hand_index(kind)] += hp.count;
    }
    for &(kind, max) in &MAX_PIECES {
        let idx = hand_index(kind);
        for _ in 0..(max - used[idx]) { pos.gote_hand.add(kind); }
    }
    pos
}
fn kanji_to_kind(s: &str) -> PieceKind {
    match s {
        "飛" => PieceKind::Rook, "角" => PieceKind::Bishop, "金" => PieceKind::Gold,
        "銀" => PieceKind::Silver, "桂" => PieceKind::Knight, "香" => PieceKind::Lance,
        "歩" => PieceKind::Pawn, _ => panic!("drop kanji {s}"),
    }
}
fn to_move(md: &MoveData, meta: &MetaPosition) -> Move {
    if let Some(dp) = &md.drop_piece {
        Move::drop(Square::new(md.to_file, md.to_rank), kanji_to_kind(dp))
    } else {
        let from = Square::new(md.from_file.unwrap(), md.from_rank.unwrap());
        let kind = meta.positions.iter()
            .find_map(|p| p.piece_at(from).filter(|pc| pc.color == Color::Sente).map(|pc| pc.kind))
            .expect("attacker piece at from");
        Move::normal(from, Square::new(md.to_file, md.to_rank), md.promotion, kind)
    }
}
fn find_path(node: &SolutionNode, target: &str, acc: &mut Vec<(MoveData, Observation)>)
    -> Option<(Vec<(MoveData, Observation)>, MoveData)> {
    if let SolutionNode::AttackMove { mv, branches, .. } = node {
        if mv.notation == target && branches.len() >= 2 {
            return Some((acc.clone(), mv.clone()));
        }
        for b in branches {
            if b.observation != Observation::Checkmate {
                acc.push((mv.clone(), b.observation.clone()));
                if let Some(r) = find_path(&b.continuation, target, acc) { return Some(r); }
                acc.pop();
            }
        }
    }
    None
}
fn king_sq(p: &Position) -> String {
    for f in 1..=9 { for r in 1..=9 {
        if let Some(pc) = p.piece_at(Square::new(f, r)) {
            if pc.color == Color::Gote && pc.kind == PieceKind::King {
                return Square::new(f, r).to_japanese();
            }
        }
    }}
    "?".into()
}

/// 後処理方式: 通常求解後に muda_corrected_moves で無駄合いを除外した手数を計算
/// 期待: 8_kanryaku 17→13手、nakaai は変化なし（4一を残すので正しい）
#[test]
#[ignore]
fn posthoc_muda() {
    use std::time::Instant;
    // (ファイルパス, 最短探索するか)
    let cases: [(&str, bool); 4] = [
        ("../8_kanryaku.json", false),
        ("../nakaai.json", false),
        ("../sample-questions/8.json", false),
        ("../sample-questions/8.json", true),
    ];
    for (rel, shortest) in cases {
        let file = format!("{}{}", if shortest { "[最短]" } else { "" }, rel);
        let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
        let q: QuestionJson = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let root = MetaPosition::new(build_position(&q));

        let mut s = TsuitateDfpnSolver::new(50_000_000, Arc::new(AtomicBool::new(false)));
        let t0 = Instant::now();
        let r = s.solve_to_solution(&root, shortest);
        let solve_t = t0.elapsed().as_secs_f64();
        println!("{}: found={} msg={}", file, r.found, r.message);
        let Some(tree) = r.tree else { continue };
        let orig = tree.max_moves();

        let t1 = Instant::now();
        let corrected = tsuitate_solver_lib::solver::tsuitate_dfpn::muda_corrected_moves(&tree, &root);
        let corr_t = t1.elapsed().as_secs_f64();

        println!(
            "{}: 通常={}手 / 無駄合い除外={}手  (求解{:.3}s, 後処理{:.3}s)",
            file, orig, corrected, solve_t, corr_t
        );
    }
}

/// エンドツーエンド: set_omit_futile → solve_to_solution のメッセージが補正手数か
#[test]
#[ignore]
fn e2e_omit_futile() {
    for (rel, shortest) in [("../8_kanryaku.json", true), ("../sample-questions/8.json", true)] {
        let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
        let q: QuestionJson = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let root = MetaPosition::new(build_position(&q));
        let mut s = TsuitateDfpnSolver::new(50_000_000, Arc::new(AtomicBool::new(false)));
        s.set_omit_futile(true);
        let r = s.solve_to_solution(&root, shortest);
        println!("{}: found={} msg={}", rel, r.found, r.message);
    }
}

/// ベンチ範囲で muda_corrected_moves を検証（手数変化・後処理コスト）
#[test]
#[ignore]
fn posthoc_range() {
    use std::time::Instant;
    let lo: u32 = std::env::var("LO").ok().and_then(|s| s.parse().ok()).unwrap_or(1);
    let hi: u32 = std::env::var("HI").ok().and_then(|s| s.parse().ok()).unwrap_or(60);
    let mut changed = Vec::new();
    let mut t_solve = 0.0f64;
    let mut t_post = 0.0f64;
    let mut max_post = 0.0f64;
    for num in lo..=hi {
        let path = format!("{}/../sample-questions/{}.json", env!("CARGO_MANIFEST_DIR"), num);
        let q: QuestionJson = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let root = MetaPosition::new(build_position(&q));
        let mut s = TsuitateDfpnSolver::new(50_000_000, Arc::new(AtomicBool::new(false)));
        let t0 = Instant::now();
        let r = s.solve_to_solution(&root, true); // 最短の木に適用
        t_solve += t0.elapsed().as_secs_f64();
        let Some(tree) = r.tree else { continue };
        let orig = tree.max_moves();
        let t1 = Instant::now();
        let corrected = tsuitate_solver_lib::solver::tsuitate_dfpn::muda_corrected_moves(&tree, &root);
        let dt = t1.elapsed().as_secs_f64();
        t_post += dt;
        if dt > max_post { max_post = dt; }
        if corrected != orig {
            changed.push((num, orig, corrected));
            println!("  no{}: {}手 → {}手 (後処理{:.2}s)", num, orig, corrected, dt);
        } else if dt > 0.3 {
            fn count_nodes(n: &SolutionNode) -> usize {
                match n {
                    SolutionNode::Checkmate { .. } => 1,
                    SolutionNode::AttackMove { branches, .. } => {
                        1 + branches.iter().map(|b| count_nodes(&b.continuation)).sum::<usize>()
                    }
                }
            }
            let t2 = Instant::now();
            let mm = tree.max_moves();
            let mm_t = t2.elapsed().as_secs_f64();
            println!(
                "  no{}: 手数不変({}手) 後処理{:.2}s 木ノード数{} max_moves{:.2}s",
                num, orig, dt, count_nodes(&tree), mm_t
            );
            let _ = mm;
        }
        if corrected > orig {
            println!("  ★no{}: 手数が増加（バグ）", num);
        }
    }
    println!("\n=== 集計 (1..={}) ===", hi);
    println!("手数が変化: {}件 {:?}", changed.len(), changed);
    println!("求解合計 {:.1}s / 後処理合計 {:.1}s (最大 {:.2}s)", t_solve, t_post, max_post);
}

/// nakaai: ▲５一飛成 後の合駒に muda_ai を適用。4一は false であるべき。
#[test]
#[ignore]
fn research_nakaai() {
    let path = format!("{}/../nakaai.json", env!("CARGO_MANIFEST_DIR"));
    let q: QuestionJson = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let mut pos = build_position(&q);
    pos.init_zobrist_hash();
    // ▲５一飛成（飛6一→5一、金を取り成る）
    let mv = Move::normal(Square::new(6, 1), Square::new(5, 1), true, PieceKind::Rook);
    pos.make_move(mv);
    println!("▲５一飛成 後: 後手玉が王手されている = {}", pos.is_in_check(Color::Gote));

    println!("\n合駒応手の判定:");
    for ev in pos.generate_check_evasions() {
        let is_king = ev.from.map_or(false, |f| pos.piece_at(f).map_or(false, |pc| pc.kind == PieceKind::King));
        if is_king { continue; }
        if pos.piece_at(ev.to).is_some() { continue; } // 合駒（空きマス）のみ
        let old = pos.is_futile_interposition(&ev);
        let new = muda_ai(&pos, &ev);
        let kind = ev.drop_piece.map(|k| format!("打{}", k.to_kanji()))
            .unwrap_or_else(|| format!("移動{}", ev.from.and_then(|f| pos.piece_at(f)).map(|pc| pc.kind.to_kanji()).unwrap_or("?")));
        let mark = if new { "無駄合い" } else { "★有効な受け" };
        println!("  {}{}: is_futile={}, muda_ai={} {}", ev.to.to_japanese(), kind, old, new, mark);
    }
}

#[test]
#[ignore]
fn research_kanryaku8() {
    let path = format!("{}/../8_kanryaku.json", env!("CARGO_MANIFEST_DIR"));
    let q: QuestionJson = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let root = MetaPosition::new(build_position(&q));

    let cancelled = Arc::new(AtomicBool::new(false));
    let mut solver = TsuitateDfpnSolver::new(50_000_000, cancelled);
    let sol = solver.solve_to_solution(&root, false);
    let tree = sol.tree.expect("solved");
    println!("solve: {}", sol.message);

    let mut acc = Vec::new();
    let (prefix, imv) = find_path(&tree, "▲１一飛成", &mut acc).expect("▲１一飛成 node");
    println!("\nパス（▲１一飛成 まで）:");
    for (md, obs) in &prefix { println!("  {} -> {:?}", md.notation, obs); }

    // 観測で情報集合を絞りながら決定的リプレイ
    let mut meta = root.clone();
    let mut after_check: Option<MetaPosition> = None;
    for (md, obs) in &prefix {
        let mv = to_move(md, &meta);
        let (legal_after, illegal_before) = meta.apply_attack_move_split(mv);
        meta = if *obs == Observation::Illegal {
            illegal_before
        } else {
            let groups = legal_after.expand_defense_moves(mv);
            let mut positions = Vec::new();
            let mut seen = HashSet::new();
            for (gobs, gmeta) in groups {
                if gobs == *obs {
                    for p in gmeta.positions { if seen.insert(p.zobrist_hash) { positions.push(p); } }
                }
            }
            after_check = Some(legal_after);
            MetaPosition { positions }
        };
    }
    println!("\n▲１一飛成 直前の情報集合 |M| = {}", meta.positions.len());

    // 飛成の合法/非合法で分割
    let mv = to_move(&imv, &meta);
    let (mut l, mut m0): (Vec<Position>, Vec<Position>) = (Vec::new(), Vec::new());
    for p in &meta.positions {
        if p.generate_legal_moves().contains(&mv) { l.push(p.clone()); } else { m0.push(p.clone()); }
    }
    println!("|L(飛成=合法=本線)| = {}, |M0(飛成=反則=無駄合い枝)| = {}", l.len(), m0.len());

    use std::collections::HashMap;
    let mut lk: HashMap<String, usize> = HashMap::new();
    for p in &l { *lk.entry(king_sq(p)).or_insert(0) += 1; }
    let mut mk: HashMap<String, usize> = HashMap::new();
    for p in &m0 { *mk.entry(king_sq(p)).or_insert(0) += 1; }
    println!("L 玉位置: {:?}", lk);
    println!("M0 玉位置: {:?}", mk);

    // M0 にあって L に無い後手駒（合駒炙り出し）
    let mut l_gote: HashSet<(u8, u8)> = HashSet::new();
    for p in &l { for f in 1..=9 { for r in 1..=9 {
        if p.piece_at(Square::new(f, r)).map_or(false, |pc| pc.color == Color::Gote) { l_gote.insert((f, r)); }
    }}}
    let mut extra: HashMap<(u8, u8, String), usize> = HashMap::new();
    for p in &m0 { for f in 1..=9 { for r in 1..=9 {
        if let Some(pc) = p.piece_at(Square::new(f, r)) {
            if pc.color == Color::Gote && !l_gote.contains(&(f, r)) {
                *extra.entry((f, r, pc.kind.to_kanji().into())).or_insert(0) += 1;
            }
        }
    }}}
    let mut ev: Vec<_> = extra.into_iter().collect();
    ev.sort_by_key(|&(_, c)| std::cmp::Reverse(c));
    println!("M0 にあって L に無い後手駒（合駒候補）: {:?}", ev);

    // ▲４二角成 直後（玉方手番）での合駒応手を is_futile と muda_ai で比較
    if let Some(after) = after_check {
        let mut old_ok = 0usize; // is_futile=true
        let mut new_ok = 0usize; // muda_ai=true
        let mut total = 0usize;
        let mut disagree = Vec::new();
        for p in &after.positions {
            for ev in p.generate_check_evasions() {
                let is_king = ev.from.map_or(false, |f| p.piece_at(f).map_or(false, |pc| pc.kind == PieceKind::King));
                if is_king { continue; }
                total += 1;
                let old = p.is_futile_interposition(&ev);
                let new = muda_ai(p, &ev);
                if old { old_ok += 1; }
                if new { new_ok += 1; }
                if old != new {
                    let kind = ev.drop_piece.map(|k| format!("打{}", k.to_kanji()))
                        .unwrap_or_else(|| format!("移動{}", ev.from.and_then(|f| p.piece_at(f)).map(|pc| pc.kind.to_kanji()).unwrap_or("?")));
                    disagree.push(format!("{}{} (is_futile={}, muda_ai={})", ev.to.to_japanese(), kind, old, new));
                }
            }
        }
        println!("\n▲４二角成直後の合駒応手 {}件:", total);
        println!("  is_futile=true: {}件 / muda_ai=true: {}件", old_ok, new_ok);
        println!("  判定が食い違う合駒: {:?}", disagree);
        println!("  → muda_ai が全{}件を無駄合いと判定すべき（正解13手）", total);
    }
}
