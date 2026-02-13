<script lang="ts">
  import { boardState, senteHand, goteHand, selectedPieceKind, selectedColor } from "./stores";
  import { PIECE_KANJI, HAND_PIECE_KINDS } from "./types";
  import type { Piece, PieceKind, Color, HandState } from "./types";

  // 盤面クリック時の処理
  function onCellClick(file: number, rank: number) {
    boardState.update((board) => {
      const existing = board[file][rank];
      const kind = $selectedPieceKind;
      const color = $selectedColor;

      if (kind === null) {
        // 駒選択なし → 既存の駒を除去
        board[file][rank] = null;
      } else if (existing && existing.color === color && existing.kind === kind) {
        // 同じ駒をクリック → 除去
        board[file][rank] = null;
      } else {
        // 駒を配置
        board[file][rank] = { color, kind };
      }
      return board;
    });
  }

  // 持ち駒クリック
  function onHandClick(color: Color, kind: PieceKind) {
    const hand = color === "sente" ? senteHand : goteHand;
    hand.update((h: HandState) => {
      const current = h.get(kind) || 0;
      if ($selectedColor === color && $selectedPieceKind === kind) {
        // 減らす
        if (current > 0) {
          h.set(kind, current - 1);
          if (h.get(kind) === 0) h.delete(kind);
        }
      } else {
        // 増やす
        h.set(kind, current + 1);
      }
      return h;
    });
  }

  // 駒の表示テキスト
  function pieceText(piece: Piece | null): string {
    if (!piece) return "";
    return PIECE_KANJI[piece.kind];
  }

  // 段の数字（漢数字）
  const rankLabels = ["一", "二", "三", "四", "五", "六", "七", "八", "九"];
  // 筋の数字
  const fileLabels = ["９", "８", "７", "６", "５", "４", "３", "２", "１"];
</script>

<div class="board-container">
  <!-- 後手の持ち駒 -->
  <div class="hand gote-hand">
    <div class="hand-label">△後手 持ち駒</div>
    <div class="hand-pieces">
      {#each HAND_PIECE_KINDS as kind}
        {@const count = $goteHand.get(kind) || 0}
        {#if count > 0}
          <button
            class="hand-piece"
            on:click={() => onHandClick("gote", kind)}
          >
            {PIECE_KANJI[kind]}{count > 1 ? count : ""}
          </button>
        {/if}
      {/each}
      <button
        class="hand-add-btn"
        on:click={() => {
          if ($selectedPieceKind && $selectedPieceKind !== "king") {
            onHandClick("gote", $selectedPieceKind);
          }
        }}
        title="選択中の駒を後手持ち駒に追加"
      >＋</button>
    </div>
  </div>

  <!-- 盤面 -->
  <div class="board">
    <!-- 筋の番号 -->
    <div class="file-labels">
      {#each fileLabels as label}
        <div class="file-label">{label}</div>
      {/each}
    </div>

    <div class="board-grid-wrapper">
      <div class="board-grid">
        {#each { length: 9 } as _, rankIdx}
          {#each { length: 9 } as _, fileDisplayIdx}
            {@const file = 8 - fileDisplayIdx}
            {@const rank = rankIdx}
            {@const piece = $boardState[file][rank]}
            <button
              class="cell"
              class:has-piece={piece !== null}
              class:sente-piece={piece?.color === "sente"}
              class:gote-piece={piece?.color === "gote"}
              on:click={() => onCellClick(file, rank)}
            >
              {#if piece}
                <span class="piece-text" class:gote={piece.color === "gote"}>
                  {pieceText(piece)}
                </span>
              {/if}
            </button>
          {/each}
        {/each}
      </div>

      <!-- 段の番号 -->
      <div class="rank-labels">
        {#each rankLabels as label}
          <div class="rank-label">{label}</div>
        {/each}
      </div>
    </div>
  </div>

  <!-- 先手の持ち駒 -->
  <div class="hand sente-hand">
    <div class="hand-label">▲先手 持ち駒</div>
    <div class="hand-pieces">
      {#each HAND_PIECE_KINDS as kind}
        {@const count = $senteHand.get(kind) || 0}
        {#if count > 0}
          <button
            class="hand-piece"
            on:click={() => onHandClick("sente", kind)}
          >
            {PIECE_KANJI[kind]}{count > 1 ? count : ""}
          </button>
        {/if}
      {/each}
      <button
        class="hand-add-btn"
        on:click={() => {
          if ($selectedPieceKind && $selectedPieceKind !== "king") {
            onHandClick("sente", $selectedPieceKind);
          }
        }}
        title="選択中の駒を先手持ち駒に追加"
      >＋</button>
    </div>
  </div>
</div>

<style>
  .board-container {
    display: flex;
    flex-direction: column;
    align-items: center;
    gap: 8px;
  }

  .hand {
    display: flex;
    flex-direction: column;
    align-items: center;
    padding: 4px 8px;
    min-height: 40px;
  }

  .hand-label {
    font-size: 12px;
    color: #666;
    margin-bottom: 4px;
  }

  .hand-pieces {
    display: flex;
    gap: 4px;
    flex-wrap: wrap;
    justify-content: center;
  }

  .hand-piece {
    background: #fff8e7;
    border: 1px solid #cba135;
    border-radius: 4px;
    padding: 2px 6px;
    font-size: 16px;
    cursor: pointer;
    font-family: serif;
  }

  .hand-piece:hover {
    background: #ffe0a0;
  }

  .hand-add-btn {
    background: #f0f0f0;
    border: 1px dashed #999;
    border-radius: 4px;
    padding: 2px 6px;
    cursor: pointer;
    font-size: 14px;
  }

  .hand-add-btn:hover {
    background: #e0e0e0;
  }

  .board {
    display: flex;
    flex-direction: column;
    align-items: center;
  }

  .file-labels {
    display: grid;
    grid-template-columns: repeat(9, 48px);
    margin-left: 0;
    margin-bottom: 2px;
  }

  .file-label {
    text-align: center;
    font-size: 12px;
    color: #666;
  }

  .board-grid-wrapper {
    display: flex;
    align-items: flex-start;
  }

  .board-grid {
    display: grid;
    grid-template-columns: repeat(9, 48px);
    grid-template-rows: repeat(9, 48px);
    border: 2px solid #333;
    background: #f5d89a;
  }

  .cell {
    width: 48px;
    height: 48px;
    border: 1px solid #b89040;
    display: flex;
    align-items: center;
    justify-content: center;
    cursor: pointer;
    background: transparent;
    padding: 0;
    font-family: serif;
  }

  .cell:hover {
    background: rgba(0, 0, 0, 0.08);
  }

  .piece-text {
    font-size: 22px;
    font-weight: bold;
    color: #1a1a1a;
    user-select: none;
  }

  .piece-text.gote {
    transform: rotate(180deg);
    color: #cc3333;
  }

  .rank-labels {
    display: flex;
    flex-direction: column;
    margin-left: 4px;
  }

  .rank-label {
    height: 48px;
    display: flex;
    align-items: center;
    font-size: 12px;
    color: #666;
  }
</style>
