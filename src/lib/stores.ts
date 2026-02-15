import { writable, derived, get } from "svelte/store";
import type { Piece, PieceKind, Color, SolutionData } from "./types";
import { unpromoted, MAX_PIECE_COUNT, HAND_PIECE_KINDS } from "./types";

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
}
