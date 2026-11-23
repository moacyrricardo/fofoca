<script lang="ts">
  // Settings → Processing (spec 015): what runs during and after a call, and where.
  import type { Device, Layout, Updates, Workflow, WorkflowView } from "../lib/types";
  import { LAYOUTS, UPDATES, afterSteps, fmt, peakMib, transcriptDevice, unavailable } from "../lib/workflow";
  import { shortModel } from "../lib/format";

  let { view, onchange, ongomodels }: { view: WorkflowView; onchange: (w: Workflow) => void; ongomodels: () => void } = $props();

  const w = $derived(view.workflow);
  const locked = $derived(view.recording);
  const gpuMib = $derived(view.gpu?.total_mib ?? null);
  const wMib = $derived(view.whisper.footprint?.mib ?? null);
  const lMib = $derived(view.llm.footprint?.mib ?? null);
  const both = $derived(w.live_transcript && w.live_summary);
  const peak = $derived(peakMib(w, wMib, lMib));
  const estimated = $derived(!view.whisper.footprint?.measured || !view.llm.footprint?.measured);

  const set = (patch: Partial<Workflow>) => onchange({ ...w, ...patch });
  function setLive(transcript: boolean, summary: boolean) {
    set({ live_transcript: transcript, live_summary: transcript && summary });
  }
  function pickLayout(id: Layout) {
    if (locked || unavailable(id, gpuMib, wMib, lMib)) return;
    set({ layout: id });
  }
  const pct = (mib: number) => (gpuMib ? Math.min(100, (mib / gpuMib) * 100) : 0);
</script>

