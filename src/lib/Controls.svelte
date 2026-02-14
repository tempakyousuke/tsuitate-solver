<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import {
    boardState,
    senteHand,
    goteHand,
    solution,
    solving,
    errorMessage,
    maxDepth,
    findSecondSolution,
    clearBoard,
  } from "./stores";
  import type { BoardState, HandState } from "./stores";
  import type { PositionData, HandPieceData, SolutionData, PieceKind, Color } from "./types";
  import { ALL_PIECE_KINDS, HAND_PIECE_KINDS } from "./types";

  /** JSON インポート/エクスポート用のテキスト */
  let jsonText = "";
  let jsonVisible = false;

  /** サンプル問題 */
  let sampleQuestions: number[] = [];
  let selectedQuestion: number | null = null;

  onMount(async () => {
    try {
      sampleQuestions = await invoke<number[]>("list_sample_questions");
      if (sampleQuestions.length > 0) {
        selectedQuestion = sampleQuestions[0];
      }
    } catch (_) {
      // サンプル問題が見つからなくても無視
    }
  });

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

  /** 盤面を JSON にエクスポート */
  function handleExport() {
    // 盤面上の駒だけを簡潔に表現
    const pieces: { file: number; rank: number; color: Color; kind: PieceKind }[] = [];
    for (let f = 0; f < 9; f++) {
      for (let r = 0; r < 9; r++) {
        const p = $boardState[f][r];
        if (p) {
          pieces.push({ file: f + 1, rank: r + 1, color: p.color, kind: p.kind });
        }
      }
    }

    const senteHandArr: { kind: PieceKind; count: number }[] = [];
    $senteHand.forEach((count, kind) => {
      if (count > 0) senteHandArr.push({ kind, count });
    });

    const data = {
      board: pieces,
      sente_hand: senteHandArr,
    };

    jsonText = JSON.stringify(data, null, 2);
    jsonVisible = true;
  }

  /** JSON から盤面をインポート */
  function handleImport() {
    try {
      const data = JSON.parse(jsonText);

      // バリデーション
      if (!data.board || !Array.isArray(data.board)) {
        throw new Error("board フィールドが必要です");
      }

      // 盤面をクリアしてから復元
      clearBoard();

      boardState.update((board) => {
        for (const p of data.board) {
          const f = (p.file as number) - 1;
          const r = (p.rank as number) - 1;
          if (f < 0 || f >= 9 || r < 0 || r >= 9) continue;
          const color = p.color as Color;
          const kind = p.kind as PieceKind;
          if (!["sente", "gote"].includes(color)) continue;
          if (!ALL_PIECE_KINDS.includes(kind)) continue;
          board[f][r] = { color, kind };
        }
        return board;
      });

      if (data.sente_hand && Array.isArray(data.sente_hand)) {
        senteHand.update((h) => {
          for (const hp of data.sente_hand) {
            const kind = hp.kind as PieceKind;
            const count = hp.count as number;
            if (!HAND_PIECE_KINDS.includes(kind)) continue;
            if (count > 0) h.set(kind, count);
          }
          return h;
        });
      }

      errorMessage.set("");
      jsonVisible = false;
    } catch (e) {
      errorMessage.set(`JSON読み込みエラー: ${e}`);
    }
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
        findSecondSolution: $findSecondSolution,
      });

      solution.set(result);
    } catch (e) {
      errorMessage.set(String(e));
    } finally {
      solving.set(false);
    }
  }

  async function handleCancel() {
    await invoke("cancel_solve");
  }

  function handleClear() {
    clearBoard();
  }

  /** サンプル問題を読み込む */
  async function loadSampleQuestion() {
    if (selectedQuestion == null) return;
    try {
      const json = await invoke<string>("load_sample_question", { number: selectedQuestion });
      jsonText = json;
      handleImport();
    } catch (e) {
      errorMessage.set(`サンプル問題の読み込みエラー: ${e}`);
    }
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
      <option value={13}>13手</option>
      <option value={15}>15手</option>
      <option value={17}>17手</option>
      <option value={19}>19手</option>
      <option value={21}>21手</option>
      <option value={23}>23手</option>
      <option value={25}>25手</option>
      <option value={27}>27手</option>
    </select>
  </div>

  <div class="option-control">
    <label>
      <input type="checkbox" bind:checked={$findSecondSolution} disabled={$solving} />
      2つ目の解を探す（余詰めチェック）
    </label>
  </div>

  <div class="buttons">
    {#if $solving}
      <button class="cancel-btn" on:click={handleCancel}>
        中止
      </button>
    {:else}
      <button class="solve-btn" on:click={handleSolve}>
        解く
      </button>
    {/if}
    <button class="clear-btn" on:click={handleClear} disabled={$solving}>
      クリア
    </button>
  </div>

  {#if sampleQuestions.length > 0}
    <div class="sample-question">
      <label for="sample-select">サンプル問題:</label>
      <select id="sample-select" bind:value={selectedQuestion} disabled={$solving}>
        {#each sampleQuestions as num}
          <option value={num}>第{num}問</option>
        {/each}
      </select>
      <button class="load-btn" on:click={loadSampleQuestion} disabled={$solving}>
        読み込み
      </button>
    </div>
  {/if}

  <div class="io-buttons">
    <button class="io-btn" on:click={handleExport} disabled={$solving}>
      JSONエクスポート
    </button>
    <button class="io-btn" on:click={() => { jsonVisible = !jsonVisible; }} disabled={$solving}>
      JSONインポート
    </button>
  </div>

  {#if jsonVisible}
    <div class="json-area">
      <textarea
        bind:value={jsonText}
        rows="8"
        placeholder="JSONを貼り付けてください"
      ></textarea>
      <button class="import-btn" on:click={handleImport}>読み込み</button>
    </div>
  {/if}

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

  .option-control label {
    display: flex;
    align-items: center;
    gap: 6px;
    font-size: 14px;
    cursor: pointer;
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

  .cancel-btn {
    flex: 1;
    padding: 10px 20px;
    background: #d9534f;
    color: white;
    border: none;
    border-radius: 6px;
    font-size: 16px;
    font-weight: bold;
    cursor: pointer;
  }

  .cancel-btn:hover {
    background: #c9302c;
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

  .sample-question {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .sample-question label {
    font-size: 14px;
    font-weight: bold;
    white-space: nowrap;
  }

  .sample-question select {
    flex: 1;
    padding: 4px 8px;
    border-radius: 4px;
    border: 1px solid #ccc;
    font-size: 14px;
  }

  .load-btn {
    padding: 4px 12px;
    background: #5cb85c;
    color: white;
    border: none;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
    white-space: nowrap;
  }

  .load-btn:hover:not(:disabled) {
    background: #4cae4c;
  }

  .load-btn:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .io-buttons {
    display: flex;
    gap: 8px;
  }

  .io-btn {
    flex: 1;
    padding: 6px 12px;
    background: #fff;
    border: 1px solid #ccc;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
  }

  .io-btn:hover:not(:disabled) {
    background: #f0f0f0;
  }

  .io-btn:disabled {
    cursor: not-allowed;
    opacity: 0.5;
  }

  .json-area {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .json-area textarea {
    width: 100%;
    font-family: monospace;
    font-size: 12px;
    padding: 8px;
    border: 1px solid #ccc;
    border-radius: 4px;
    resize: vertical;
    box-sizing: border-box;
  }

  .import-btn {
    align-self: flex-end;
    padding: 6px 16px;
    background: #5cb85c;
    color: white;
    border: none;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
  }

  .import-btn:hover {
    background: #4cae4c;
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
