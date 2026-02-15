<script lang="ts">
  import {
    boardState, senteHand, goteHand, selectedPieceKind, selectedColor, remainingPieces,
    previewMode, previewHighlight, solution,
    navigateToStart, navigateBackward, navigateForward, navigateToEnd,
  } from "./stores";
  import type { HandState } from "./stores";
  import { PIECE_KANJI, HAND_PIECE_KINDS, unpromoted, isPromoted } from "./types";
  import type { Piece, PieceKind } from "./types";

  // 盤面クリック時の処理
  function onCellClick(file: number, rank: number) {
    if ($previewMode) return;
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
        // 駒数上限チェック（このマスを上書きする場合は現在のマスを除外してカウント）
        const baseKind = unpromoted(kind);
        const remaining = remainingPieces(baseKind, { file, rank });
        if (remaining <= 0) return board; // 上限に達している

        // 二歩チェック: 同じ筋に同じ色の歩が既にある場合は配置不可
        if (kind === "pawn") {
          for (let r = 0; r < 9; r++) {
            if (r === rank) continue;
            const p = board[file][r];
            if (p && p.color === color && p.kind === "pawn") return board;
          }
        }

        board[file][rank] = { color, kind };
      }
      return board;
    });
  }

  // 先手の持ち駒を減らす（既存の持ち駒をクリック）
  function removeSenteHand(kind: PieceKind) {
    if ($previewMode) return;
    senteHand.update((h: HandState) => {
      const current = h.get(kind) || 0;
      if (current > 0) {
        h.set(kind, current - 1);
        if (h.get(kind) === 0) h.delete(kind);
      }
      return h;
    });
  }

  // 先手の持ち駒を増やす（＋ボタン）
  function addSenteHand(kind: PieceKind) {
    if ($previewMode) return;
    senteHand.update((h: HandState) => {
      const remaining = remainingPieces(kind);
      if (remaining <= 0) return h;
      const current = h.get(kind) || 0;
      h.set(kind, current + 1);
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
  <!-- 後手の持ち駒（自動計算・読み取り専用） -->
  <div class="hand gote-hand">
    <div class="hand-label">△後手 持ち駒（残り全て）</div>
    <div class="hand-pieces">
      {#each HAND_PIECE_KINDS as kind}
        {@const count = $goteHand.get(kind) || 0}
        {#if count > 0}
          <span class="hand-piece readonly">
            {PIECE_KANJI[kind]}{count > 1 ? count : ""}
          </span>
        {/if}
      {/each}
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
            {@const hiddenKing = $previewMode && piece?.color === "gote" && piece?.kind === "king"}
            {@const visiblePiece = piece && !hiddenKing}
            {@const hl = $previewHighlight}
            {@const isHighlightFrom = hl !== null && hl.fromFile === file && hl.fromRank === rank}
            {@const isHighlightTo = hl !== null && hl.toFile === file && hl.toRank === rank}
            {@const isIllegalHighlight = isHighlightTo && hl !== null && hl.illegal}
            <button
              class="cell"
              class:has-piece={visiblePiece}
              class:sente-piece={visiblePiece && piece?.color === "sente"}
              class:gote-piece={visiblePiece && piece?.color === "gote"}
              class:highlight-from={isHighlightFrom}
              class:highlight-to={isHighlightTo && !isIllegalHighlight}
              class:highlight-illegal={isIllegalHighlight}
              class:preview={$previewMode}
              on:click={() => onCellClick(file, rank)}
            >
              {#if visiblePiece}
                <span class="piece-text" class:gote={piece.color === "gote"} class:promoted={isPromoted(piece.kind)}>
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
            on:click={() => removeSenteHand(kind)}
          >
            {PIECE_KANJI[kind]}{count > 1 ? count : ""}
          </button>
        {/if}
      {/each}
      <button
        class="hand-add-btn"
        on:click={() => {
          if ($selectedPieceKind && $selectedPieceKind !== "king") {
            addSenteHand(unpromoted($selectedPieceKind));
          }
        }}
        title="選択中の駒を先手持ち駒に追加"
      >＋</button>
    </div>
  </div>

  <!-- リプレイ操作 -->
  {#if $solution?.tree}
    <div class="replay-controls">
      <button class="replay-btn" on:click={navigateToStart} disabled={!$previewMode} title="最初に戻る">|&lt;</button>
      <button class="replay-btn" on:click={navigateBackward} disabled={!$previewMode} title="前の手">&lt;</button>
      <button class="replay-btn" on:click={navigateForward} title="次の手">&gt;</button>
      <button class="replay-btn" on:click={navigateToEnd} title="最後まで進む">&gt;|</button>
    </div>
  {/if}
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

  .hand-piece:hover:not(.readonly) {
    background: #ffe0a0;
  }

  .hand-piece.readonly {
    cursor: default;
    opacity: 0.8;
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

  .cell:hover:not(.preview) {
    background: rgba(0, 0, 0, 0.08);
  }

  .cell.preview {
    cursor: default;
  }

  .cell.highlight-from {
    background: rgba(80, 160, 80, 0.25);
  }

  .cell.highlight-to {
    background: rgba(60, 160, 60, 0.35);
  }

  .cell.highlight-illegal {
    background: rgba(220, 80, 50, 0.35);
  }

  .piece-text {
    font-size: 22px;
    font-weight: bold;
    color: #1a1a1a;
    user-select: none;
  }

  .piece-text.promoted {
    color: #cc3333;
  }

  .piece-text.gote {
    transform: rotate(180deg);
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

  .replay-controls {
    display: flex;
    gap: 6px;
    justify-content: center;
    margin-top: 4px;
  }

  .replay-btn {
    width: 40px;
    height: 32px;
    border: 1px solid #aaa;
    border-radius: 4px;
    background: #fff;
    font-size: 16px;
    font-weight: bold;
    color: #333;
    cursor: pointer;
    display: flex;
    align-items: center;
    justify-content: center;
    font-family: monospace;
  }

  .replay-btn:hover:not(:disabled) {
    background: #e8e8e8;
  }

  .replay-btn:disabled {
    opacity: 0.35;
    cursor: default;
  }
</style>
