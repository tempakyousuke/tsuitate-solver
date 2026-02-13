import { writable } from "svelte/store";
import type { Piece, PieceKind, Color, SolutionData } from "./types";

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

/** 後手の持ち駒 */
export const goteHand = writable<HandState>(createEmptyHand());

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

/** 最大探索手数 */
export const maxDepth = writable<number>(7);

/** 盤面をクリア */
export function clearBoard() {
  boardState.set(createEmptyBoard());
  senteHand.set(createEmptyHand());
  goteHand.set(createEmptyHand());
  solution.set(null);
  errorMessage.set("");
}
