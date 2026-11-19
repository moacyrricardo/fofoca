<script lang="ts">
  // Label chips with remove buttons + an input that adds on Enter / comma, with autocomplete from
  // labels already in use (spec 007). The parent owns the list and saves it in `onchange`.
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { normalizeLabel, normalizeLabels } from "../lib/format";

  let { labels, onchange, placeholder = "Add label", compact = false }: {
    labels: string[];
    onchange: (labels: string[]) => void;
    placeholder?: string;
    compact?: boolean;
  } = $props();

  let text = $state("");
  let known = $state<string[]>([]);
  const listId = `labels-${Math.random().toString(36).slice(2, 9)}`;

  onMount(() => {
    api.knownLabels().then((k) => (known = k.map((x) => x.label))).catch(() => {});
  });

  const suggestions = $derived(known.filter((l) => !labels.includes(l)));

  function commit() {
    const parts = text.split(",").map(normalizeLabel).filter((l): l is string => !!l);
    text = "";
    if (!parts.length) return;
    const next = normalizeLabels([...labels, ...parts]);
    if (next.length !== labels.length) onchange(next);
  }

  function key(e: KeyboardEvent) {
    if (e.key === "Enter" || e.key === ",") {
      e.preventDefault();
      commit();
    } else if (e.key === "Backspace" && !text && labels.length) {
      onchange(labels.slice(0, -1));
    }
  }
</script>

<div class="labels" class:compact>
  {#each labels as l (l)}
    <span class="chip label">{l}<button type="button" class="x" aria-label={`Remove ${l}`} onclick={() => onchange(labels.filter((x) => x !== l))}>×</button></span>
  {/each}
  <input type="text" list={listId} bind:value={text} onkeydown={key} onblur={commit} {placeholder} spellcheck="false" autocomplete="off" maxlength="40" />
  <datalist id={listId}>
    {#each suggestions as s (s)}<option value={s}></option>{/each}
  </datalist>
</div>

<style>
  .labels { display: flex; flex-wrap: wrap; gap: 6px; align-items: center; }
  .x { background: none; border: 0; color: inherit; opacity: .6; cursor: pointer; padding: 0 0 0 4px; font-size: 14px; line-height: 1; }
  .x:hover { opacity: 1; }
  input[type="text"] { width: 130px; padding: 4px 9px; font-size: 12.5px; border-radius: 20px; }
  .compact input[type="text"] { width: 110px; }
</style>
