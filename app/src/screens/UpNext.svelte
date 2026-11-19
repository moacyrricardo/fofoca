<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import type { CalendarEvent } from "../lib/types";

  let { onrecord, onsetup, disabled }: { onrecord: (title: string) => void; onsetup: () => void; disabled: boolean } = $props();

  let events = $state<CalendarEvent[] | null>(null);
  let hasCalendars = $state(true);
  let errors = $state<string[]>([]);
  let now = $state(Date.now());

  async function load(force = false) {
    try {
      const s = await api.calendars();
      hasCalendars = s.sources.length > 0;
      if (!hasCalendars) {
        events = [];
        return;
      }
      const u = await api.upcoming(force);
      events = u.events;
      errors = u.errors;
    } catch {
      events = [];
    }
  }

  onMount(() => {
    load();
    const t = setInterval(() => {
      now = Date.now();
      load();
    }, 60_000);
    return () => clearInterval(t);
  });

  // in progress or starting within the next 18 hours, timed events only
  const soon = $derived(
    (events ?? []).filter((e) => !e.all_day && new Date(e.end).getTime() > now && new Date(e.start).getTime() - now < 18 * 3600_000).slice(0, 3),
  );

  function when(e: CalendarEvent): string {
    const s = new Date(e.start).getTime();
    if (s <= now) return "now";
    const min = Math.round((s - now) / 60_000);
    if (min < 60) return `in ${min} min`;
    return new Date(e.start).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  }
  function service(link: string | null): string {
    if (!link) return "";
    if (link.includes("meet.google")) return "Google Meet";
    if (link.includes("zoom.us")) return "Zoom";
    if (link.includes("teams.microsoft")) return "Teams";
    return "Call";
  }
</script>

{#if !hasCalendars}
  <button class="setup" onclick={onsetup}>📅 Connect a calendar to see upcoming meetings here →</button>
{:else if soon.length}
  <div class="up">
    {#each soon as e, i (e.uid + e.start)}
      <div class="ev" class:first={i === 0}>
        <span class="when" class:now={when(e) === "now"}>{when(e)}</span>
        <div class="txt">
          <b>{e.title}</b>
          <div class="faint small">{[service(e.call_link), e.attendees.slice(0, 3).join(", ") + (e.attendees.length > 3 ? ` +${e.attendees.length - 3}` : "")].filter(Boolean).join(" · ")}</div>
        </div>
        <button class="btn sm" class:rec={i === 0} class:ghost={i > 0} onclick={() => onrecord(e.title)} {disabled}>● Record this</button>
      </div>
    {/each}
  </div>
{:else if events}
  <div class="faint small none">No meetings with a call link in the next hours.{#if errors.length} <span class="err">({errors.join("; ")})</span>{/if}</div>
{/if}

<style>
  .up { display: flex; flex-direction: column; gap: 6px; margin-top: 12px; }
  .ev { display: flex; align-items: center; gap: 12px; background: var(--navy); border: 1px dashed #3a6080; border-radius: 12px; padding: 8px 12px; }
  .ev.first { border-style: solid; border-color: #3f8d93; }
  .when { font-weight: 800; color: var(--teal); min-width: 64px; font-size: 13.5px; }
  .when.now { color: var(--coral); }
  .txt { flex: 1; min-width: 0; }
  .txt b { display: block; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
  .small { font-size: 12.5px; }
  .none { margin-top: 12px; }
  .err { color: var(--warn); }
  .setup { margin-top: 12px; background: none; border: 1px dashed var(--line); color: var(--ink-2); border-radius: 12px; padding: 9px 12px; cursor: pointer; font-weight: 700; font-size: 13px; }
  .setup:hover { border-color: var(--teal); color: var(--ink); }
</style>
