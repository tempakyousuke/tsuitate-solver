<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import {
    boardState,
    senteHand,
    goteHand,
    solution,
    solving,
    errorMessage,
    maxDepth,
    clearBoard,
  } from "./stores";
  import type { PositionData, HandPieceData, SolutionData } from "./types";
  import type { BoardState, HandState } from "./stores";

  function buildPositionData(
    board: BoardState,
    sHand: HandState,
    gHand: HandState
  ): PositionData {
    const boardData = Array.from({ length: 9 }, (_, file) =>
      Array.from({ length: 9 }, (_, rank) => {
        const piece = board[file][rank];
        if (!piece) return null;
        return { color: piece.color, kind: piece.kind };
      })
    );

    const senteHandData: HandPieceData[] = [];
    sHand.forEach((count, kind) => {
      if (count > 0) senteHandData.push({ kind, count });
    });

    const goteHandData: HandPieceData[] = [];
    gHand.forEach((count, kind) => {
      if (count > 0) goteHandData.push({ kind, count });
    });

    return {
      board: boardData,
      sente_hand: senteHandData,
      gote_hand: goteHandData,
    };
  }

  async function handleSolve() {
    errorMessage.set("");
    solution.set(null);
    solving.set(true);

    try {
      const posData = buildPositionData($boardState, $senteHand, $goteHand);

      // まずバリデーション
      await invoke("validate_position", { position: posData });

      // 解く
      const result = await invoke<SolutionData>("solve", {
        position: posData,
        maxDepth: $maxDepth,
      });

      solution.set(result);
    } catch (e) {
      errorMessage.set(String(e));
    } finally {
      solving.set(false);
    }
  }

  function handleClear() {
    clearBoard();
  }
</script>

<div class="controls">
  <div class="depth-control">
    <label for="max-depth">最大手数:</label>
    <select id="max-depth" bind:value={$maxDepth}>
      <option value={1}>1手</option>
      <option value={3}>3手</option>
      <option value={5}>5手</option>
      <option value={7}>7手</option>
      <option value={9}>9手</option>
      <option value={11}>11手</option>
    </select>
  </div>

  <div class="buttons">
    <button
      class="solve-btn"
      on:click={handleSolve}
      disabled={$solving}
    >
      {#if $solving}
        求解中...
      {:else}
        解く
      {/if}
    </button>
    <button class="clear-btn" on:click={handleClear} disabled={$solving}>
      クリア
    </button>
  </div>

  {#if $errorMessage}
    <div class="error">{$errorMessage}</div>
  {/if}
</div>

<style>
  .controls {
    display: flex;
    flex-direction: column;
    gap: 12px;
    padding: 12px;
    background: #f8f8f8;
    border-radius: 8px;
    border: 1px solid #ddd;
  }

  .depth-control {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .depth-control label {
    font-size: 14px;
    font-weight: bold;
  }

  .depth-control select {
    padding: 4px 8px;
    border-radius: 4px;
    border: 1px solid #ccc;
    font-size: 14px;
  }

  .buttons {
    display: flex;
    gap: 8px;
  }

  .solve-btn {
    flex: 1;
    padding: 10px 20px;
    background: #4a90d9;
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 16px;
    font-weight: bold;
    cursor: pointer;
  }

  .solve-btn:hover:not(:disabled) {
    background: #357abd;
  }

  .solve-btn:disabled {
    background: #a0c4e8;
    cursor: not-allowed;
  }

  .clear-btn {
    padding: 10px 20px;
    background: #e0e0e0;
    border: none;
    border-radius: 6px;
    font-size: 16px;
    cursor: pointer;
  }

  .clear-btn:hover:not(:disabled) {
    background: #ccc;
  }

  .clear-btn:disabled {
    cursor: not-allowed;
  }

  .error {
    padding: 8px 12px;
    background: #fee;
    color: #c33;
    border: 1px solid #fcc;
    border-radius: 4px;
    font-size: 13px;
  }
</style>
