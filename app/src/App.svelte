<script lang="ts">
  import { onMount } from "svelte";
  import { api, onSession, preview } from "./lib/api";
  import { live, resetLive } from "./lib/live.svelte";
  import type { CalendarEvent, ConfigView, MeetingMeta, SessionEvent } from "./lib/types";
  import { shortModel } from "./lib/format";
  import Meetings from "./screens/Meetings.svelte";
  import Live from "./screens/Live.svelte";
  import Processing from "./screens/Processing.svelte";
  import MeetingView from "./screens/MeetingView.svelte";
  import Settings from "./screens/Settings.svelte";
  import launchIcon from "./assets/launch.png";

  type Screen = "meetings" | "live" | "processing" | "meeting" | "settings";
  let screen = $state<Screen>("meetings");
  let openId = $state<string | null>(null);
  let meetings = $state<MeetingMeta[]>([]);
  let config = $state<ConfigView | null>(null);
  let toast = $state<{ text: string; error: boolean } | null>(null);
  let toastTimer: number | undefined;

  function notify(text: string, error = false) {
    toast = { text, error };
    clearTimeout(toastTimer);
    toastTimer = window.setTimeout(() => (toast = null), error ? 8000 : 3500);
  }

  async function refresh() {
    try {
      meetings = await api.meetings();
    } catch (e) {
      notify(String(e), true);
    }
  }

  function open(id: string) {
    openId = id;
    screen = "meeting";
  }

  async function start(title: string, attendees: string[] = []) {
    try {
      const m = await api.start(title, attendees);
      if (!live.meeting) resetLive(m);
      screen = "live";
    } catch (e) {
      notify(String(e), true);
    }
  }

  function handle(e: SessionEvent) {
    switch (e.type) {
      case "started":
        resetLive(e.meeting);
        screen = "live";
        break;
      case "status": {
        const { type: _, ...s } = e;
        live.status = s;
        break;
      }
      case "segment":
        live.segments.push(e.segment);
        break;
      case "summary":
        if (e.summary.kind === "partial") live.partial = e.summary;
        else live.final = e.summary;
        live.summarizing = false;
        break;
      case "step":
        live.steps.push(e.step);
        if (screen === "live") screen = "processing";
        break;
      case "notice":
        live.summarizing = false;
        notify(e.message);
        break;
      case "error":
        live.summarizing = false;
        notify(e.message, true);
        break;
      case "discarded":
        resetLive(null);
        screen = "meetings";
        refresh();
        notify("Recording discarded — nothing was kept.");
        break;
      case "finished":
        resetLive(null);
        refresh().then(() => open(e.meeting.id));
        break;
    }
  }

  // ---- calendar: heads-up banner + desktop notification, optional auto-record (spec 004)
  let banner = $state<CalendarEvent | null>(null);
  const notified = new Set<string>();
  const autoStarted = new Set<string>();

  async function calendarTick() {
    try {
      const s = await api.calendars();
      if (!s.sources.length || (!s.notify_before && !s.auto_record)) return;
      const { events } = await api.upcoming(false);
      const now = Date.now();
      for (const e of events) {
        if (e.all_day) continue;
        const startMs = new Date(e.start).getTime();
        const key = e.uid + e.start;
        const untilStart = startMs - now;
        if (s.notify_before && untilStart <= 70_000 && untilStart > -60_000 && !notified.has(key)) {
          notified.add(key);
          if (!live.meeting) {
            banner = e;
            api.notify(`${e.title} starts ${untilStart > 0 ? "in a minute" : "now"}`, s.auto_record ? "fo-foca will start recording." : "Open fo-foca to record it.").catch(() => {});
          }
        }
        if (s.auto_record && untilStart <= 0 && untilStart > -120_000 && !autoStarted.has(key) && !live.meeting) {
          autoStarted.add(key);
          banner = null;
          await start(e.title, e.attendees);
          api.notify(`Recording “${e.title}”`, "Stop or discard it anytime in fo-foca.").catch(() => {});
        }
      }
    } catch {
      // calendar problems are shown in Settings / Up next, not as popups every 20 s
    }
  }

  onMount(() => {
    calendarTick();
    const calTimer = setInterval(calendarTick, 20_000);
    let unlisten: (() => void) | undefined;
    onSession(handle).then((u) => (unlisten = u));
    refresh();
    api.config().then((c) => (config = c)).catch((e) => notify(String(e), true));
    api.activeSession().then((m) => {
      if (m) {
        resetLive(m);
        screen = "live";
      }
    });
    // Browser preview only: #live, #processing, #meeting, #settings jump straight to a screen.
    if (preview && location.hash) {
      const h = location.hash.slice(1);
      setTimeout(async () => {
        if (h === "live" || h === "processing") {
          await start("Sync semanal — produto");
          if (h === "processing") setTimeout(() => api.stop(true).then(() => (screen = "processing")), 3000);
        } else if (h === "meeting") open("2026-11-19-140200");
        else if (h === "settings") screen = "settings";
      }, 50);
    }
    return () => {
      unlisten?.();
      clearInterval(calTimer);
    };
  });

  const nav: { id: Screen; label: string; path: string }[] = [
    { id: "meetings", label: "Meetings", path: "M4 6h16M4 12h16M4 18h10" },
    { id: "live", label: "Record", path: "M9 3h6v11a3 3 0 0 1-6 0zM5 11a7 7 0 0 0 14 0M12 18v3" },
    { id: "settings", label: "Settings", path: "M12 15a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM19 12l2-1-1-3-2 .2-1.3-1.3.2-2-3-1-1 2h-1.8l-1-2-3 1 .2 2L6.9 6.2 5 6l-1 3 2 1v2l-2 1 1 3 2-.2 1.3 1.3-.2 2 3 1 1-2h1.8l1 2 3-1-.2-2 1.3-1.3 2 .2 1-3z" },
  ];

  function go(id: Screen) {
    screen = id;
    if (id === "meetings") refresh();
  }

  const recording = $derived(!!live.meeting);
