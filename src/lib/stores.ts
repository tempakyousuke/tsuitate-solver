import { writable, derived, get } from "svelte/store";
import type { Piece, PieceKind, Color, SolutionData, SolutionNode, SolutionBranch, MoveData, Observation } from "./types";
import { unpromoted, MAX_PIECE_COUNT, HAND_PIECE_KINDS, PIECE_KANJI } from "./types";

/** 盤面のディープコピー */
export function deepCopyBoard(board: BoardState): BoardState {
  return board.map(file => file.map(cell => cell ? { ...cell } : null));
}

/** 盤面の型: 9x9 配列 (board[file][rank], 0-indexed) */
export type BoardState = (Piece | null)[][];

/** 持ち駒の型 */
export type HandState = Map<PieceKind, number>;

/** 空の盤面を作成 */
function createEmptyBoard(): BoardState {
  return Array.from({ length: 9 }, () => Array.from({ length: 9 }, () => null));
}

/** 空の持ち駒を作成 */
function createEmptyHand(): HandState {
  return new Map();
}

/** 盤面状態 */
export const boardState = writable<BoardState>(createEmptyBoard());

/** 先手の持ち駒 */
export const senteHand = writable<HandState>(createEmptyHand());

/**
 * 後手の持ち駒（自動計算）
 * 詰将棋のルール: 盤上と先手の持ち駒にない駒は全て後手の持ち駒（玉を除く）
 */
export const goteHand = derived(
  [boardState, senteHand],
  ([$board, $sHand]) => {
    const hand: HandState = new Map();

    for (const baseKind of HAND_PIECE_KINDS) {
      const max = MAX_PIECE_COUNT[baseKind] ?? 0;
      let used = 0;

      // 盤面上の駒を数える（両方の色、成駒も含む）
      for (let f = 0; f < 9; f++) {
        for (let r = 0; r < 9; r++) {
          const p = $board[f][r];
          if (p && unpromoted(p.kind) === baseKind) used++;
        }
      }

      // 先手の持ち駒を数える
      if ($sHand.has(baseKind)) used += $sHand.get(baseKind)!;

      const remaining = max - used;
      if (remaining > 0) {
        hand.set(baseKind, remaining);
      }
    }

    return hand;
  },
);

/** 選択中の駒種 */
export const selectedPieceKind = writable<PieceKind | null>(null);

/** 選択中の色 */
export const selectedColor = writable<Color>("sente");

/** 解答結果 */
export const solution = writable<SolutionData | null>(null);

/** 求解中フラグ */
export const solving = writable<boolean>(false);

/** エラーメッセージ */
export const errorMessage = writable<string>("");

/** 2つ目の解を探すかどうか（余詰めチェック） */
export const findSecondSolution = writable<boolean>(false);

/** 最短経路を探すかどうか */
export const findShortestPath = writable<boolean>(false);

/** 無駄合いを省くかどうか（玉方は無駄合いを着手しない慣例で手数を数える）。
 *  正しく機能させるため最短探索とペアで使う。 */
export const omitFutile = writable<boolean>(false);

/** 現在の問題タイトル */
export const questionTitle = writable<string>("");

/** 現在の問題作者 */
export const questionAuthor = writable<string>("");

/** 初期盤面（解探索時に保存） */
export const initialPosition = writable<{ board: BoardState; senteHand: HandState } | null>(null);

/** プレビューモード */
export const previewMode = writable<boolean>(false);

/** プレビュー時のハイライト（0-indexed 座標） */
export const previewHighlight = writable<{
  fromFile: number | null;
  fromRank: number | null;
  toFile: number;
  toRank: number;
  illegal: boolean;
} | null>(null);

/** 初期盤面を保存 */
export function saveInitialPosition() {
  const board = deepCopyBoard(get(boardState));
  const hand = new Map(get(senteHand));
  initialPosition.set({ board, senteHand: hand });
}

/** 選択中の手順パス */
export const selectedPath = writable<string | null>(null);

/** プレビューモードを終了して初期盤面に戻す */
export function exitPreview() {
  if (get(previewMode)) {
    const initial = get(initialPosition);
    if (initial) {
      boardState.set(deepCopyBoard(initial.board));
      senteHand.set(new Map(initial.senteHand));
    }
  }
  previewMode.set(false);
  previewHighlight.set(null);
  selectedPath.set(null);
}

