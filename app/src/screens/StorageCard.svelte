<script lang="ts">
  // Settings → Storage: how long meeting audio is kept (spec 011).
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { AudioConfig, StorageView } from "../lib/types";

  let { notify }: { notify: (t: string, e?: boolean) => void } = $props();
  let view = $state<StorageView | null>(null);
  let saving = $state(false);

  onMount(() => {
    api.storage().then((v) => (view = v)).catch((e) => notify(String(e), true));
  });

  async function save(audio: AudioConfig) {
    saving = true;
    try {
      const before = view?.meetings_with_audio ?? 0;
      view = await api.setAudioPolicy(audio);
      const freed = before - view.meetings_with_audio;
      notify(freed > 0 ? `Saved — audio of ${freed} meeting(s) deleted.` : "Saved.");
    } catch (e) {
      notify(String(e), true);
    } finally {
      saving = false;
    }
  }

  const mb = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(1)} GB` : `${Math.round(b / 1e6)} MB`);
</script>

<div class="card storage">
  <h2>Storage</h2>
  <p class="muted small">Recordings are only needed until the transcript, speakers and notes are done. Transcripts, notes and labels are never deleted.</p>
  {#if view}
    {@const v = view}
    <label class="opt"><input type="radio" name="keep" checked={v.audio.keep === "after_processing"} disabled={saving} onchange={() => save({ ...v.audio, keep: "after_processing" })} />
      <span><b>Delete audio after processing</b> <span class="faint">— recommended; kept if a step failed, so it can be retried</span></span></label>
    <label class="opt"><input type="radio" name="keep" checked={v.audio.keep === "days"} disabled={saving} onchange={() => save({ ...v.audio, keep: "days" })} />
      <span><b>Delete audio after</b>
        <input class="days" type="number" min="1" max="3650" value={v.audio.keep_days} disabled={saving || v.audio.keep !== "days"}
          onchange={(e) => save({ ...v.audio, keep_days: Math.max(1, Number((e.target as HTMLInputElement).value) || 30) })} /> days</span></label>
    <label class="opt"><input type="radio" name="keep" checked={v.audio.keep === "always"} disabled={saving} onchange={() => save({ ...v.audio, keep: "always" })} />
      <span><b>Keep all audio</b> <span class="faint">— about 115 MB per hour of meeting</span></span></label>
    <div class="faint small usage">{v.meetings_with_audio} meeting(s) still have audio · {mb(v.audio_bytes)}. Single meetings can be kept or cleared on their page.</div>
  {:else}
    <p class="faint">Loading…</p>
  {/if}
</div>

<style>
  .storage { margin-top: 16px; max-width: 900px; }
  h2 { margin: 0 0 4px; }
  .small { font-size: 13px; }
  .opt { display: flex; gap: 8px; align-items: center; margin-top: 10px; font-size: 13.5px; cursor: pointer; }
  .opt input[type="radio"] { accent-color: var(--teal); width: 16px; height: 16px; }
  .days { width: 64px; padding: 3px 6px; margin: 0 4px; background: var(--navy); color: var(--ink); border: 1px solid var(--line); border-radius: 8px; font: inherit; }
  .usage { margin-top: 12px; }
</style>
