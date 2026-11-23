<script lang="ts">
  import { live } from "../lib/live.svelte";
  import mascot from "../assets/mascot.jpg";

  import type { Workflow } from "../lib/types";
  import { transcriptDevice } from "../lib/workflow";

  let { onopen, workflow }: { workflow?: Workflow; onopen: (id: string) => void } = $props();

  const steps = $derived([
    { id: "transcribing", title: "Finish transcription", detail: "last chunk · Whisper on the GPU", chip: "GPU" },
    ...(live.steps.includes("tracks") || (workflow && (!workflow.live_transcript || workflow.transcript_after))
      ? [{ id: "tracks", title: "Transcribe each track", detail: "mic and call separately", chip: workflow ? transcriptDevice(workflow).toUpperCase() : "Whisper" }] : []),
    { id: "speakers", title: "Separate speakers", detail: "who said what on the call · runs on the processor", chip: "CPU" },
    { id: "summarizing", title: "Write checked notes", detail: "summary, decisions and tasks — each checked against what was said", chip: "GPU" },
    { id: "saving", title: "Save notes", detail: "transcript, summary and both audio tracks stay on this computer", chip: "disk" },
  ]);

  function state(id: string): "done" | "run" | "todo" {
    const i = live.steps.lastIndexOf(id);
    if (i === -1) return "todo";
    return i === live.steps.length - 1 ? "run" : "done";
  }
  const meetingId = $derived(live.meeting?.id);
</script>

<section class="proc">
  <img src={mascot} alt="" />
  <h1>Wrapping up your meeting</h1>
  <div class="muted">You can keep working — the notes open when they're ready.</div>
  <div class="steps">
    {#each steps as s}
      {@const st = state(s.id)}
      <div class="step {st}">
        <span class="st">{st === "done" ? "✓" : ""}</span>
        <div><b>{s.title}</b><div class="d">{s.detail}</div></div>
        <span class="chip" class:gpu={s.chip === "GPU"}>{s.chip}</span>
      </div>
    {/each}
  </div>
  {#if meetingId}
    <button class="btn ghost sm" onclick={() => onopen(meetingId)}>Open transcript now →</button>
  {/if}
</section>

<style>
  .proc { max-width: 640px; margin: 30px auto; text-align: center; padding: 0 20px; }
  .proc img { width: 150px; border-radius: 16px; }
  h1 { margin-top: 12px; }
  .steps { text-align: left; margin: 22px 0 18px; display: flex; flex-direction: column; gap: 10px; }
  .step { display: flex; align-items: center; gap: 12px; background: var(--navy-2); border: 1px solid var(--line); border-radius: 12px; padding: 12px 14px; }
  .step .chip { margin-left: auto; }
  .st { width: 22px; height: 22px; border-radius: 50%; border: 2px solid var(--line); flex: none; display: grid; place-items: center; font-size: 12px; font-weight: 900; }
  .step.done .st { background: var(--ok); border-color: var(--ok); color: #0d2a17; }
  .step.run .st { border-top-color: var(--teal); animation: sp .8s linear infinite; }
  .step.todo { opacity: .6; }
  .d { font-size: 12.5px; color: var(--ink-3); }
</style>
