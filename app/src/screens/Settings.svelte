<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { ConfigView, Workflow, WorkflowView } from "../lib/types";
  import { layoutInfo, setupTitle } from "../lib/workflow";
  import settingsIcon from "../assets/settings.png";
  import CalendarsCard from "./CalendarsCard.svelte";
  import StorageCard from "./StorageCard.svelte";
  import ProcessingTab from "./ProcessingTab.svelte";
  import ModelsTab from "./ModelsTab.svelte";

  let { config, notify, onchange }: { config: ConfigView | null; notify: (t: string, e?: boolean) => void; onchange?: () => void } = $props();

  // Spec 015: tabs, with the current setup above them on every tab.
  type Tab = "processing" | "models" | "calendars" | "storage";
  let tab = $state<Tab>("processing");
  let view = $state<WorkflowView | null>(null);
  let checking = $state(false);

  onMount(() => {
    api.workflowView().then((v) => (view = v)).catch((e) => notify(String(e), true));
  });

  async function setWorkflow(w: Workflow) {
    try {
      view = await api.setWorkflow(w);
      onchange?.();
    } catch (e) {
      notify(String(e), true);
    }
  }

  async function check() {
    checking = true;
    try {
      view = await api.probeHardware();
      const g = view.gpu;
      notify(g ? `${g.name}: ${g.total_mib.toLocaleString("en-US")} MiB of GPU memory.` : "No usable GPU found — everything can run on the CPU.");
    } catch (e) {
      notify(String(e), true);
    } finally {
      checking = false;
    }
  }

  function modelsChanged() {
    api.workflowView().then((v) => (view = v)).catch(() => {});
    onchange?.();
  }

  const w = $derived(view?.workflow);
  const setupText = $derived(!w ? "" : !w.live_transcript ? "Nothing runs during the call. When you stop: transcript, then the steps below."
    : !w.live_summary ? `Text while you talk; no summary until the call ends.` : layoutInfo(w.layout).why);
  const TABS: { id: Tab; label: string }[] = [
    { id: "processing", label: "⚡ Processing" }, { id: "models", label: "🧠 Models" }, { id: "calendars", label: "📅 Calendars" }, { id: "storage", label: "🗄️ Storage" },
  ];
</script>

<section class="screen">
  <h1>Settings</h1>
  <div class="muted">fo-foca runs everything on this computer.</div>

  <div class="recom">
    <img src={settingsIcon} alt="" />
    <div class="grow">
      <div class="faint label">CURRENT SETUP</div>
      <b class="big">{w ? setupTitle(w) : "…"}</b>
      <div class="muted small">{setupText}</div>
    </div>
    <button class="btn ghost sm" onclick={check} disabled={checking}>{checking ? "Checking…" : "↻ Check this computer again"}</button>
  </div>

  <div class="tabs">
    {#each TABS as t}<button class="tab" class:on={tab === t.id} onclick={() => (tab = t.id)}>{t.label}</button>{/each}
  </div>
  {#if view?.recording && (tab === "processing" || tab === "models")}
    <div class="lock"><span class="dot live"></span>A recording is running — these settings can be changed when it ends.</div>
  {/if}

  {#if tab === "processing"}
    {#if view}<ProcessingTab {view} onchange={setWorkflow} ongomodels={() => (tab = "models")} />{:else}<p class="faint">Loading…</p>{/if}
  {:else if tab === "models"}
    <ModelsTab {notify} locked={!!view?.recording} onchange={modelsChanged} />
  {:else if tab === "calendars"}
    <div class="pane"><CalendarsCard {notify} /></div>
  {:else if config}
    <div class="pane">
      <StorageCard {notify} />
      <div class="card files">
        <div><span class="faint">Config file</span> <code>{config.path}</code></div>
        <div><span class="faint">Meetings folder</span> <code>{config.meetings_dir}</code></div>
      </div>
    </div>
  {/if}
</section>

<style>
  /* one width for every card, so their edges line up */
  .screen { padding: 26px 30px 30px; max-width: 960px; }
  .recom { display: flex; gap: 14px; align-items: center; background: linear-gradient(120deg, #1e4650, #1b3a4f); border: 1px solid #3f8d93; border-radius: 16px; padding: 14px 16px; margin: 18px 0 12px; }
  .recom img { width: 64px; height: 64px; border-radius: 14px; }
  .grow { flex: 1; min-width: 0; }
  .label { font-size: 12px; font-weight: 800; letter-spacing: .5px; }
  .big { font-size: 17px; }
  .small { font-size: 13px; }
  .tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--line); margin: 4px 0 16px; }
  .tab { background: none; border: 0; color: var(--ink-2); font-weight: 800; font-size: 14px; padding: 9px 14px; border-bottom: 3px solid transparent; cursor: pointer; }
  .tab.on { color: var(--ink); border-bottom-color: var(--coral); }
  .lock { display: flex; gap: 8px; align-items: center; background: #3f3622; border: 1px solid #8a7240; color: #f8e2b0; border-radius: 10px; padding: 8px 12px; font-size: 13px; margin-bottom: 12px; }
  .files { margin-top: 16px; display: flex; flex-direction: column; gap: 6px; font-size: 13px; }
  code { font-size: 12.5px; color: var(--ink-2); }
  /* the reused cards bring a top margin; the tab bar already spaces them */
  .pane > :global(.card:first-child) { margin-top: 0; }
</style>
