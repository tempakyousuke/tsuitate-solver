<script lang="ts">
  import {
    solution, previewMode, selectedPath, exitPreview,
    selectMove, selectObservation, isAttackMove, isCaptured,
  } from "./stores";
  import type { SolutionNode, Observation } from "./types";

  const RANK_KANJI = ["", "一", "二", "三", "四", "五", "六", "七", "八", "九"];

  let collapsed = $state(new Set<string>());

  function toggle(key: string) {
    const next = new Set(collapsed);
    if (next.has(key)) {
      next.delete(key);
    } else {
      next.add(key);
    }
    collapsed = next;
  }

  function collapseAll(node: SolutionNode, prefix: string, keys: Set<string>) {
    if (isAttackMove(node)) {
      if (node.AttackMove.branches.length > 0) {
        keys.add(prefix);
      }
      for (let i = 0; i < node.AttackMove.branches.length; i++) {
        const b = node.AttackMove.branches[i];
        if (b.observation !== "Checkmate") {
          collapseAll(b.continuation, `${prefix}/${i}`, keys);
        }
      }
    }
  }

  function foldAll(tree: SolutionNode | undefined, secondTree: SolutionNode | undefined) {
    const keys = new Set<string>();
    if (tree) collapseAll(tree, "t", keys);
    if (secondTree) collapseAll(secondTree, "s", keys);
    collapsed = keys;
  }

  function unfoldAll() {
    collapsed = new Set();
  }

  function countBranches(node: SolutionNode): number {
    if (!isAttackMove(node)) return 0;
    return node.AttackMove.branches.length;
  }

  function squareLabel(file: number, rank: number): string {
    return `${file}${RANK_KANJI[rank]}`;
  }

  function observationLabel(obs: Observation): string {
    if (obs === "NoCapture") return "取られない";
    if (obs === "Checkmate") return "詰み";
    if (obs === "Illegal") return "反則";
    if (isCaptured(obs)) {
      return `${squareLabel(obs.Captured.file, obs.Captured.rank)}の駒を取られた`;
    }
    return "不明";
  }

  function isCheckmate(node: SolutionNode): node is { Checkmate: { depth: number } } {
    return "Checkmate" in node;
  }

  function handleMoveClick(pathStr: string, event: MouseEvent) {
    event.stopPropagation();
    selectMove(pathStr);
  }

  function handleObservationClick(parentPath: string, branchIdx: number, event: MouseEvent) {
    event.stopPropagation();
    selectObservation(parentPath, branchIdx);
  }
</script>

