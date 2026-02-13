<script lang="ts">
  import { solution } from "./stores";
  import type { SolutionNode, SolutionBranch, Observation } from "./types";

  function observationLabel(obs: Observation): string {
    switch (obs) {
      case "NoCapture":
        return "取られない";
      case "Captured":
        return "駒を取られた";
      case "Checkmate":
        return "詰み";
      case "Illegal":
        return "反則";
    }
  }

  function isCheckmate(node: SolutionNode): node is { Checkmate: { depth: number } } {
    return "Checkmate" in node;
  }

  function isAttackMove(node: SolutionNode): node is {
    AttackMove: { mv: { notation: string }; branches: SolutionBranch[] };
  } {
    return "AttackMove" in node;
  }
</script>

{#if $solution}
  <div class="solution">
    <h3>結果</h3>
    <p class="message" class:found={$solution.found} class:not-found={!$solution.found}>
      {$solution.message}
    </p>

    {#if $solution.tree}
      <div class="tree">
        <h4>解の手順</h4>
        {#snippet nodeDisplay(node: SolutionNode, indent: number)}
          {#if isCheckmate(node)}
            <div class="node checkmate" style="margin-left: {indent * 20}px">
              詰み
            </div>
          {:else if isAttackMove(node)}
            <div class="node attack" style="margin-left: {indent * 20}px">
              <span class="move">{node.AttackMove.mv.notation}</span>
              {#each node.AttackMove.branches as branch}
                <div class="branch" style="margin-left: 20px">
                  <span class="observation">[{observationLabel(branch.observation)}]</span>
                  {#if branch.observation === "Checkmate"}
                    <span class="checkmate-inline"> → 詰み</span>
                  {:else}
                    {@render nodeDisplay(branch.continuation, indent + 1)}
                  {/if}
                </div>
              {/each}
            </div>
          {/if}
        {/snippet}
        {@render nodeDisplay($solution.tree, 0)}
      </div>
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
    margin: 8px 0 4px;
    font-size: 14px;
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
  }

  .node {
    margin: 4px 0;
  }

  .move {
    font-weight: bold;
    font-size: 14px;
  }

  .observation {
    color: #666;
    font-size: 12px;
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

  .branch {
    margin: 2px 0;
  }
</style>
