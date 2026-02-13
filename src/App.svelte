<script lang="ts">
  import Board from "./lib/Board.svelte";
  import PieceSelector from "./lib/PieceSelector.svelte";
  import Controls from "./lib/Controls.svelte";
  import Solution from "./lib/Solution.svelte";
</script>

<main>
  <h1>衝立詰将棋ソルバー</h1>
  <div class="app-layout">
    <div class="board-section">
      <Board />
    </div>
    <div class="sidebar">
      <PieceSelector />
      <Controls />
      <Solution />
    </div>
  </div>
</main>

<style>
  main {
    max-width: 900px;
    margin: 0 auto;
    padding: 16px;
    font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
  }

  h1 {
    text-align: center;
    font-size: 24px;
    margin-bottom: 16px;
    color: #333;
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
