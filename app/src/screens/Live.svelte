<script lang="ts">
  import { tick } from "svelte";
  import { api } from "../lib/api";
  import { live } from "../lib/live.svelte";
  import { markdown, mmss, pausesBefore, shortModel } from "../lib/format";
  import type { ConfigView, ModelState } from "../lib/types";
  import LabelEditor from "./LabelEditor.svelte";

  let { config, onstart, onstop, notify }: {
    config: ConfigView | null;
    onstart: (title: string, attendees?: string[], labels?: string[]) => void;
    onstop: () => void;
    notify: (t: string, e?: boolean) => void;
  } = $props();

  let title = $state("");
  let startLabels = $state<string[]>([]);

  // Labels can be changed while recording; the backend merges them into meta.json.
  async function setLiveLabels(labels: string[]) {
    const m = live.meeting;
    if (!m) return;
    try {
      const meta = await api.setMeetingLabels(m.id, labels);
      if (live.meeting?.id === m.id) live.meeting = { ...live.meeting, labels: meta.labels };
    } catch (e) {
      notify(String(e), true);
    }
  }
  let confirmDiscard = $state(false);

  let txBox: HTMLDivElement | undefined = $state();
  const BARS = 9;

  // follow the transcript as it grows
  $effect(() => {
    live.segments.length;
    tick().then(() => txBox && (txBox.scrollTop = txBox.scrollHeight));
  });

  const s = $derived(live.status);
  const swapping = $derived(live.summarizing || (s?.llm ?? "off") !== "off");

  function bars(level: number): number[] {
    const l = Math.min(1, level * 14);
    return Array.from({ length: BARS }, (_, i) => 3 + Math.max(0, l * 15 * (0.55 + 0.45 * Math.sin(i * 1.7 + (s?.elapsed_s ?? 0) * 9))));
  }

  const label: Record<ModelState, string> = { off: "off", loading: "loading…", ready: "ready", busy: "working" };

  async function summarize() {
    live.summarizing = true;
    try {
      await api.summarize();
    } catch (e) {
      live.summarizing = false;
      notify(String(e), true);
    }
  }

  async function stop(summarize: boolean) {
    live.stopping = true;
    try {
      await api.stop(summarize);
      onstop();
    } catch (e) {
      live.stopping = false;
      notify(String(e), true);
    }
  }

  const paused = $derived(!!s?.paused);
  const chunkText = $derived.by(() => {
    const max = config?.config.chunk_seconds ?? 30;
    const min = config?.config.min_chunk_seconds ?? 0;
    return min > 0 && min < max ? `${min}–${max} s` : `${max} s`;
  });
  let toggling = $state(false);
  async function togglePause() {
    toggling = true;
    try {
      await (paused ? api.resume() : api.pause());
    } catch (e) {
      notify(String(e), true);
    } finally {
      toggling = false;
    }
  }

  async function discard() {
    confirmDiscard = false;
    live.stopping = true;
    try {
      await api.discard();
    } catch (e) {
      live.stopping = false;
      notify(String(e), true);
    }
  }
</script>