{#if $solution}
  <div class="solution">
    <h3>結果</h3>
    <p class="message" class:found={$solution.found} class:not-found={!$solution.found}>
      {$solution.message}
    </p>

    {#snippet nodeDisplay(node: SolutionNode, depth: number, path: string)}
      {#if isCheckmate(node)}
        <div class="line" style="padding-left: {depth * 20 + 24}px">
          <span class="checkmate">詰み（{node.Checkmate.depth}手）</span>
        </div>
      {:else if isAttackMove(node)}
        {@const hasBranches = node.AttackMove.branches.length > 0}
        {@const isFolded = collapsed.has(path)}
        <div
          class="line foldable"
          style="padding-left: {depth * 20}px"
          onclick={() => hasBranches && toggle(path)}
        >
          <span class="gutter">
            {#if hasBranches}
              <span class="chevron" class:folded={isFolded}></span>
            {:else}
              <span class="chevron-placeholder"></span>
            {/if}
          </span>
          <span
            class="move clickable"
            class:selected={$selectedPath === path}
            onclick={(e) => handleMoveClick(path, e)}
          >{node.AttackMove.mv.notation}</span>
          {#if isFolded}
            <span class="fold-badge">{countBranches(node)} 分岐</span>
          {/if}
        </div>
        {#if !isFolded}
          {#each node.AttackMove.branches as branch, i}
            <div class="line branch-line" style="padding-left: {depth * 20 + 24}px">
              <span
                class="observation clickable"
                class:selected={$selectedPath === `${path}/${i}!`}
                onclick={(e) => handleObservationClick(path, i, e)}
              >[{observationLabel(branch.observation)}]</span>
              {#if branch.observation === "Checkmate" && isCheckmate(branch.continuation)}
                <span class="checkmate-inline"> → 詰み（{branch.continuation.Checkmate.depth}手）</span>
              {/if}
            </div>
            {#if branch.observation !== "Checkmate"}
              {@render nodeDisplay(branch.continuation, depth + 1, `${path}/${i}`)}
            {/if}
          {/each}
        {/if}
      {/if}
    {/snippet}

    {#if $solution.tree}
      <div class="tree">
        <div class="tree-header">
          <h4>解の手順</h4>
          <div class="fold-controls">
            {#if $previewMode}
              <button class="reset-btn" onclick={() => exitPreview()}>盤面を戻す</button>
            {/if}
            <button onclick={() => foldAll($solution?.tree ?? undefined, $solution?.second_tree ?? undefined)}>全て折りたたむ</button>
            <button onclick={() => unfoldAll()}>全て展開</button>
          </div>
        </div>
        {@render nodeDisplay($solution.tree, 0, "t")}
      </div>
    {/if}

    {#if $solution.second_tree}
      <div class="tree second-solution">
        <h4>2つ目の解（余詰め）</h4>
        {@render nodeDisplay($solution.second_tree, 0, "s")}
      </div>
    {/if}

    {#if $solution.trace && $solution.trace.length > 0}
      <details class="trace-section">
        <summary>探索ログ ({$solution.trace.length}行)</summary>
        <pre class="trace-log">{$solution.trace.join("\n")}</pre>
      </details>
    {/if}
  </div>
{/if}

<style>
  .solution {
    padding: 12px;
    background: #f8f8f8;
    border-radius: 8px;
    border: 1px solid #ddd;
  }

  h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  h4 {
    margin: 0;
    font-size: 14px;
  }

  .tree-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    margin: 8px 0 4px;
  }

  .fold-controls {
    display: flex;
    gap: 4px;
  }

  .fold-controls button {
    padding: 2px 8px;
    font-size: 11px;
    border: 1px solid #ccc;
    border-radius: 3px;
    background: #fff;
    color: #555;
    cursor: pointer;
  }

  .fold-controls button:hover {
    background: #e8e8e8;
  }

  .message {
    padding: 8px;
    border-radius: 4px;
    font-size: 14px;
  }

  .message.found {
    background: #e8f5e9;
    color: #2e7d32;
  }

  .message.not-found {
    background: #fff3e0;
    color: #e65100;
  }

  .tree {
    margin-top: 8px;
    font-family: monospace;
    font-size: 13px;
    overflow-x: auto;
  }

  .line {
    display: flex;
    align-items: center;
    height: 22px;
    white-space: nowrap;
  }

  .line.foldable {
    cursor: pointer;
    border-radius: 3px;
  }

  .line.foldable:hover {
    background: rgba(0, 0, 0, 0.04);
  }

  .gutter {
    display: inline-flex;
    align-items: center;
    justify-content: center;
    width: 20px;
    flex-shrink: 0;
  }

  .chevron {
    display: inline-block;
    width: 0;
    height: 0;
    border-left: 5px solid #888;
    border-top: 4px solid transparent;
    border-bottom: 4px solid transparent;
    transition: transform 0.1s ease;
    transform: rotate(90deg);
  }

  .chevron.folded {
    transform: rotate(0deg);
  }

  .chevron-placeholder {
    display: inline-block;
    width: 5px;
  }

  .move {
    font-weight: bold;
    font-size: 14px;
  }

  .move.clickable {
    cursor: pointer;
    padding: 0 4px;
    border-radius: 3px;
  }

  .move.clickable:hover {
    background: rgba(74, 144, 217, 0.15);
  }

  .move.selected {
    background: rgba(74, 144, 217, 0.25);
  }

  .reset-btn {
    padding: 2px 8px;
    font-size: 11px;
    border: 1px solid #c33;
    border-radius: 3px;
    background: #fff;
    color: #c33;
    cursor: pointer;
  }

  .reset-btn:hover {
    background: #fee;
  }

  .fold-badge {
    margin-left: 8px;
    padding: 0 6px;
    font-size: 11px;
    color: #888;
    background: #e4e4e4;
    border-radius: 3px;
    line-height: 18px;
  }

  .observation {
    color: #666;
    font-size: 12px;
  }

  .observation.clickable {
    cursor: pointer;
    padding: 0 4px;
    border-radius: 3px;
  }

  .observation.clickable:hover {
    background: rgba(100, 100, 100, 0.12);
  }

  .observation.selected {
    background: rgba(100, 100, 100, 0.2);
  }

  .checkmate {
    color: #c33;
    font-weight: bold;
  }

  .checkmate-inline {
    color: #c33;
    font-weight: bold;
    font-size: 13px;
  }

  .branch-line {
    margin: 0;
  }

  .second-solution {
    margin-top: 12px;
    padding-top: 8px;
    border-top: 1px solid #ddd;
  }

  .second-solution h4 {
    color: #c33;
  }

  .trace-section {
    margin-top: 12px;
    border-top: 1px solid #ddd;
    padding-top: 8px;
  }

  .trace-section summary {
    cursor: pointer;
    font-size: 13px;
    color: #666;
    user-select: none;
  }

  .trace-log {
    margin-top: 4px;
    padding: 8px;
    background: #1e1e1e;
    color: #d4d4d4;
    border-radius: 4px;
    font-size: 11px;
    line-height: 1.4;
    max-height: 400px;
    overflow: auto;
    white-space: pre;
  }
</style>
