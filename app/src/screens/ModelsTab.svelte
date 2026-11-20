<script lang="ts">
  // Settings → Models (spec 015): a model per task, from a file you have or a suggested download.
  import { onMount } from "svelte";
  import { api, onDownload } from "../lib/api";
  import type { DownloadEvent, ModelSetting, ModelTask, ModelsView, TaskRow } from "../lib/types";

  let { notify, locked, onchange }: { notify: (t: string, e?: boolean) => void; locked: boolean; onchange: () => void } = $props();

  let view = $state<ModelsView | null>(null);
  /** Entry picked in each row's menu (may not be downloaded yet). */
  let chosen = $state<Partial<Record<ModelTask, string>>>({});
  let progress = $state<Record<string, number>>({});

  const TASKS: Record<ModelTask, { icon: string; title: string; engine: string; settings: ModelSetting[] }> = {
    whisper: { icon: "🗣️", title: "Speech → text", engine: "whisper.cpp", settings: ["whisper"] },
    llm: { icon: "📝", title: "Summaries & notes", engine: "llama.cpp", settings: ["llm"] },
    speakers: { icon: "👥", title: "Speakers", engine: "runs on the CPU", settings: ["segmentation", "embedding"] },
    vad: { icon: "🔇", title: "Voice detection", engine: "skips silence", settings: ["vad"] },
  };
  const ENGINES: Record<string, string> = { whisper_server: "whisper-server", llm_server: "llama-server" };

  function load() {
    return api.modelsView().then((v) => {
      view = v;
      for (const t of v.tasks) if (!chosen[t.task] || chosen[t.task] === t.current) chosen[t.task] = t.current ?? "custom";
    }).catch((e) => notify(String(e), true));
  }

  onMount(() => {
    load();
    let off: (() => void) | undefined;
    onDownload((e: DownloadEvent) => {
      if (e.state === "running") { progress[e.id] = e.total ? e.done / e.total : 0; return; }
      delete progress[e.id];
      if (e.state === "done") { notify("Downloaded and checked — it's used from the next recording."); onchange(); }
      else if (e.state === "failed") notify(`Download failed: ${e.error ?? "unknown error"}`, true);
      load();
    }).then((u) => (off = u));
    return () => off?.();
  });

  const mb = (b: number) => (b >= 1e9 ? `${(b / 1e9).toFixed(2)} GB` : b >= 1e6 ? `${Math.round(b / 1e6)} MB` : `${(b / 1e6).toFixed(1)} MB`);
  const entryOf = (row: TaskRow) => row.entries.find((e) => e.id === chosen[row.task]);

  async function pick(row: TaskRow, value: string) {
    if (value === "custom") {
      chosen[row.task] = row.current ?? "custom";
      await pickFiles(row);
      return;
    }
    chosen[row.task] = value;
    const e = row.entries.find((x) => x.id === value);
    if (e?.present && value !== row.current) {
      try { view = await api.useModel(value); onchange(); } catch (err) { notify(String(err), true); }
    }
  }

  async function pickFiles(row: TaskRow) {
    try {
      for (const setting of TASKS[row.task].settings) {
        const path = await api.pickModelFile(setting);
        if (!path) return;
        view = await api.setModelPath(setting, path);
      }
      chosen[row.task] = "custom";
      onchange();
    } catch (e) { notify(String(e), true); }
  }

  async function pickEngine(setting: ModelSetting) {
    try {
      const path = await api.pickModelFile(setting);
      if (path) { view = await api.setModelPath(setting, path); onchange(); }
    } catch (e) { notify(String(e), true); }
  }

  async function download(id: string) {
    progress[id] = 0;
    try { await api.downloadModel(id); } catch (e) { delete progress[id]; notify(String(e), true); }
  }
</script>

