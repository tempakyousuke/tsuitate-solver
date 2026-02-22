<script lang="ts">
  import Board from "./lib/Board.svelte";
  import PieceSelector from "./lib/PieceSelector.svelte";
  import Controls from "./lib/Controls.svelte";
  import Solution from "./lib/Solution.svelte";
  import { questionTitle, questionAuthor } from "./lib/stores";
</script>

<main>
  <h1>衝立詰将棋ソルバー</h1>
  {#if $questionTitle}
    <div class="question-info">
      <span class="question-title">{$questionTitle}</span>
      <span class="question-author">（{$questionAuthor}）</span>
    </div>
  {/if}
  <div class="app-layout">
    <div class="board-section">
      <Board />
    </div>
    <div class="sidebar">
      <PieceSelector />
      <Controls />
    </div>
  </div>
  <Solution />
</main>

<style>
  main {
    margin: 0 auto;
    padding: 16px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  }

  h1 {
    text-align: center;
    font-size: 24px;
    margin-bottom: 4px;
    color: #333;
  }

  .question-info {
    text-align: center;
    margin-bottom: 12px;
    font-size: 16px;
    color: #555;
  }

  .question-title {
    font-weight: bold;
  }

  .question-author {
    color: #888;
  }

  .app-layout {
    display: flex;
    gap: 20px;
    align-items: flex-start;
  }

  .board-section {
    flex-shrink: 0;
  }

  .sidebar {
    flex: 1;
    display: flex;
    flex-direction: column;
    gap: 12px;
    min-width: 280px;
  }
</style>
