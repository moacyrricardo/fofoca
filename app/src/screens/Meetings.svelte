<script lang="ts">
  import type { MeetingMeta } from "../lib/types";
  import { minutes, when } from "../lib/format";
  import mascot from "../assets/mascot.jpg";
  import summaryIcon from "../assets/summary.png";
  import transcriptionIcon from "../assets/transcription.png";

  let { meetings, recording, onopen, onstart, onresume }: {
    meetings: MeetingMeta[];
    recording: boolean;
    onopen: (id: string) => void;
    onstart: (title: string) => void;
    onresume: () => void;
  } = $props();

  let title = $state("");
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
        <form class="start" onsubmit={(e) => { e.preventDefault(); onstart(title); }}>
          <input type="text" placeholder="Meeting title (optional)" bind:value={title} />
          <button class="btn rec" type="submit">● Start recording</button>
        </form>
      {/if}
    </div>
  </div>

  <h2>Recent</h2>
  {#if meetings.length === 0}
    <p class="faint">No meetings yet. Start a recording and it will show up here.</p>
  {:else}
    <div class="list">
      {#each meetings as m (m.id)}
        <button class="item" onclick={() => onopen(m.id)}>
          <img class="ic" src={m.finished ? summaryIcon : transcriptionIcon} alt="" />
          <div class="txt">
            <div class="ttl">{m.title}</div>
            <div class="meta">{when(m.started_at)} · {minutes(m.duration_s)}</div>
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
</style>