// ─── リプレイ関連 ───

/** 漢字 → 駒種の逆引き */
const KANJI_TO_KIND: Record<string, PieceKind> = Object.fromEntries(
  Object.entries(PIECE_KANJI).map(([kind, kanji]) => [kanji, kind as PieceKind]),
);

/** SolutionNode が AttackMove か判定 */
export function isAttackMove(node: SolutionNode): node is {
  AttackMove: { mv: MoveData; branches: SolutionBranch[] };
} {
  return "AttackMove" in node;
}

/** Observation が Captured か判定 */
export function isCaptured(obs: Observation): obs is { Captured: { file: number; rank: number } } {
  return typeof obs === "object" && obs !== null && "Captured" in obs;
}

/** 駒を成る */
function promotePiece(kind: PieceKind): PieceKind {
  const map: Partial<Record<PieceKind, PieceKind>> = {
    rook: "promoted_rook", bishop: "promoted_bishop",
    silver: "promoted_silver", knight: "promoted_knight",
    lance: "promoted_lance", pawn: "promoted_pawn",
  };
  return map[kind] ?? kind;
}

/** 盤面に攻め方の手を適用（座標は 1-indexed） */
function applyMoveToBoard(board: BoardState, hand: HandState, mv: MoveData) {
  const toF = mv.to_file - 1;
  const toR = mv.to_rank - 1;

  if (mv.drop_piece) {
    const kind = KANJI_TO_KIND[mv.drop_piece];
    if (kind) {
      board[toF][toR] = { color: "sente", kind };
      const count = hand.get(kind) || 0;
      if (count > 1) hand.set(kind, count - 1);
      else hand.delete(kind);
    }
  } else if (mv.from_file !== null && mv.from_rank !== null) {
    const fromF = mv.from_file - 1;
    const fromR = mv.from_rank - 1;
    const piece = board[fromF][fromR];
    board[fromF][fromR] = null;
    if (piece) {
      const kind = mv.promotion ? promotePiece(piece.kind) : piece.kind;
      const captured = board[toF][toR];
      if (captured && captured.color === "gote") {
        const unprom = unpromoted(captured.kind);
        if (unprom !== "king") {
          const count = hand.get(unprom) || 0;
          hand.set(unprom, count + 1);
        }
      }
      board[toF][toR] = { color: "sente", kind };
    }
  }
}

/**
 * 盤面を指定パスまでリプレイ。
 * applyFinalMove=true: パス末端のノードの手も適用（指手用）
 * applyFinalMove=false: パス末端のノードの手は適用しない（観測用）
 */
export function replayToPath(pathStr: string, applyFinalMove: boolean): { board: BoardState; hand: HandState; lastMove: MoveData | null } | null {
  const initial = get(initialPosition);
  if (!initial) return null;

  const sol = get(solution);
  const prefix = pathStr.split("/")[0];
  let tree: SolutionNode | null | undefined;
  if (prefix === "s") {
    tree = sol?.second_tree;
  } else if (prefix.startsWith("k")) {
    const idx = parseInt(prefix.substring(1));
    tree = sol?.kizu_trees?.[idx];
  } else {
    tree = sol?.tree;
  }
  if (!tree) return null;

  const board = deepCopyBoard(initial.board);
  const hand = new Map(initial.senteHand);

  const indices = pathStr.split("/").slice(1).map(Number);
  let current: SolutionNode = tree;
  let lastMove: MoveData | null = null;

  for (const idx of indices) {
    if (!isAttackMove(current)) break;
    const mv = current.AttackMove.mv;
    const branch = current.AttackMove.branches[idx];

    if (branch.observation !== "Illegal") {
      applyMoveToBoard(board, hand, mv);
    }
    if (isCaptured(branch.observation)) {
      const cf = branch.observation.Captured.file - 1;
      const cr = branch.observation.Captured.rank - 1;
      board[cf][cr] = null;
    }

    lastMove = mv;
    current = branch.continuation;
  }

  if (applyFinalMove && isAttackMove(current)) {
    lastMove = current.AttackMove.mv;
    applyMoveToBoard(board, hand, lastMove);
  }

  return { board, hand, lastMove };
}

