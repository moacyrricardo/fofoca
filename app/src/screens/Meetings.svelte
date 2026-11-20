<script lang="ts">
  import type { MeetingMeta } from "../lib/types";
  import { minutes, when } from "../lib/format";
  import mascot from "../assets/mascot.jpg";
  import summaryIcon from "../assets/summary.png";
  import transcriptionIcon from "../assets/transcription.png";
  import UpNext from "./UpNext.svelte";
  import LabelEditor from "./LabelEditor.svelte";

  let { meetings, recording, onopen, onstart, onresume, onsetup }: {
    meetings: MeetingMeta[];
    recording: boolean;
    onopen: (id: string) => void;
    onstart: (title: string, attendees?: string[], labels?: string[]) => void;
    onresume: () => void;
    onsetup: () => void;
  } = $props();

  let title = $state("");
  let startLabels = $state<string[]>([]);

  // Label filter (spec 007): pick one or more labels → meetings carrying all of them.
  let filter = $state<string[]>([]);
  const counts = $derived.by(() => {
    const c = new Map<string, number>();
    for (const m of meetings) for (const l of m.labels ?? []) c.set(l, (c.get(l) ?? 0) + 1);
    return [...c].sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]));
  });
  // drop selections whose label no longer exists (e.g. removed from the last meeting)
  const active = $derived(filter.filter((l) => counts.some(([x]) => x === l)));
  const shown = $derived(active.length ? meetings.filter((m) => active.every((l) => m.labels?.includes(l))) : meetings);
  const toggle = (l: string) => (filter = active.includes(l) ? active.filter((x) => x !== l) : [...active, l]);
</script>

<section class="screen">
  <h1>Meetings</h1>
  <div class="muted">Everything stays on this computer.</div>

  <div class="hero">
    <img src={mascot} alt="" />
    <div class="t">
      {#if recording}
        <h2>Recording in progress</h2>
        <div class="muted">fo-foca is listening. Go back to see the live transcript.</div>
        <div class="start"><button class="btn rec" onclick={onresume}>● Back to recording</button></div>
      {:else}
        <h2>Ready when you are</h2>
        <div class="muted">fo-foca records your mic and the call audio on separate tracks. No bot joins the call.</div>
        <form class="start" onsubmit={(e) => { e.preventDefault(); onstart(title, [], startLabels); }}>
          <input type="text" placeholder="Meeting title (optional)" bind:value={title} />
          <button class="btn rec" type="submit">● Start recording</button>
        </form>
        <div class="startlabels"><LabelEditor labels={startLabels} onchange={(l) => (startLabels = l)} placeholder="+ label" compact /></div>
        <UpNext onrecord={onstart} {onsetup} disabled={recording} />
      {/if}
    </div>
  </div>

  <h2>Recent</h2>
  {#if counts.length}
    <div class="filter" role="group" aria-label="Filter by label">
      <button class="fchip" class:on={!active.length} onclick={() => (filter = [])}>All <span>{meetings.length}</span></button>
      {#each counts as [l, n] (l)}
        <button class="fchip" class:on={active.includes(l)} aria-pressed={active.includes(l)} onclick={() => toggle(l)}>#{l} <span>{n}</span></button>
      {/each}
    </div>
  {/if}
  {#if meetings.length === 0}
    <p class="faint">No meetings yet. Start a recording and it will show up here.</p>
  {:else if shown.length === 0}
    <p class="faint">No meetings carry all of {active.map((l) => `#${l}`).join(", ")}.</p>
  {:else}
    <div class="list">
      {#each shown as m (m.id)}
        <button class="item" onclick={() => onopen(m.id)}>
          <img class="ic" src={m.finished ? summaryIcon : transcriptionIcon} alt="" />
          <div class="txt">
            <div class="ttl">{m.title}</div>
            <div class="meta">{when(m.started_at)} · {minutes(m.duration_s)}</div>
            {#if m.labels?.length}<div class="rowlabels">{#each m.labels as l (l)}<span class="chip label">{l}</span>{/each}</div>{/if}
          </div>
          <div class="tail">
            {#if m.finished}<span class="chip ok">✓ Saved</span>{:else}<span class="chip">Unfinished</span>{/if}
          </div>
        </button>
      {/each}
    </div>
  {/if}
</section>

<style>
  .screen { padding: 26px 30px 30px; }
  .hero { display: flex; gap: 22px; align-items: center; background: linear-gradient(120deg, #1d3a55, #1a3349); border: 1px solid var(--line); border-radius: 18px; padding: 18px 22px; margin: 18px 0 22px; }
  .hero img { width: 120px; border-radius: 14px; }
  .hero .t { flex: 1; }
  .hero h2 { font-size: 20px; margin: 0 0 4px; }
  .start { display: flex; gap: 10px; margin-top: 14px; }
  .start input { flex: 1; max-width: 380px; }
  .list { display: flex; flex-direction: column; }
  .item { display: flex; align-items: center; gap: 14px; padding: 12px 14px; border-radius: 12px; cursor: pointer; border: 1px solid transparent; background: none; color: inherit; text-align: left; font-size: inherit; }
  .item:hover { background: var(--navy-2); border-color: var(--line); }
  .ic { width: 42px; height: 42px; border-radius: 11px; flex: none; }
  .txt { min-width: 0; }
  .ttl { font-weight: 800; }
  .meta { font-size: 12.5px; color: var(--ink-3); }
  .tail { margin-left: auto; display: flex; gap: 8px; }
  .startlabels { margin-top: 8px; }
  .filter { display: flex; flex-wrap: wrap; gap: 6px; margin: 4px 0 10px; }
  .fchip { background: none; border: 1px solid var(--line); color: var(--ink-2); border-radius: 20px; padding: 4px 11px; font-size: 12.5px; font-weight: 700; cursor: pointer; }
  .fchip span { color: var(--ink-3); font-weight: 600; margin-left: 3px; }
  .fchip:hover { border-color: var(--teal); color: var(--ink); }
  .fchip.on { background: #1d4652; border-color: var(--teal); color: var(--ink); }
  .fchip.on span { color: var(--ink-2); }
  .rowlabels { display: flex; flex-wrap: wrap; gap: 4px; margin-top: 4px; }
  .rowlabels .chip { font-size: 11px; padding: 1px 8px; }
</style>
