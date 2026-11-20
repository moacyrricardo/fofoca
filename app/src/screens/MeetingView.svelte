<script lang="ts">
  import { api } from "../lib/api";
  import { markdown, minutes, mmss, when } from "../lib/format";
  import type { Meeting } from "../lib/types";
  import summaryIcon from "../assets/summary.png";
  import transcriptionIcon from "../assets/transcription.png";
  import LabelEditor from "./LabelEditor.svelte";

  let { id, onback, notify }: { id: string; onback: () => void; notify: (t: string, e?: boolean) => void } = $props();

  let meeting = $state<Meeting | null>(null);
  let tab = $state<"summary" | "transcript">("summary");

  $effect(() => {
    const want = id;
    meeting = null;
    api.meeting(want).then((m) => {
      if (want === id) {
        meeting = m;
        tab = m.summaries.some((s) => s.kind === "final") ? "summary" : "transcript";
      }
    }).catch((e) => notify(String(e), true));
  });

  async function saveLabels(labels: string[]) {
    if (!meeting) return;
    const m = meeting;
    const before = m.meta.labels ?? [];
    m.meta.labels = labels; // optimistic
    try {
      m.meta = await api.setMeetingLabels(m.meta.id, labels);
    } catch (e) {
      m.meta.labels = before;
      notify(String(e), true);
    }
  }

  const final = $derived(meeting?.summaries.filter((s) => s.kind === "final").at(-1));
  const partials = $derived(meeting?.summaries.filter((s) => s.kind === "partial") ?? []);
</script>

<section class="screen">
  <button class="back" onclick={onback}>← Meetings</button>
  {#if !meeting}
    <p class="faint">Loading…</p>
  {:else}
    <h1>{meeting.meta.title}</h1>
    <div class="muted small">
      {when(meeting.meta.started_at)} · {minutes(meeting.meta.duration_s)} · {meeting.segments.length} lines
      {#if !meeting.meta.finished}<span class="chip">unfinished</span>{/if}
    </div>
    {#if meeting.meta.attendees?.length}
      <div class="faint small">Invited: {meeting.meta.attendees.join(", ")}</div>
    {/if}
    <div class="mlabels"><LabelEditor labels={meeting.meta.labels ?? []} onchange={saveLabels} /></div>

    <div class="tabs">
      <button class="tab" class:on={tab === "summary"} onclick={() => (tab = "summary")}><img src={summaryIcon} alt="" />Summary</button>
      <button class="tab" class:on={tab === "transcript"} onclick={() => (tab = "transcript")}><img src={transcriptionIcon} alt="" />Transcript</button>
    </div>

    {#if tab === "summary"}
      {#if final}
        <div class="md body">{@html markdown(final.text)}</div>
        <div class="card note">Written by {final.model} on this computer ({final.gen_tokens_per_s.toFixed(0)} tok/s). Small models can get details wrong — check against the transcript.</div>
      {:else}
        <p class="faint">No final summary for this meeting{meeting.segments.length ? "" : " — nothing was transcribed"}.</p>
      {/if}
      {#if partials.length}
        <details>
          <summary>{partials.length} summar{partials.length === 1 ? "y" : "ies"} written during the call</summary>
          {#each partials as p}
            <div class="partial"><div class="faint small">up to {mmss(p.at_s)}</div><div class="md">{@html markdown(p.text)}</div></div>
          {/each}
        </details>
      {/if}
    {:else}
      <div class="card tx">
        {#each meeting.segments as seg, i (i)}
          <div class="line">
            <span class="ts">{mmss(seg.start)}</span>
            <span class="who {seg.track}">{seg.track === "you" ? "You" : "Call"}</span>
            <span>{seg.text}</span>
          </div>
        {:else}
          <p class="faint">Nothing was transcribed.</p>
        {/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .screen { padding: 18px 30px 30px; }
  .back { background: none; border: 0; color: var(--ink-3); cursor: pointer; padding: 0; margin-bottom: 10px; font-weight: 700; }
  .back:hover { color: var(--ink); }
  .small { font-size: 13px; }
  .mlabels { margin-top: 10px; }
  .tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--line); margin: 18px 0 16px; }
  .tab { background: none; border: 0; color: var(--ink-2); font-weight: 800; font-size: 14px; padding: 9px 14px; border-bottom: 3px solid transparent; cursor: pointer; display: flex; gap: 7px; align-items: center; }
  .tab.on { color: var(--ink); border-bottom-color: var(--coral); }
  .tab img { width: 22px; height: 22px; border-radius: 6px; }
  .body { max-width: 760px; }
  .note { margin-top: 14px; font-size: 12.5px; color: var(--ink-2); max-width: 760px; }
  details { margin-top: 16px; max-width: 760px; }
  summary { cursor: pointer; color: var(--ink-2); font-weight: 700; }
  .partial { border-left: 3px solid var(--line); padding-left: 12px; margin: 10px 0; }
  .tx { padding: 4px 16px; }
  .line { display: grid; grid-template-columns: 54px 56px 1fr; gap: 8px; padding: 7px 0; border-bottom: 1px solid #22405c; line-height: 1.5; }
  .line:last-child { border-bottom: 0; }
  .ts { color: var(--ink-3); font-variant-numeric: tabular-nums; font-size: 12.5px; padding-top: 2px; }
  .line .who { padding-top: 2px; }
</style>