<fieldset class="card" disabled={locked}>
  <h2>Models <span class="faint small">· used by every recording</span></h2>
  {#if !view}
    <p class="faint">Loading…</p>
  {:else}
    {#each view.tasks as row (row.task)}
      {@const t = TASKS[row.task]}
      {@const e = entryOf(row)}
      <div class="mrow">
        <div class="ico">{t.icon}</div>
        <div class="task"><b>{t.title}</b><span>{t.engine}</span></div>
        <div class="pick">
          <select value={chosen[row.task] ?? "custom"} onchange={(ev) => pick(row, (ev.target as HTMLSelectElement).value)}>
            {#each row.entries as x}<option value={x.id}>{x.name} — {x.note}{x.present ? "" : ` · ${mb(x.bytes)} to download`}</option>{/each}
            {#if !row.current}<option value="custom">Your file</option>{/if}
          </select>
          <div class="path" title={row.paths.join("\n")}>{chosen[row.task] === row.current || chosen[row.task] === "custom" ? row.paths.join(" · ") : "not on this computer yet"}</div>
        </div>
        <div class="act">
          {#if e && progress[e.id] !== undefined}
            <span class="bar"><i style="width:{Math.round(progress[e.id] * 100)}%"></i></span>
            <span class="faint small">{Math.round(progress[e.id] * 100)} %</span>
            <button class="link" type="button" onclick={() => api.cancelDownload(e.id)}>Cancel</button>
          {:else if e && !e.present}
            <span class="chip warn">Not downloaded</span>
            <button class="btn teal sm" type="button" onclick={() => download(e.id)}>Download {mb(e.bytes)}</button>
          {:else if row.found}
            <span class="chip ok">✓ on this computer</span>
          {:else}
            <span class="chip warn">File missing</span>
          {/if}
          <button class="link" type="button" onclick={() => pickFiles(row)}>Use another…</button>
        </div>
      </div>
    {/each}

    <div class="sub">ENGINES</div>
    {#each view.engines as g (g.setting)}
      <div class="mrow">
        <div class="ico">⚙️</div>
        <div class="task"><b>{ENGINES[g.setting]}</b><span>local server</span></div>
        <div class="pick"><div class="path" title={g.path}>{g.path}</div></div>
        <div class="act">
          <span class="chip" class:ok={g.found} class:warn={!g.found}>{g.found ? "✓ found" : "Not found"}</span>
          <button class="link" type="button" onclick={() => pickEngine(g.setting)}>Use another…</button>
        </div>
      </div>
    {/each}
    <p class="faint small foot">Models aren't part of the app. Point at files you already have, or download a suggested one — it goes to <code>{view.dir}</code> and is checked before use.</p>
  {/if}
</fieldset>

<style>
  fieldset { min-width: 0; margin: 0; }
  fieldset:disabled { opacity: .6; }
  h2 { margin: 0 0 6px; }
  .small { font-size: 12.5px; }
  .mrow { display: grid; grid-template-columns: 26px 150px minmax(0, 1fr) auto; gap: 12px; align-items: center; padding: 10px 0; border-top: 1px solid #22405c; }
  .mrow:first-of-type { border-top: 0; }
  .ico { font-size: 18px; text-align: center; }
  .task b { display: block; font-size: 13.5px; }
  .task span { font-size: 12px; color: var(--ink-3); }
  .pick { display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .pick select { max-width: 100%; }
  .path { font-size: 12px; color: var(--ink-3); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .act { display: flex; gap: 8px; align-items: center; justify-content: flex-end; white-space: nowrap; }
  .chip.warn { color: #f8e2b0; border-color: #8a7240; background: #3f3622; }
  .bar { width: 110px; height: 6px; background: #12263a; border-radius: 4px; overflow: hidden; }
  .bar i { display: block; height: 100%; background: var(--teal); transition: width .2s; }
  .link { background: none; border: 0; color: var(--teal); cursor: pointer; font-weight: 700; padding: 0; font-size: 13px; }
  .sub { font-size: 12px; font-weight: 800; letter-spacing: .5px; color: var(--ink-3); margin: 14px 0 0; }
  .foot { margin: 10px 0 0; }
  code { font-size: 12px; }
</style>