/** プレビュー結果を盤面に反映 */
function applyPreviewResult(result: { board: BoardState; hand: HandState; lastMove: MoveData | null }, key: string, illegal = false) {
  boardState.set(result.board);
  senteHand.set(result.hand);
  previewMode.set(true);
  selectedPath.set(key);

  if (result.lastMove) {
    previewHighlight.set({
      fromFile: result.lastMove.from_file !== null ? result.lastMove.from_file - 1 : null,
      fromRank: result.lastMove.from_rank !== null ? result.lastMove.from_rank - 1 : null,
      toFile: result.lastMove.to_file - 1,
      toRank: result.lastMove.to_rank - 1,
      illegal,
    });
  } else {
    previewHighlight.set(null);
  }
}

/** 指手クリック → その手まで盤面をリプレイ */
export function selectMove(pathStr: string) {
  if (get(selectedPath) === pathStr) { exitPreview(); return; }
  const result = replayToPath(pathStr, true);
  if (!result) return;
  applyPreviewResult(result, pathStr);
}

/** 観測クリック → 手＋観測結果まで盤面をリプレイ */
export function selectObservation(parentPath: string, branchIdx: number) {
  const obsKey = `${parentPath}/${branchIdx}!`;
  if (get(selectedPath) === obsKey) { exitPreview(); return; }
  const result = replayToPath(`${parentPath}/${branchIdx}`, false);
  if (!result) return;

  // 反則かどうか判定
  const tree = getTreeForPath(parentPath);
  let illegal = false;
  if (tree) {
    const parentNode = getNodeAtPath(tree, parentPath);
    if (parentNode && isAttackMove(parentNode)) {
      illegal = parentNode.AttackMove.branches[branchIdx]?.observation === "Illegal";
    }
  }

  applyPreviewResult(result, obsKey, illegal);
}

/** パス末端のノードを取得 */
function getNodeAtPath(tree: SolutionNode, pathStr: string): SolutionNode | null {
  const indices = pathStr.split("/").slice(1).map(Number);
  let current: SolutionNode = tree;
  for (const idx of indices) {
    if (!isAttackMove(current)) return null;
    const branch = current.AttackMove.branches[idx];
    if (!branch) return null;
    current = branch.continuation;
  }
  return current;
}

/** ツリーからプレフィックスに応じたルートを取得 */
function getTreeForPath(pathStr: string): SolutionNode | null {
  const sol = get(solution);
  if (!sol) return null;
  if (pathStr.startsWith("s")) return sol.second_tree ?? null;
  if (pathStr.startsWith("k")) {
    const idx = parseInt(pathStr.substring(1));
    return sol.kizu_trees?.[idx] ?? null;
  }
  return sol.tree ?? null;
}

/** 前進（次の手へ / 次の観測へ） */
export function navigateForward() {
  const path = get(selectedPath);

  if (!path) {
    // 未選択 → ルートの手へ
    const sol = get(solution);
    if (sol?.tree && isAttackMove(sol.tree)) {
      selectMove("t");
    }
    return;
  }

  if (path.endsWith("!")) {
    // 観測 → 続きの指手へ
    const obsPath = path.slice(0, -1); // "t/0!" → "t/0"
    const tree = getTreeForPath(obsPath);
    if (!tree) return;

    const parentParts = obsPath.split("/");
    const branchIdx = Number(parentParts.pop());
    const parentPath = parentParts.join("/");
    const parentNode = getNodeAtPath(tree, parentPath);
    if (!parentNode || !isAttackMove(parentNode)) return;

    const branch = parentNode.AttackMove.branches[branchIdx];
    if (!branch || branch.observation === "Checkmate") return;

    if (isAttackMove(branch.continuation)) {
      selectMove(obsPath);
    }
    return;
  }

  // 指手 → 最初の観測へ
  const tree = getTreeForPath(path);
  if (!tree) return;
  const node = getNodeAtPath(tree, path);
  if (!node || !isAttackMove(node)) return;

  if (node.AttackMove.branches.length === 0) return;

  // デフォルト分岐: 最初の非 Checkmate で続きがある分岐
  let defaultIdx = -1;
  for (let i = 0; i < node.AttackMove.branches.length; i++) {
    const branch = node.AttackMove.branches[i];
    if (branch.observation !== "Checkmate" && isAttackMove(branch.continuation)) {
      defaultIdx = i;
      break;
    }
  }

  // 見つからなければ最初の分岐（Checkmate で終端）
  if (defaultIdx === -1) defaultIdx = 0;

  selectObservation(path, defaultIdx);
}