</script>

<div class="app">
  <aside class="side">
    <div class="brand">
      <img src={launchIcon} alt="" />
      <div><b>fo-foca</b><small>local ai transcribe &amp; task</small></div>
    </div>
    {#each nav as n}
      <button class="nav" class:on={screen === n.id || (n.id === "live" && screen === "processing") || (n.id === "meetings" && screen === "meeting")} onclick={() => go(n.id)}>
        <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round"><path d={n.path} /></svg>
        {n.label}
        {#if n.id === "live" && recording}<span class="dot live" style="margin-left:auto"></span>{/if}
      </button>
    {/each}
    <div class="grow"></div>
    <button class="hwchip" onclick={() => go("settings")}>
      {#if recording}
        <div><span class="dot live"></span><b>Recording</b></div>
      {:else}
        <div><span class="dot ok"></span><b>Ready</b></div>
      {/if}
      <div class="faint">{shortModel(config?.whisper_model)} · {shortModel(config?.llm_model)}</div>
      <div class="faint">swap layout</div>
    </button>
  </aside>

  <main class="main">
    {#if banner && !recording}
      <div class="banner">
        <span>📅 <b>{banner.title}</b> {new Date(banner.start).getTime() > Date.now() ? "starts in a minute" : "is starting"}</span>
        <span class="sp"></span>
        <button class="btn rec sm" onclick={() => { const b = banner!; banner = null; start(b.title, b.attendees); }}>● Record</button>
        <button class="btn ghost sm" onclick={() => (banner = null)}>Dismiss</button>
      </div>
    {/if}
    {#if screen === "meetings"}
      <Meetings {meetings} {recording} onopen={open} onstart={start} onresume={() => (screen = "live")} onsetup={() => (screen = "settings")} />
    {:else if screen === "live"}
      <Live {config} onstart={start} onstop={() => (screen = "processing")} {notify} />
    {:else if screen === "processing"}
      <Processing onopen={(id) => open(id)} />
    {:else if screen === "meeting" && openId}
      <MeetingView id={openId} onback={() => go("meetings")} {notify} />
    {:else if screen === "settings"}
      <Settings {config} {notify} />
    {/if}
  </main>
</div>

{#if toast}
  <div class="toast" class:error={toast.error} role="status">{toast.text}</div>
{/if}

<style>
  .app { display: flex; height: 100%; }
  .side { width: 212px; background: var(--side); border-right: 1px solid var(--line); display: flex; flex-direction: column; padding: 16px 12px; gap: 4px; flex: none; }
  .brand { display: flex; align-items: center; gap: 10px; padding: 2px 6px 16px; }
  .brand img { width: 38px; height: 38px; border-radius: 10px; }
  .brand b { font-size: 20px; }
  .brand small { display: block; font-size: 11px; color: var(--ink-3); font-weight: 600; margin-top: -2px; }
  .nav { display: flex; align-items: center; gap: 10px; padding: 9px 10px; border-radius: 10px; color: var(--ink-2); cursor: pointer; font-weight: 700; font-size: 14px; border: 0; background: none; text-align: left; }
  .nav:hover { background: var(--navy-2); color: var(--ink); }
  .nav.on { background: var(--navy-3); color: var(--ink); }
  .nav svg { width: 18px; height: 18px; flex: none; }
  .grow { flex: 1; }
  .hwchip { border: 1px solid var(--line); border-radius: 12px; padding: 10px; font-size: 12px; color: var(--ink-2); background: var(--navy-2); cursor: pointer; text-align: left; overflow: hidden; }
  .hwchip b { color: var(--ink); font-size: 12.5px; }
  .hwchip div { white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .main { flex: 1; min-width: 0; overflow: auto; position: relative; }
  .banner { position: sticky; top: 0; z-index: 5; display: flex; align-items: center; gap: 10px; background: #1e4650; border-bottom: 1px solid #3f8d93; padding: 10px 30px; }
  .banner .sp { flex: 1; }
</style>
