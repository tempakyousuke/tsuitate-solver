<script lang="ts">
  import { selectedPieceKind, selectedColor } from "./stores";
  import { PIECE_KANJI, ALL_PIECE_KINDS } from "./types";
  import type { PieceKind, Color } from "./types";

  function selectPiece(kind: PieceKind) {
    if ($selectedPieceKind === kind) {
      selectedPieceKind.set(null);
    } else {
      selectedPieceKind.set(kind);
    }
  }

  function selectColor(color: Color) {
    selectedColor.set(color);
  }
</script>

<div class="piece-selector">
  <div class="color-selector">
    <button
      class="color-btn"
      class:active={$selectedColor === "sente"}
      on:click={() => selectColor("sente")}
    >
      ▲先手
    </button>
    <button
      class="color-btn"
      class:active={$selectedColor === "gote"}
      on:click={() => selectColor("gote")}
    >
      △後手
    </button>
  </div>

  <div class="pieces-grid">
    {#each ALL_PIECE_KINDS as kind}
      <button
        class="piece-btn"
        class:selected={$selectedPieceKind === kind}
        on:click={() => selectPiece(kind)}
        title={kind}
      >
        {PIECE_KANJI[kind]}
      </button>
    {/each}
  </div>

  <div class="hint">
    {#if $selectedPieceKind}
      選択中: {$selectedColor === "sente" ? "▲" : "△"}{PIECE_KANJI[$selectedPieceKind]}
      <button class="deselect-btn" on:click={() => selectedPieceKind.set(null)}>解除</button>
    {:else}
      駒を選択して盤面をクリック（未選択でクリックすると駒を除去）
    {/if}
  </div>
</div>

<style>
  .piece-selector {
    display: flex;
    flex-direction: column;
    gap: 8px;
    padding: 8px;
    background: #f8f8f8;
    border-radius: 8px;
    border: 1px solid #ddd;
  }

  .color-selector {
    display: flex;
    gap: 8px;
  }

  .color-btn {
    flex: 1;
    padding: 6px 12px;
    border: 2px solid #ccc;
    border-radius: 4px;
    background: #fff;
    cursor: pointer;
    font-size: 14px;
    font-weight: bold;
  }

  .color-btn.active {
    border-color: #4a90d9;
    background: #e8f0fe;
    color: #1a56db;
  }

  .pieces-grid {
    display: grid;
    grid-template-columns: repeat(7, 1fr);
    gap: 4px;
  }

  .piece-btn {
    padding: 8px 4px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    cursor: pointer;
    font-size: 18px;
    font-family: serif;
    font-weight: bold;
  }

  .piece-btn:hover {
    background: #f0f0f0;
  }

  .piece-btn.selected {
    border-color: #4a90d9;
    background: #e8f0fe;
    box-shadow: 0 0 0 2px rgba(74, 144, 217, 0.3);
  }

  .hint {
    font-size: 12px;
    color: #666;
    text-align: center;
  }

  .deselect-btn {
    margin-left: 8px;
    padding: 2px 8px;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: #fff;
    cursor: pointer;
    font-size: 11px;
  }

  .deselect-btn:hover {
    background: #f0f0f0;
  }
</style>
