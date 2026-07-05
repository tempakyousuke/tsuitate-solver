/** 先手/後手 */
export type Color = "sente" | "gote";

/** 駒の種類 */
export type PieceKind =
  | "king"
  | "rook"
  | "bishop"
  | "gold"
  | "silver"
  | "knight"
  | "lance"
  | "pawn"
  | "promoted_rook"
  | "promoted_bishop"
  | "promoted_silver"
  | "promoted_knight"
  | "promoted_lance"
  | "promoted_pawn";

/** 駒 */
export interface Piece {
  color: Color;
  kind: PieceKind;
}

/** 持ち駒 */
export interface HandPiece {
  kind: PieceKind;
  count: number;
}

/** 盤面データ (Rust に送る形式) */
export interface PositionData {
  board: (PieceData | null)[][];
  sente_hand: HandPieceData[];
  gote_hand: HandPieceData[];
}

export interface PieceData {
  color: string;
  kind: string;
}

export interface HandPieceData {
  kind: string;
  count: number;
}

/** 解のデータ */
export interface SolutionData {
  found: boolean;
  tree: SolutionNode | null;
  second_tree: SolutionNode | null;
  kizu_trees: SolutionNode[];
  message: string;
  trace: string[];
}

export type SolutionNode = CheckmateNode | AttackMoveNode;

export interface CheckmateNode {
  Checkmate: {
    depth: number;
    /** 詰め上がり時に攻め方（先手）の持ち駒に残っている枚数（駒余り判定用） */
    hand_count?: number;
  };
}

export interface AttackMoveNode {
  AttackMove: {
    mv: MoveData;
    branches: SolutionBranch[];
  };
}

export interface MoveData {
  from_file: number | null;
  from_rank: number | null;
  to_file: number;
  to_rank: number;
  promotion: boolean;
  drop_piece: string | null;
  notation: string;
}

export interface SolutionBranch {
  observation: Observation;
  continuation: SolutionNode;
}

export type Observation = "NoCapture" | { Captured: { file: number; rank: number } } | "Illegal" | "Checkmate";

/** かくれんぼ問題の情報 */
export interface KakurenboQuestionInfo {
  id: string;
  title: string;
  author: string;
}

/** 駒の漢字表示 */
export const PIECE_KANJI: Record<PieceKind, string> = {
  king: "玉",
  rook: "飛",
  bishop: "角",
  gold: "金",
  silver: "銀",
  knight: "桂",
  lance: "香",
  pawn: "歩",
  promoted_rook: "竜",
  promoted_bishop: "馬",
  promoted_silver: "成銀",
  promoted_knight: "成桂",
  promoted_lance: "成香",
  promoted_pawn: "と",
};

/** 駒種ごとの最大枚数（将棋のルール） */
export const MAX_PIECE_COUNT: Record<string, number> = {
  king: 2,
  rook: 2,
  bishop: 2,
  gold: 4,
  silver: 4,
  knight: 4,
  lance: 4,
  pawn: 18,
};

/** 成駒かどうか */
export function isPromoted(kind: PieceKind): boolean {
  return kind.startsWith("promoted_");
}

/** 成駒を元の駒種に変換（非成駒はそのまま返す） */
export function unpromoted(kind: PieceKind): PieceKind {
  const map: Partial<Record<PieceKind, PieceKind>> = {
    promoted_rook: "rook",
    promoted_bishop: "bishop",
    promoted_silver: "silver",
    promoted_knight: "knight",
    promoted_lance: "lance",
    promoted_pawn: "pawn",
  };
  return map[kind] ?? kind;
}

/** 持ち駒にできる駒種 */
export const HAND_PIECE_KINDS: PieceKind[] = [
  "rook",
  "bishop",
  "gold",
  "silver",
  "knight",
  "lance",
  "pawn",
];

/** 全ての駒種（配置用） */
export const ALL_PIECE_KINDS: PieceKind[] = [
  "king",
  "rook",
  "bishop",
  "gold",
  "silver",
  "knight",
  "lance",
  "pawn",
  "promoted_rook",
  "promoted_bishop",
  "promoted_silver",
  "promoted_knight",
  "promoted_lance",
  "promoted_pawn",
];
