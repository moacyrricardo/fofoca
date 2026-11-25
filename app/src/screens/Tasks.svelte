<script lang="ts">
  // Tasks (spec 016): every meeting's next steps, filtered by the meetings' labels.
  import { onMount } from "svelte";
  import { api } from "../lib/api";
  import { when } from "../lib/format";
  import { byDue, byMeeting, filterTasks, isOverdue, isoDay, labelCounts } from "../lib/tasks";
  import type { TaskItem } from "../lib/types";
  import tasksIcon from "../assets/tasks.png";

  let { onopen, notify }: { onopen: (meetingId: string) => void; notify: (t: string, e?: boolean) => void } = $props();

  let tasks = $state<TaskItem[] | null>(null);
  let filter = $state<string[]>([]);
  let view = $state<"meeting" | "due">("meeting");
  const today = isoDay(new Date());

  onMount(() => {
    api.tasks().then((t) => (tasks = t)).catch((e) => notify(String(e), true));
  });

  const counts = $derived(labelCounts(tasks ?? []));
  const active = $derived(filter.filter((l) => counts.some(([x]) => x === l)));
  const shown = $derived(filterTasks(tasks ?? [], active));
  const toggle = (l: string) => (filter = active.includes(l) ? active.filter((x) => x !== l) : [...active, l]);
  const overdue = $derived(shown.filter((t) => isOverdue(t, today)).length);
  const dueText = (d: string) => new Date(`${d}T12:00:00`).toLocaleDateString([], { weekday: "short", day: "numeric", month: "short" });
</script>

{#snippet task(t: TaskItem, showMeeting: boolean)}
  <button class="task" onclick={() => onopen(t.meeting_id)} title="Open the meeting's Tasks tab">
    <span class="ttl">{t.text}</span>
    <span class="meta">
      {#if t.owner}<span class="owner">{t.owner}</span>{:else}<span class="faint">No owner named</span>{/if}
      {#if t.due}<span class="due" class:late={isOverdue(t, today)}>{isOverdue(t, today) ? "overdue · " : "due "}{dueText(t.due)}</span>{/if}
      {#if showMeeting}<span class="faint">· {t.meeting_title}</span>{/if}
    </span>
  </button>
{/snippet}

<section class="screen">
  <div class="head">
    <img src={tasksIcon} alt="" />
    <div>
      <h1>Tasks</h1>
      <div class="muted">Next steps from the checked notes of every meeting.{#if overdue}{" "}<span class="late">{overdue} overdue.</span>{/if}</div>
    </div>
  </div>

  {#if !tasks}
    <p class="faint">Loading…</p>
  {:else if !tasks.length}
    <div class="card empty">No tasks yet. They come from a meeting's checked notes — written when a recording stops, or with "Write checked notes" on a meeting's Summary tab.</div>
  {:else}
    <div class="bar">
      {#if counts.length}
        <div class="filter" role="group" aria-label="Filter by label">
          <button class="fchip" class:on={!active.length} onclick={() => (filter = [])}>All <span>{tasks.length}</span></button>
          {#each counts as [l, n] (l)}
            <button class="fchip" class:on={active.includes(l)} aria-pressed={active.includes(l)} onclick={() => toggle(l)}>#{l} <span>{n}</span></button>
          {/each}
        </div>
      {/if}
      <div class="seg" role="group" aria-label="Order">
        <button class:on={view === "meeting"} onclick={() => (view = "meeting")}>By meeting</button>
        <button class:on={view === "due"} onclick={() => (view = "due")}>By due date</button>
      </div>
    </div>

    {#if !shown.length}
      <p class="faint">No tasks in meetings labelled {active.map((l) => `#${l}`).join(" + ")}.</p>
    {:else if view === "meeting"}
      {#each byMeeting(shown) as g (g.meeting_id)}
        <div class="group">
          <button class="ghead" onclick={() => onopen(g.meeting_id)}>
            <b>{g.meeting_title}</b><span class="faint">{when(g.started_at)}</span>
            {#each g.labels as l (l)}<span class="chip label">{l}</span>{/each}
          </button>
          {#each g.tasks as t}{@render task(t, false)}{/each}
        </div>
      {/each}
    {:else}
      <div class="group">{#each byDue(shown) as t}{@render task(t, true)}{/each}</div>
    {/if}
    <p class="faint small foot">Tasks are filtered by the labels of the meeting they came from. Open a meeting to see the lines each task was taken from.</p>
  {/if}
</section>

<style>
  .screen { padding: 26px 30px 30px; max-width: 960px; }
  .head { display: flex; gap: 14px; align-items: center; margin-bottom: 16px; }
  .head img { width: 56px; height: 56px; border-radius: 14px; }
  .small { font-size: 12.5px; }
  .late { color: var(--coral); font-weight: 700; }
  .empty { color: var(--ink-2); font-size: 13.5px; }
  .bar { display: flex; gap: 12px; align-items: flex-start; justify-content: space-between; margin-bottom: 12px; }
  .filter { display: flex; flex-wrap: wrap; gap: 6px; }
  .fchip { background: none; border: 1px solid var(--line); color: var(--ink-2); border-radius: 20px; padding: 4px 11px; font-size: 12.5px; font-weight: 700; cursor: pointer; }
  .fchip span { color: var(--ink-3); font-weight: 600; margin-left: 3px; }
  .fchip:hover { border-color: var(--teal); color: var(--ink); }
  .fchip.on { background: #1d4652; border-color: var(--teal); color: var(--ink); }
  .fchip.on span { color: var(--ink-2); }
  .group { background: var(--navy-2); border: 1px solid var(--line); border-radius: 14px; padding: 6px 14px; margin-bottom: 12px; }
  .ghead { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; width: 100%; background: none; border: 0; border-bottom: 1px solid #22405c; color: var(--ink); padding: 8px 0; cursor: pointer; text-align: left; font-size: 14px; }
  .ghead:hover b { color: var(--teal); }
  .ghead .faint { font-size: 12.5px; }
  .ghead .chip { font-size: 11px; padding: 1px 8px; }
  .task { display: flex; flex-direction: column; gap: 3px; width: 100%; background: none; border: 0; border-bottom: 1px solid #22405c; color: var(--ink); text-align: left; padding: 10px 0; cursor: pointer; }
  .task:last-child { border-bottom: 0; }
  .task:hover .ttl { color: var(--teal); }
  .ttl { font-weight: 800; font-size: 14px; }
  .meta { display: flex; gap: 8px; flex-wrap: wrap; font-size: 12.5px; }
  .owner { color: var(--teal); font-weight: 700; }
  .due { color: var(--ink-2); }
  .due.late { color: var(--coral); font-weight: 700; }
  .foot { margin-top: 4px; }
</style>