<section class="screen">
  {#if !live.meeting}
    <div class="idle">
      <h1>Record a meeting</h1>
      <p class="muted">fo-foca records your microphone and the call audio until you press Stop. Nothing is recorded before you start.</p>
      <form class="startform" onsubmit={(e) => { e.preventDefault(); onstart(title, [], startLabels); }}>
        <input type="text" placeholder="Meeting title (optional)" bind:value={title} />
        <button class="btn rec" type="submit">● Start recording</button>
      </form>
      <div class="startlabels"><LabelEditor labels={startLabels} onchange={(l) => (startLabels = l)} placeholder="+ label" compact /></div>
    </div>
  {:else}
    <div class="livebar">
      <span class="dot big" class:live={!paused} class:idle={paused}></span>
      <span class="timer" class:dim={paused}>{mmss(s?.elapsed_s ?? 0)}</span>
      <div class="ttl"><b>{live.meeting.title}</b><div class="faint" class:pausedtxt={paused}>{paused ? "Paused — nothing is being recorded" : "recording mic + system audio"}</div>
        <div class="livelabels"><LabelEditor labels={live.meeting.labels ?? []} onchange={setLiveLabels} placeholder="+ label" compact /></div></div>
      <div class="meter" class:dim={paused}><span>You</span><span class="bars">{#each bars(s?.mic_level ?? 0) as h}<i style="height:{h}px"></i>{/each}</span></div>
      <div class="meter sys" class:dim={paused}><span>Call</span><span class="bars">{#each bars(s?.call_level ?? 0) as h}<i style="height:{h}px"></i>{/each}</span></div>
      <span class="sp"></span>
      <button class="btn sm" class:ghost={!paused} class:teal={paused} onclick={togglePause} disabled={live.stopping || toggling} title={paused ? "Keep recording" : "Stop keeping audio until you resume"}>{paused ? "▶ Resume" : "❚❚ Pause"}</button>
      <button class="btn ghost sm" onclick={() => (confirmDiscard = true)} disabled={live.stopping} title="Stop and delete this recording">✕ Discard</button>
      <button class="btn ghost sm" onclick={() => stop(false)} disabled={live.stopping} title="Stop and keep the transcript, without a summary">■ Stop, no summary</button>
      <button class="btn rec" onclick={() => stop(true)} disabled={live.stopping}>■ Stop &amp; summarize</button>
    </div>

    <div class="grid">
      <div class="card tx" bind:this={txBox}>
        <div class="txhead">
          <h2>Live transcript</h2>
          <span class="chip gpu">Whisper · {label[s?.asr ?? "off"]}</span>
          <span class="faint small">{config?.config.language ?? ""} · {chunkText} chunks</span>
        </div>
        {#if live.segments.length === 0}
          <p class="faint empty">
            {#if (s?.asr ?? "off") === "loading"}Loading Whisper on the GPU…{:else}Listening — lines appear after each chunk ({chunkText}, cut at pauses in speech).{/if}
          </p>
        {/if}
        {#each live.segments as seg, i (i)}
          {#each pausesBefore(live.pauses, live.segments, i) as p}<div class="pausemark">— paused at {mmss(p)} —</div>{/each}
          <div class="line">
            <span class="ts">{mmss(seg.start)}</span>
            <span class="who {seg.track}">{seg.track === "you" ? "You" : "Call"}</span>
            <span class="text">{seg.text}</span>
          </div>
        {/each}
        {#each pausesBefore(live.pauses, live.segments, live.segments.length) as p}<div class="pausemark">— paused at {mmss(p)} —</div>{/each}
      </div>

      <div class="card side">
        <div class="row between"><h2>Summary so far</h2><span class="faint small">{live.partial ? `up to ${mmss(live.partial.at_s)}` : "not yet"}</span></div>
        {#if swapping}
          <div class="swap">
            <span class="spin"></span>
            <div>
              <b>{(s?.llm ?? "off") === "busy" ? "Writing summary…" : "Swapping models…"}</b>
              <div class="faint small">Whisper paused · audio is buffered</div>
            </div>
          </div>
        {/if}
        <div class="sum md">
          {#if live.partial}{@html markdown(live.partial.text)}{:else}<p class="faint">Press the button for a summary of what was said so far. It takes a few seconds to swap models on the GPU.</p>{/if}
        </div>
        <button class="btn teal sm" onclick={summarize} disabled={swapping || live.stopping}>↻ Update now</button>
        <div class="models">
          <div class="m"><span>Speech → text</span><span class="chip" class:gpu={(s?.asr ?? "off") !== "off"}>{shortModel(config?.whisper_model)} · {label[s?.asr ?? "off"]}</span></div>
          <div class="m"><span>Summaries</span><span class="chip" class:gpu={(s?.llm ?? "off") !== "off"}>{shortModel(config?.llm_model)} · {label[s?.llm ?? "off"]}</span></div>
          <div class="m"><span>Backlog</span><span class="faint">{(s?.backlog_s ?? 0).toFixed(0)} s of audio</span></div>
        </div>
      </div>
    </div>
  {/if}
</section>

{#if confirmDiscard}
  <div class="scrim" role="presentation" onclick={() => (confirmDiscard = false)}>
    <div class="dialog card" role="alertdialog" aria-labelledby="dt" tabindex="-1" onclick={(e) => e.stopPropagation()} onkeydown={(e) => e.key === "Escape" && (confirmDiscard = false)}>
      <h2 id="dt">Discard this recording?</h2>
      <p class="muted">Recording stops and the audio, transcript and summaries of this meeting are deleted. This can't be undone.</p>
      <div class="row end">
        <button class="btn ghost sm" onclick={() => (confirmDiscard = false)}>Keep recording</button>
        <button class="btn sm danger" onclick={discard}>Discard</button>
      </div>
    </div>
  </div>
{/if}

<style>
  .idle { max-width: 620px; margin: 60px auto; text-align: center; }
  .startform { display: flex; gap: 10px; justify-content: center; margin-top: 18px; }
  .startform input { flex: 1; max-width: 360px; }
  .scrim { position: fixed; inset: 0; background: #0009; display: grid; place-items: center; z-index: 20; }
  .dialog { max-width: 420px; }
  .end { justify-content: flex-end; gap: 10px; }
  .danger { background: var(--bad); color: #2a0606; }
  .screen { padding: 22px 30px 24px; height: 100%; display: flex; flex-direction: column; }
  .livebar { display: flex; align-items: center; gap: 14px; margin-bottom: 16px; }
  .big { width: 12px; height: 12px; }
  .timer { font-variant-numeric: tabular-nums; font-size: 26px; font-weight: 800; }
  .ttl .faint { font-size: 12.5px; }
  .meter { display: flex; align-items: center; gap: 8px; font-size: 12px; color: var(--ink-2); }
  .bars { display: flex; gap: 2px; align-items: flex-end; height: 18px; }
  .bars i { width: 3px; border-radius: 2px; background: var(--teal); transition: height .12s; }
  .meter.sys .bars i { background: var(--coral); }
  .sp { flex: 1; }
  .grid { display: grid; grid-template-columns: 1fr 330px; gap: 16px; flex: 1; min-height: 0; }
  .tx { overflow: auto; padding-top: 0; }
  .txhead { position: sticky; top: 0; background: var(--navy-2); padding: 12px 0 8px; display: flex; align-items: center; gap: 10px; z-index: 1; }
  .txhead h2 { margin: 0; }
  .txhead .small { margin-left: auto; }
  .small { font-size: 12.5px; }
  .empty { margin-top: 18px; }
  .line { display: grid; grid-template-columns: 54px 56px 1fr; gap: 8px; padding: 7px 0; border-bottom: 1px solid #22405c; line-height: 1.5; }
  .ts { color: var(--ink-3); font-variant-numeric: tabular-nums; font-size: 12.5px; padding-top: 2px; }
  .line .who { padding-top: 2px; }
  .side { display: flex; flex-direction: column; gap: 10px; overflow: auto; }
  .between { justify-content: space-between; }
  .side h2 { margin: 0; }
  .sum { flex: 1; }
  .swap { display: flex; align-items: center; gap: 10px; font-size: 13px; background: #0f2234; border: 1px solid var(--line); border-radius: 10px; padding: 10px; }
  .models { display: flex; flex-direction: column; gap: 6px; font-size: 12.5px; background: var(--navy); border: 1px solid var(--line); border-radius: 12px; padding: 10px; }
  .m { display: flex; justify-content: space-between; align-items: center; gap: 8px; }
  .m .chip { max-width: 190px; overflow: hidden; text-overflow: ellipsis; }
  .startlabels { margin-top: 8px; display: flex; justify-content: center; }
  .livelabels { margin-top: 4px; }
  .dim { opacity: .45; }
  .pausedtxt { color: var(--peach); font-weight: 700; }
  .pausemark { text-align: center; color: var(--ink-3); font-size: 12px; font-weight: 700; padding: 8px 0; border-bottom: 1px solid #22405c; }
</style>