<fieldset disabled={locked}>
  <div class="uses">Using <b>{shortModel(view.whisper.name)}</b> + <b>{shortModel(view.llm.name)}</b> <button class="link" type="button" onclick={ongomodels}>change in Models</button></div>

  <div class="card">
    <h2>During the call</h2>
    <div class="toggles">
      <label class="tog" class:on={w.live_transcript}><input type="checkbox" checked={w.live_transcript} onchange={(e) => setLive((e.target as HTMLInputElement).checked, w.live_summary)} />
        <span><b>Live transcript</b><span class="faint">Text appears while you talk.</span></span></label>
      <label class="tog" class:on={both} class:off={!w.live_transcript}><input type="checkbox" checked={both} disabled={!w.live_transcript} onchange={(e) => setLive(true, (e.target as HTMLInputElement).checked)} />
        <span><b>Live summary</b><span class="faint">{w.live_transcript ? `A "summary so far" during the call.` : "Needs the live transcript."}</span></span></label>
    </div>
    {#if !w.live_transcript}
      <p class="faint small note">Record only: nothing runs during the call, so the computer stays free. The transcript is made when you stop.</p>
    {/if}

    {#if both}
      <div class="sub">WHERE EACH RUNS</div>
      <div class="layouts">
        {#each LAYOUTS as l (l.id)}
          {@const why = unavailable(l.id, gpuMib, wMib, lMib)}
          <button type="button" class="lay" class:on={w.layout === l.id} class:off={!!why} onclick={() => pickLayout(l.id)} aria-pressed={w.layout === l.id}>
            {#if view.suggested === l.id}<span class="sug">Suggested for this computer</span>{/if}
            <span class="hd"><span class="radio" class:checked={w.layout === l.id}></span><b>{l.name}</b></span>
            <span class="lanes">
              <span>GPU</span><span class="lane">{#each l.lanes.gpu as c}<i class={c}></i>{/each}</span>
              <span>CPU</span><span class="lane">{#each l.lanes.cpu as c}<i class={c}></i>{/each}</span>
            </span>
            <span class="why">{why ?? l.why}</span>
          </button>
        {/each}
      </div>
      <div class="legend"><span style="--c:var(--teal)">Speech → text</span><span style="--c:var(--peach)">Summary</span><span class="faint">each block ≈ one piece of audio</span></div>
    {:else if w.live_transcript}
      <div class="sub">WHERE IT RUNS</div>
      <label class="inline">Live transcript on
        <select value={w.live_device} onchange={(e) => set({ live_device: (e.target as HTMLSelectElement).value as Device })}>
          <option value="gpu" disabled={!view.gpu}>GPU{view.gpu ? ` (${view.gpu.name})` : ""}</option><option value="cpu">CPU</option>
        </select>
        <span class="faint">{w.live_device === "gpu" ? "fastest — nothing else needs the GPU during the call" : "no GPU used; slower"}</span></label>
    {/if}
    {#if w.live_transcript}
      <div class="fit">
        <span class="faint">GPU memory at peak</span>
        {#if !view.gpu}<span class="faint">no usable GPU found</span>
        {:else if peak.mib === null}<span class="faint">unknown — a model file is missing</span>
        {:else}
          <span class="meter"><i class="w" style="width:{pct(peak.whisper)}%"></i><i class="q" style="left:{pct(peak.whisper)}%;width:{pct(peak.llm)}%"></i></span>
          <span>{peak.mib ? `${fmt(peak.mib)} of ${fmt(gpuMib!)} MiB` : "GPU not used during the call"}{#if both && w.layout === "swap"}<span class="faint"> (one model at a time)</span>{/if}{#if estimated && peak.mib}<span class="faint"> · estimate</span>{/if}</span>
        {/if}
      </div>

      <div class="sub">LIVE UPDATES</div>
      <div class="pace">
        {#each UPDATES as u (u.id)}
          <button type="button" class="pc" class:on={w.updates === u.id} onclick={() => set({ updates: u.id as Updates })} aria-pressed={w.updates === u.id}>
            <span class="hd"><span class="radio" class:checked={w.updates === u.id}></span><b>{u.name}</b><span class="est" title={u.measured ? "Measured on the MX230 (spec 010)" : "Not measured yet"}>{u.measured ? "measured" : "estimate"}</span></span>
            <span class="what">{u.what}</span><span class="how">{u.how}</span>
          </button>
        {/each}
      </div>
      <p class="faint small note">{w.updates === "custom" ? "Custom: using chunk_seconds, min_chunk_seconds and summary_every_s from config.toml. Pick one above to go back." : `The "Summary so far" button on the Record screen works in every mode.`}</p>
    {/if}
  </div>

  <div class="card">
    <h2>After the call</h2>
    <div class="steps">
      <label class="step"><input type="checkbox" checked={!w.live_transcript || w.transcript_after} disabled={!w.live_transcript}
        onchange={(e) => set({ transcript_after: e.currentTarget.checked })} />
        <span>Transcript from each track <span class="faint">— transcribes your mic and the call separately, so every line is on the right side</span></span>
        <span class="chip {transcriptDevice(w)}">{transcriptDevice(w).toUpperCase()}</span></label>
      <label class="step"><input type="checkbox" checked={w.speakers_after} onchange={(e) => set({ speakers_after: (e.target as HTMLInputElement).checked })} />
        <span>Separate speakers <span class="faint">— names each voice on the call; 40 s of audio took 1.2 s</span></span><span class="chip cpu">CPU</span></label>
      <label class="step"><input type="checkbox" checked={w.notes_after} onchange={(e) => set({ notes_after: (e.target as HTMLInputElement).checked })} />
        <span>Write checked notes <span class="faint">— summary, decisions and tasks, each linked to the lines it came from</span></span>
        <select value={w.notes_device} disabled={!w.notes_after} onchange={(e) => set({ notes_device: (e.target as HTMLSelectElement).value as Device })}>
          <option value="gpu" disabled={!view.gpu}>GPU</option><option value="cpu">CPU</option>
        </select></label>
    </div>
    <div class="flow"><span class="faint">When you stop:</span>
      {#each afterSteps(w) as s, i}{#if i}<span class="arrow">→</span>{/if}<span class="chip {s.device ?? ''}">{s.name === "Storage" ? "Storage: audio policy" : s.name}</span>{/each}
    </div>
    <p class="faint small note">Imports follow the same steps. They're always transcribed this way.</p>
  </div>
</fieldset>

<style>
  fieldset { border: 0; padding: 0; margin: 0; min-width: 0; display: flex; flex-direction: column; gap: 16px; }
  fieldset:disabled { opacity: .6; }
  .uses { display: flex; gap: 6px; align-items: center; font-size: 13px; color: var(--ink-2); }
  .link { background: none; border: 0; color: var(--teal); cursor: pointer; font-weight: 700; padding: 0; font-size: inherit; }
  .small { font-size: 12.5px; }
  .note { margin: 8px 0 0; }
  .sub { font-size: 12px; font-weight: 800; letter-spacing: .5px; color: var(--ink-3); margin: 16px 0 8px; }
  .toggles { display: flex; gap: 12px; flex-wrap: wrap; }
  .tog { flex: 1; min-width: 240px; display: flex; gap: 10px; align-items: flex-start; border: 1.5px solid var(--line); border-radius: 12px; padding: 10px 12px; cursor: pointer; background: var(--navy); }
  .tog input { accent-color: var(--teal); width: 17px; height: 17px; margin-top: 2px; }
  .tog.on { border-color: #3f8d93; background: #1a3d4d; }
  .tog.off { opacity: .6; cursor: default; }
  .tog b { font-size: 13.5px; }
  .tog .faint { font-size: 12.5px; display: block; margin-top: 2px; }
  .layouts { display: grid; grid-template-columns: 1fr 1fr; gap: 10px; }
  .lay, .pc { font: inherit; color: inherit; text-align: left; border: 1.5px solid var(--line); border-radius: 12px; padding: 11px 12px; cursor: pointer; background: var(--navy); display: flex; flex-direction: column; gap: 7px; position: relative; }
  .lay:hover:not(.off), .pc:hover { border-color: #3f8d93; }
  .lay.on, .pc.on { border-color: var(--teal); background: #1a3d4d; box-shadow: 0 0 0 1px var(--teal) inset; }
  .lay.off { opacity: .45; cursor: not-allowed; }
  .hd { display: flex; gap: 8px; align-items: center; }
  .hd b { font-size: 13.5px; }
  .radio { width: 15px; height: 15px; border-radius: 50%; border: 2px solid var(--ink-3); flex: none; }
  .radio.checked { border-color: var(--teal); background: radial-gradient(var(--teal) 45%, transparent 50%); }
  .why, .what { font-size: 12.5px; color: var(--ink-2); line-height: 1.45; }
  .sug { position: absolute; right: 10px; top: 10px; font-size: 10.5px; font-weight: 800; color: #08262a; background: var(--teal); border-radius: 8px; padding: 1px 7px; }
  .lanes { display: grid; grid-template-columns: 34px 1fr; gap: 3px 6px; align-items: center; font-size: 10.5px; font-weight: 800; color: var(--ink-3); }
  .lane { height: 12px; border-radius: 4px; background: #12263a; display: flex; gap: 2px; overflow: hidden; }
  .lane i { flex: 1; border-radius: 3px; }
  .lane .asr { background: var(--teal); }
  .lane .llm { background: var(--peach); }
  .legend { display: flex; gap: 14px; font-size: 11.5px; color: var(--ink-3); margin-top: 8px; }
  .legend span::before { content: ""; display: inline-block; width: 9px; height: 9px; border-radius: 3px; margin-right: 5px; background: var(--c); vertical-align: -1px; }
  .fit { display: flex; align-items: center; gap: 10px; margin-top: 12px; font-size: 12.5px; }
  .meter { position: relative; flex: 1; max-width: 320px; height: 10px; background: #12263a; border-radius: 6px; overflow: hidden; }
  .meter i { position: absolute; top: 0; bottom: 0; }
  .meter .w { left: 0; background: var(--teal); }
  .meter .q { background: var(--peach); }
  .inline { display: flex; gap: 10px; align-items: center; font-size: 13.5px; }
  .pace { display: grid; grid-template-columns: repeat(3, 1fr); gap: 10px; }
  .pc { gap: 4px; padding: 10px 12px; }
  .how { font-size: 11.5px; color: var(--ink-3); }
  .est { font-size: 10.5px; font-weight: 800; color: var(--ink-3); border: 1px solid var(--line); border-radius: 6px; padding: 0 5px; margin-left: auto; }
  .steps { display: flex; flex-direction: column; gap: 8px; }
  .step { display: grid; grid-template-columns: 22px 1fr auto; gap: 10px; align-items: center; font-size: 13.5px; cursor: pointer; }
  .step input { accent-color: var(--teal); width: 16px; height: 16px; }
  .step .faint { font-size: 12.5px; }
  .flow { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; margin-top: 12px; font-size: 12px; }
  .arrow { color: var(--ink-3); }
</style>