/** 後退（前の観測へ / 前の手へ） */
export function navigateBackward() {
  const path = get(selectedPath);
  if (!path) return;

  if (path.endsWith("!")) {
    // 観測 → その手に戻る
    const obsPath = path.slice(0, -1); // "t/0!" → "t/0"
    const parentParts = obsPath.split("/");
    parentParts.pop(); // 分岐インデックスを除去
    const movePath = parentParts.join("/"); // "t"
    selectMove(movePath);
    return;
  }

  // 指手 → 前の観測に戻る
  const parts = path.split("/");

  if (parts.length <= 1) {
    // ルートの手 → 初期盤面に戻る
    exitPreview();
    return;
  }

  // "t/0" → selectObservation("t", 0) で "t/0!" へ
  const branchIdx = Number(parts.pop());
  const parentPath = parts.join("/");
  selectObservation(parentPath, branchIdx);
}

/** 最初に戻る */
export function navigateToStart() {
  exitPreview();
}

/** 最後まで進む */
export function navigateToEnd() {
  const sol = get(solution);
  if (!sol) return;

  let path = get(selectedPath);
  if (!path) {
    if (!sol.tree || !isAttackMove(sol.tree)) return;
    path = "t";
  }

  const movePath = path.endsWith("!") ? path.slice(0, -1) : path;
  const tree = getTreeForPath(movePath);
  if (!tree) return;

  // 最初の非 Checkmate 分岐を辿り続ける
  let currentPath = movePath;
  let node = getNodeAtPath(tree, currentPath);

  while (node && isAttackMove(node)) {
    let found = false;
    for (let i = 0; i < node.AttackMove.branches.length; i++) {
      const branch = node.AttackMove.branches[i];
      if (branch.observation !== "Checkmate" && isAttackMove(branch.continuation)) {
        currentPath = `${currentPath}/${i}`;
        node = branch.continuation;
        found = true;
        break;
      }
    }
    if (!found) break;
  }

  selectMove(currentPath);
}

/**
 * 盤面＋先手持ち駒における、指定した基本駒種（unpromoted）の使用数を返す。
 * excludeSquare を指定すると、そのマスの駒はカウントから除外する（上書き配置時用）。
 */
function countUsedPieces(
  baseKind: PieceKind,
  excludeSquare?: { file: number; rank: number },
): number {
  const board = get(boardState);
  const sHand = get(senteHand);

  let count = 0;

  // 盤面上の駒を数える
  for (let f = 0; f < 9; f++) {
    for (let r = 0; r < 9; r++) {
      if (excludeSquare && excludeSquare.file === f && excludeSquare.rank === r) continue;
      const p = board[f][r];
      if (p && unpromoted(p.kind) === baseKind) count++;
    }
  }

  // 先手の持ち駒を数える
  if (sHand.has(baseKind)) count += sHand.get(baseKind)!;

  return count;
}

/** 指定した駒種をあと何枚配置できるか（盤面＋先手持ち駒の合計が上限未満か） */
export function remainingPieces(baseKind: PieceKind, excludeSquare?: { file: number; rank: number }): number {
  const max = MAX_PIECE_COUNT[baseKind] ?? 0;
  return max - countUsedPieces(baseKind, excludeSquare);
}

/** 盤面をクリア */
export function clearBoard() {
  boardState.set(createEmptyBoard());
  senteHand.set(createEmptyHand());
  solution.set(null);
  errorMessage.set("");
  initialPosition.set(null);
  previewMode.set(false);
  previewHighlight.set(null);
  selectedPath.set(null);
  questionTitle.set("");
  questionAuthor.set("");
}
