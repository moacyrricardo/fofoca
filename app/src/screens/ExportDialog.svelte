<script lang="ts">
  // Export a meeting to a folder (spec 023). Follows specs/023-assets/export-mock.html: form →
  // working (progress + Cancel) → done (Open folder).
  import { onMount } from "svelte";
  import { api, onExport } from "../lib/api";
  import { audioWhat, baseName, defaults, everything, options, partBytes, picked, progressRatio, progressText, total, tree, formatBytes, type Selection } from "../lib/export";
  import { minutes } from "../lib/format";
  import type { ExportEvent, ExportPlan, Meeting, TranscriptFormat } from "../lib/types";

  let { meeting, notify }: { meeting: Meeting; notify: (t: string, e?: boolean) => void } = $props();

  let dlg: HTMLDialogElement;
  let view = $state<"form" | "working" | "done">("form");
  let plan = $state<ExportPlan | null>(null);
  let dest = $state("");
  let remembered = $state(false);
  let chosen = $state(false);
  let sel = $state<Selection>({ transcript: true, format: "markdown", notes: true, tasks: true, audio: false, zip: false });
  let progress = $state<ExportEvent | null>(null);
  let result = $state<{ folder: string; zip: string | null } | null>(null);
  let opened = $state(false);

  const files = $derived(plan ? picked(plan.files, sel) : []);
  const lines = $derived(plan ? tree(plan.dest_display, plan.folder_name, files, sel.zip) : []);
  const parts = $derived(plan?.parts);
  const day = (iso: string) => new Date(iso).toLocaleDateString([], { month: "short", day: "numeric" });

  async function replan() {
    plan = await api.exportPlan(meeting.meta.id, everything(dest));
  }

  /** Open the dialog on the form, with the remembered (or default) folder. */
  export async function show() {
    if (dlg.open) return;
    if (view === "working") return dlg.showModal(); // reopen on a running export
    view = "form";
    result = null;
    progress = null;
    chosen = false;
    const c = await api.config();
    dest = c.export_dir;
    remembered = c.export_dir_remembered;
    await replan();
    sel = defaults(plan!.parts);
    dlg.showModal();
  }

  async function choose() {
    try {
      const p = await api.pickExportFolder(dest);
      if (!p) return;
      dest = p;
      chosen = true;
      await replan();
    } catch (e) {
      notify(String(e), true);
    }
  }

  async function start() {
    view = "working";
    progress = null;
    try {
      await api.exportMeeting(meeting.meta.id, options(sel, dest));
    } catch (e) {
      view = "form";
      notify(String(e), true);
    }
  }

  async function openFolder() {
    if (!result) return;
    try {
      await api.openExportFolder(result.folder);
      opened = true;
      setTimeout(() => (opened = false), 1400);
    } catch (e) {
      notify(String(e), true);
    }
  }

  onMount(() => {
    const off = onExport((e) => {
      if (e.meeting_id !== meeting.meta.id) return;
      if (e.state === "running") progress = e;
      else if (e.state === "done") {
        result = { folder: e.folder ?? "", zip: e.zip };
        remembered = true;
        view = "done";
        if (!dlg.open) notify(`Exported to ${baseName(result.folder)}.`);
      } else {
        view = "form";
        if (e.state === "failed") notify(`Export failed: ${e.error ?? "unknown error"}`, true);
        else replan().catch(() => {});
      }
    });
    return () => { off.then((u) => u()); };
  });

  const formats: [TranscriptFormat, string][] = [["markdown", "Markdown"], ["srt", "SRT"], ["both", "Both"]];
</script>

