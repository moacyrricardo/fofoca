<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { CalendarSettings } from "../lib/types";

  let { notify }: { notify: (t: string, e?: boolean) => void } = $props();

  let settings = $state<CalendarSettings | null>(null);
  let input = $state("");
  let adding = $state(false);
  let help = $state(false);

  onMount(() => {
    api.calendars().then((s) => (settings = s)).catch((e) => notify(String(e), true));
  });

  async function add(e: Event) {
    e.preventDefault();
    if (!input.trim()) return;
    adding = true;
    try {
      const src = await api.addCalendar(input.trim());
      input = "";
      settings = await api.calendars();
      notify(`Added “${src.label}” — the link is stored in your system keyring.`);
    } catch (err) {
      notify(String(err), true);
    } finally {
      adding = false;
    }
  }

  async function remove(id: string, label: string) {
    try {
      await api.removeCalendar(id);
      settings = await api.calendars();
      notify(`Removed “${label}”.`);
    } catch (err) {
      notify(String(err), true);
    }
  }

  async function setOption(key: "only_with_call_link" | "notify_before" | "auto_record", e: Event) {
    if (!settings) return;
    const next = { ...settings, [key]: (e.target as HTMLInputElement).checked };
    try {
      settings = await api.setCalendarOptions({
        onlyWithCallLink: next.only_with_call_link,
        notifyBefore: next.notify_before,
        autoRecord: next.auto_record,
      });
    } catch (err) {
      notify(String(err), true);
    }
  }
</script>

<div class="card cal">
  <div class="row between">
    <h2>Calendars</h2>
    <button class="link" onclick={() => (help = !help)}>{help ? "Hide" : "How do I get the link?"}</button>
  </div>
  <p class="muted small">fo-foca reads upcoming meetings to name recordings and show what's next. Read-only.</p>

  {#if help}
    <ol class="help">
      <li>Open <b>Google Calendar</b> on the web → ⚙ <b>Settings</b>.</li>
      <li>Under <i>Settings for my calendars</i>, pick the calendar → <b>Integrate calendar</b>.</li>
      <li>Copy <b>Secret address in iCal format</b> and paste it below.</li>
    </ol>
    <p class="faint small">Outlook, iCloud and Fastmail have the same kind of link. You can also paste the path of a local <code>.ics</code> file.
      Anyone with the link can read the calendar — if it leaks, reset it in Google and add the new one here.</p>
  {/if}

  {#if settings}
    {#each settings.sources as s (s.id)}
      <div class="src">
        <span class="dot ok"></span>
        <div class="txt"><b>{s.label}</b><div class="faint small">{s.kind === "link" ? `${s.location} · secret link in keyring` : s.location}</div></div>
        <button class="btn ghost sm" onclick={() => remove(s.id, s.label)}>Remove</button>
      </div>
    {/each}
    <form class="add" onsubmit={add}>
      <input type="text" placeholder="Paste a secret iCal link (https://calendar.google.com/…/basic.ics) or .ics path" bind:value={input} disabled={adding} spellcheck="false" autocomplete="off" />
      <button class="btn teal sm" type="submit" disabled={adding || !input.trim()}>{adding ? "Checking…" : "Add"}</button>
    </form>
    <label class="opt"><input type="checkbox" checked={settings.only_with_call_link} onchange={(e) => setOption("only_with_call_link", e)} /> Only show events with a call link (Meet, Zoom, Teams)</label>
    <label class="opt"><input type="checkbox" checked={settings.notify_before} onchange={(e) => setOption("notify_before", e)} /> Notify me about a minute before a meeting starts</label>
    <label class="opt"><input type="checkbox" checked={settings.auto_record} onchange={(e) => setOption("auto_record", e)} />
      <span>Start recording automatically when a meeting begins <span class="faint">— only while fo-foca is open; you can stop or discard anytime</span></span></label>
  {:else}
    <p class="faint">Loading…</p>
  {/if}
</div>

<style>
  .cal { margin-top: 16px; max-width: 900px; }
  .between { justify-content: space-between; }
  h2 { margin: 0; }
  .small { font-size: 12.5px; }
  .link { background: none; border: 0; color: var(--teal); cursor: pointer; font-weight: 700; font-size: 13px; }
  .help { margin: 8px 0 6px; padding-left: 20px; line-height: 1.6; font-size: 13.5px; }
  .src { display: flex; align-items: center; gap: 10px; padding: 10px 0; border-bottom: 1px solid var(--line); }
  .src .txt { flex: 1; min-width: 0; }
  .add { display: flex; gap: 10px; margin-top: 12px; }
  .add input { flex: 1; }
  .opt { display: flex; gap: 8px; align-items: center; margin-top: 12px; font-size: 13.5px; color: var(--ink-2); cursor: pointer; }
  .opt input { accent-color: var(--teal); width: 16px; height: 16px; }
  code { font-size: 12px; }
</style>