<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
<dialog bind:this={dlg} class="dlg" aria-labelledby="export-title" onclick={(e) => { if (e.target === dlg) dlg.close(); }}>
  {#if view === "form" && plan && parts}
    <h2 id="export-title">Export meeting</h2>
    <div class="sub">{meeting.meta.title} · {day(meeting.meta.started_at)} · {minutes(meeting.meta.duration_s)}</div>

    <div class="sec">Include</div>
    <div class="parts">
      <label class="part" class:off={!parts.transcript_lines}>
        <input type="checkbox" bind:checked={sel.transcript} disabled={!parts.transcript_lines} />
        <span><span class="lbl">Transcript</span> <span class="what">— {parts.transcript_lines} line{parts.transcript_lines === 1 ? "" : "s"}, with speaker and time</span></span>
        {#if parts.transcript_lines}<span class="sz">{formatBytes(partBytes(plan.files, "transcript", sel.format))}</span>{:else}<span class="why">Nothing was transcribed</span>{/if}
      </label>
      <div class="seg fmt" class:dis={!sel.transcript} role="group" aria-label="Transcript format">
        {#each formats as [f, label]}<button class:on={sel.format === f} disabled={!sel.transcript} onclick={() => (sel.format = f)}>{label}</button>{/each}
      </div>
      <label class="part" class:off={!parts.notes}>
        <input type="checkbox" bind:checked={sel.notes} disabled={!parts.notes} />
        <span><span class="lbl">Summary &amp; notes</span> <span class="what">— {parts.notes === "summary" ? "the final summary" : "checked notes with their times"}</span></span>
        {#if parts.notes}<span class="sz">{formatBytes(partBytes(plan.files, "notes", sel.format))}</span>{:else}<span class="why">No summary for this meeting</span>{/if}
      </label>
      <label class="part" class:off={!parts.tasks}>
        <input type="checkbox" bind:checked={sel.tasks} disabled={!parts.tasks} />
        <span><span class="lbl">Tasks</span> <span class="what">— {parts.tasks ? `${parts.tasks} next step${parts.tasks === 1 ? "" : "s"} as a checklist` : "next steps as a checklist"}</span></span>
        {#if parts.tasks}<span class="sz">{formatBytes(partBytes(plan.files, "tasks", sel.format))}</span>{:else}<span class="why">No tasks for this meeting</span>{/if}
      </label>
      <label class="part" class:off={!parts.audio_tracks}>
        <input type="checkbox" bind:checked={sel.audio} disabled={!parts.audio_tracks} />
        <span><span class="lbl">Audio</span> <span class="what">— {parts.audio_tracks ? audioWhat(plan.files, parts.imported) : "the recording, as WAV"}</span></span>
        {#if parts.audio_tracks}<span class="sz">{formatBytes(partBytes(plan.files, "audio", sel.format))}</span>
        {:else}<span class="why">{parts.audio_deleted_at ? `Deleted ${day(parts.audio_deleted_at)}` : "No audio was kept"}</span>{/if}
      </label>
    </div>

    <div class="sec">Save to</div>
    <div class="dest">
      <div class="path" title={plan.dest}>{plan.dest_display}</div>
      <button class="btn ghost sm" onclick={choose}>Choose…</button>
    </div>
    <div class="remember">
      {chosen ? "fo-foca will remember this folder for the next export." : remembered ? "Last used folder — fo-foca remembers it." : "First export — pick any folder; fo-foca will remember it."}
    </div>
    <label class="zip"><input type="checkbox" bind:checked={sel.zip} /> Also make a .zip <span class="faint small">— one file to send</span></label>

    <div class="sec">You'll get</div>
    <div class="tree">
      {#each lines as l}<div><span class="pre">{l.prefix}</span><span class={l.kind === "dir" ? "dir" : l.kind === "zip" ? "z" : "new"}>{l.name}</span>{#if l.size}<span class="meta">{l.size}</span>{/if}</div>{/each}
    </div>

    <div class="foot">
      <span class="total">{total(files, sel.zip)}</span>
      <button class="btn ghost" onclick={() => dlg.close()}>Cancel</button>
      <button class="btn teal" disabled={!files.length} onclick={start}>Export</button>
    </div>
  {:else if view === "working"}
    <h2 id="export-title">Exporting…</h2>
    <div class="sub">{progressText(progress, plan)}</div>
    <div class="prog"><i style="width:{Math.round(progressRatio(progress) * 100)}%"></i></div>
    <div class="faint small">You can keep using fo-foca, and record another meeting meanwhile.</div>
    <div class="foot"><span class="total"></span><button class="btn ghost" onclick={() => api.cancelExport()}>Cancel</button></div>
  {:else if view === "done" && result}
    <div class="done">
      <div class="ok">✓</div>
      <div>
        <h2 id="export-title" class="donettl">Exported</h2>
        <div class="muted small">
          Saved to <b>{plan?.dest_display}/{baseName(result.folder)}/</b>
          {#if result.zip}<br />and <b>{baseName(result.zip)}</b> next to it.{/if}
        </div>
      </div>
    </div>
    <div class="foot">
      <span class="total"></span>
      <button class="btn ghost" onclick={() => dlg.close()}>Close</button>
      <button class="btn teal" onclick={openFolder}>{opened ? "Opened in Files ✓" : "Open folder"}</button>
    </div>
  {/if}
</dialog>

<style>
  .dlg { width: 600px; max-width: calc(100vw - 40px); max-height: calc(100vh - 44px); overflow: auto; margin: 22px auto auto; background: var(--navy-2); color: var(--ink); border: 1px solid var(--line); border-radius: 14px; box-shadow: 0 24px 60px #0009; padding: 20px 22px 18px; }
  .dlg::backdrop { background: #0b1826cc; }
  h2 { font-size: 18px; margin: 0 0 2px; }
  .sub { color: var(--ink-3); font-size: 13px; margin-bottom: 14px; }
  .small { font-size: 13px; }
  .sec { font-size: 11.5px; font-weight: 800; letter-spacing: .6px; color: var(--ink-3); text-transform: uppercase; margin: 14px 0 6px; }
  .parts { display: flex; flex-direction: column; gap: 6px; }
  .part { display: flex; gap: 10px; align-items: center; background: var(--navy); border: 1px solid var(--line); border-radius: 10px; padding: 8px 12px; cursor: pointer; }
  .part input { accent-color: var(--teal); width: 16px; height: 16px; flex: none; }
  .part .lbl { font-weight: 800; }
  .part .what { color: var(--ink-3); font-size: 12.5px; }
  .part .sz { margin-left: auto; color: var(--ink-2); font-size: 12.5px; font-variant-numeric: tabular-nums; white-space: nowrap; }
  .part.off { opacity: .55; cursor: not-allowed; }
  .part .why { color: var(--coral); font-size: 12.5px; margin-left: auto; white-space: nowrap; }
  /* the shared .seg (app.css), as the mock draws it here */
  .fmt { align-self: flex-start; border-width: 1.5px; margin: 6px 0 0 26px; }
  .fmt button + button { border-left: 1.5px solid var(--line); }
  .fmt button { padding: 5px 12px; }
  .fmt button.on { background: var(--teal); color: #08262a; }
  .fmt.dis { opacity: .45; pointer-events: none; }
  .dest { display: flex; gap: 8px; align-items: center; }
  .path { flex: 1; background: var(--navy); border: 1.5px solid var(--line); border-radius: 9px; padding: 7px 10px; font-size: 13px; color: var(--ink); white-space: nowrap; overflow: hidden; text-overflow: ellipsis; font-variant-numeric: tabular-nums; }
  .remember { color: var(--ink-3); font-size: 12px; margin-top: 4px; }
  .zip { display: inline-flex; gap: 8px; align-items: center; margin-top: 10px; cursor: pointer; font-weight: 700; font-size: 13.5px; }
  .zip input { accent-color: var(--teal); width: 16px; height: 16px; }
  .zip .faint { font-weight: 600; }
  .tree { background: #12263a; border: 1px solid var(--line); border-radius: 10px; padding: 10px 12px; font: 12.5px/1.65 ui-monospace, "SF Mono", Menlo, monospace; color: var(--ink-2); max-height: 200px; overflow: auto; }
  .tree .pre { white-space: pre; }
  .tree .dir { color: var(--ink); font-weight: 700; }
  .tree .new { color: var(--teal); }
  .tree .z { color: var(--peach); }
  .tree .meta { color: var(--ink-3); float: right; }
  .foot { display: flex; gap: 10px; align-items: center; margin-top: 16px; }
  .foot .total { color: var(--ink-3); font-size: 12.5px; margin-right: auto; }
  .prog { height: 6px; background: #12263a; border-radius: 4px; overflow: hidden; margin: 14px 0 6px; }
  .prog i { display: block; height: 100%; width: 0; background: var(--teal); transition: width .25s; }
  .done { display: flex; gap: 12px; align-items: flex-start; }
  .done .ok { width: 30px; height: 30px; border-radius: 50%; background: #1f4430; border: 1px solid #3f8a58; color: #c9f3d6; display: flex; align-items: center; justify-content: center; font-weight: 900; flex: none; }
  .donettl { margin: 2px 0 4px; }
</style>
