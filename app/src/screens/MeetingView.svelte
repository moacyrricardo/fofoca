<script lang="ts">
  import { api, preview } from "../lib/api";
  import { tick } from "svelte";
  import { markdown, minutes, mmss, pausesBefore, speakerColor, speakerName, when } from "../lib/format";
  import type { Meeting } from "../lib/types";
  import summaryIcon from "../assets/summary.png";
  import transcriptionIcon from "../assets/transcription.png";
  import LabelEditor from "./LabelEditor.svelte";

  let { id, onback, notify }: { id: string; onback: () => void; notify: (t: string, e?: boolean) => void } = $props();

  let meeting = $state<Meeting | null>(null);
  let tab = $state<"summary" | "tasks" | "transcript" | "speakers">("summary");
  let separating = $state(false);
  let writing = $state(false);
  /** Transcript line highlighted after following a time chip. */
  let focusAt = $state<number | null>(null);

  async function jump(t: number) {
    tab = "transcript";
    focusAt = t;
    await tick();
    document.getElementById(`line-${t}`)?.scrollIntoView({ block: "center", behavior: "smooth" });
  }

  async function writeNotes() {
    if (!meeting) return;
    writing = true;
    try {
      meeting = await api.writeNotes(meeting.meta.id);
    } catch (e) {
      notify(String(e), true);
    } finally {
      writing = false;
    }
  }

  $effect(() => {
    const want = id;
    meeting = null;
    api.meeting(want).then((m) => {
      if (want === id) {
        meeting = m;
        tab = m.notes || m.summaries.some((s) => s.kind === "final") ? "summary" : "transcript";
        if (preview && ["#speakers", "#transcript", "#tasks"].includes(location.hash)) tab = location.hash.slice(1) as typeof tab; // browser preview only
      }
    }).catch((e) => notify(String(e), true));
  });

  async function saveLabels(labels: string[]) {
    if (!meeting) return;
    const m = meeting;
    const before = m.meta.labels ?? [];
    m.meta.labels = labels; // optimistic
    try {
      m.meta = await api.setMeetingLabels(m.meta.id, labels);
    } catch (e) {
      m.meta.labels = before;
      notify(String(e), true);
    }
  }

  // Speakers tab (spec 003): each voice with talk time and a couple of example lines.
  const voices = $derived.by(() => {
    if (!meeting) return [];
    const m = meeting;
    return Object.keys(m.speakers ?? {}).map((key) => {
      const lines = m.segments.filter((s) => s.speaker === key);
      const talk = lines.reduce((t, s) => t + (s.end - s.start), 0);
      const samples = [...lines].sort((a, b) => b.text.length - a.text.length).slice(0, 2);
      return { key, name: m.speakers?.[key]?.name ?? "", talk, samples, first: lines[0] };
    });
  });
  const attendeeList = $derived(`attendees-${id}`);

  async function rename(key: string, name: string) {
    if (!meeting || (meeting.speakers?.[key]?.name ?? "") === name.trim()) return;
    try {
      meeting = await api.renameSpeaker(meeting.meta.id, key, name);
    } catch (e) {
      notify(String(e), true);
    }
  }

  async function separate() {
    if (!meeting) return;
    separating = true;
    try {
      meeting = await api.separateSpeakers(meeting.meta.id);
      notify(`Found ${Object.keys(meeting.speakers ?? {}).length} voice(s) on the call.`);
    } catch (e) {
      notify(String(e), true);
    } finally {
      separating = false;
    }
  }

  const pauseAts = $derived(meeting?.meta.pauses?.map((p) => p.at_s) ?? []);
  const final = $derived(meeting?.summaries.filter((s) => s.kind === "final").at(-1));
  const partials = $derived(meeting?.summaries.filter((s) => s.kind === "partial") ?? []);
</script>

<section class="screen">
  <button class="back" onclick={onback}>← Meetings</button>
  {#if !meeting}
    <p class="faint">Loading…</p>
  {:else}
    <h1>{meeting.meta.title}</h1>
    <div class="muted small">
      {when(meeting.meta.started_at)} · {minutes(meeting.meta.duration_s)} · {meeting.segments.length} lines
      {#if !meeting.meta.finished}<span class="chip">unfinished</span>{/if}
    </div>
    {#if meeting.meta.attendees?.length}
      <div class="faint small">Invited: {meeting.meta.attendees.join(", ")}</div>
    {/if}
    <div class="mlabels"><LabelEditor labels={meeting.meta.labels ?? []} onchange={saveLabels} /></div>

    <div class="tabs">
      <button class="tab" class:on={tab === "summary"} onclick={() => (tab = "summary")}><img src={summaryIcon} alt="" />Summary</button>
      <button class="tab" class:on={tab === "tasks"} onclick={() => (tab = "tasks")}>✅ Tasks{#if meeting.notes?.action_items.length}<span class="count">{meeting.notes.action_items.length}</span>{/if}</button>
      <button class="tab" class:on={tab === "transcript"} onclick={() => (tab = "transcript")}><img src={transcriptionIcon} alt="" />Transcript</button>
      <button class="tab" class:on={tab === "speakers"} onclick={() => (tab = "speakers")}>👥 Speakers{#if voices.length}<span class="count">{voices.length}</span>{/if}</button>
    </div>

    {#if tab === "summary" && meeting.notes}
      {@const n = meeting.notes}
      <div class="notes body">
        {#each [{ title: "Summary", items: n.summary }, { title: "Decisions", items: n.decisions }] as sec}
          {#if sec.items.length}
            <h3>{sec.title}</h3>
            <ul>{#each sec.items as it}<li>{it.text}{#each it.refs as r}<button class="tchip" title="Show in the transcript" onclick={() => jump(r)}>{mmss(r)}</button>{/each}</li>{/each}</ul>
          {/if}
        {/each}
        {#if n.action_items.length}
          <h3>Next steps</h3>
          <ul>{#each n.action_items as a}<li>{a.text}{#if a.owner || a.due}<span class="who-due">{[a.owner, a.due].filter(Boolean).join(" · ")}</span>{/if}{#each a.refs as r}<button class="tchip" onclick={() => jump(r)}>{mmss(r)}</button>{/each}</li>{/each}</ul>
        {/if}
        {#if n.flagged.length}
          <details class="flagged">
            <summary>Couldn't verify {n.flagged.length} item{n.flagged.length === 1 ? "" : "s"}</summary>
            {#each n.flagged as f}<div class="flag"><div>{f.text}</div><div class="faint small">{f.reason}{#each f.refs as r}<button class="tchip" onclick={() => jump(r)}>{mmss(r)}</button>{/each}</div></div>{/each}
          </details>
        {/if}
      </div>
      <div class="card note">Written by {n.model} on this computer. Each point links to the lines it came from; numbers were checked against them. Small models can still reword things — click the times to check.</div>
    {:else if tab === "summary"}
      {#if final}
        <div class="md body">{@html markdown(final.text)}</div>
        <div class="card note">Written by {final.model} on this computer ({final.gen_tokens_per_s.toFixed(0)} tok/s). Small models can get details wrong — check against the transcript.</div>
      {:else}
        <p class="faint">No final summary for this meeting{meeting.segments.length ? "" : " — nothing was transcribed"}.</p>
      {/if}
      {#if meeting.segments.length && meeting.meta.finished}
        <button class="btn ghost sm" onclick={writeNotes} disabled={writing}>{writing ? "Writing checked notes… (loads the summary model)" : "Write checked notes"}</button>
      {/if}
      {#if partials.length}
        <details>
          <summary>{partials.length} summar{partials.length === 1 ? "y" : "ies"} written during the call</summary>
          {#each partials as p}
            <div class="partial"><div class="faint small">up to {mmss(p.at_s)}</div><div class="md">{@html markdown(p.text)}</div></div>
          {/each}
        </details>
      {/if}
    {:else if tab === "tasks"}
      <div class="tasks">
        {#each meeting.notes?.action_items ?? [] as a}
          <div class="task card">
            <div class="ttl">{a.text}</div>
            <div class="faint small">{a.owner ?? "No owner named"}{a.due ? ` · due ${a.due}` : ""}</div>
            {#each a.refs as r}
              {@const line = meeting.segments.find((s) => s.start === r)}
              {#if line}<button class="quote" onclick={() => jump(r)}><span class="ts">{mmss(r)}</span> {speakerName(line, meeting.speakers)}: “{line.text}”</button>{/if}
            {/each}
          </div>
        {:else}
          <p class="faint">{meeting.notes ? "No tasks were agreed in this meeting." : "Tasks come with the checked notes — see the Summary tab."}</p>
        {/each}
      </div>
    {:else if tab === "speakers"}
      <div class="voices">
        {#if voices.length === 0}
          <p class="faint">{meeting.meta.steps_done?.includes("speakers") ? "No other voices were found on the call track." : "Voices on the call haven't been separated for this meeting yet."}</p>
        {:else}
          <p class="muted small">Name each voice once — the name is used everywhere in this meeting.{#if meeting.meta.attendees?.length}{" "}Suggestions come from the calendar invite.{/if}</p>
          {#each voices as v (v.key)}
            <div class="voice card">
              <span class="swatch" style="background:{speakerColor({ speaker: v.key, track: 'call' })}"></span>
              <div class="vtxt">
                <input type="text" value={v.name} placeholder={`Person ${v.key.replace(/^s/, "")}`} list={attendeeList}
                  onchange={(e) => rename(v.key, (e.target as HTMLInputElement).value)}
                  onkeydown={(e) => e.key === "Enter" && (e.target as HTMLInputElement).blur()} />
                <div class="faint small">talks {minutes(v.talk)}{#if v.first} · first at {mmss(v.first.start)}{/if}</div>
                {#each v.samples as s}<div class="sample">“{s.text}”</div>{/each}
              </div>
            </div>
          {/each}
          <datalist id={attendeeList}>{#each meeting.meta.attendees ?? [] as a}<option value={a}></option>{/each}</datalist>
        {/if}
        <div class="again">
          <button class="btn ghost sm" onclick={separate} disabled={separating}>{separating ? "Separating voices…" : voices.length ? "↻ Separate again" : "Separate speakers"}</button>
          {#if voices.length}<span class="faint small">keeps the names given above</span>{/if}
        </div>
      </div>
    {:else}
      <div class="card tx">
        {#each meeting.segments as seg, i (i)}
          {#each pausesBefore(pauseAts, meeting.segments, i) as p}<div class="pausemark">— paused at {mmss(p)} —</div>{/each}
          <div class="line" id={`line-${seg.start}`} class:focus={focusAt === seg.start}>
            <span class="ts">{mmss(seg.start)}</span>
            <span class="who" style="color:{speakerColor(seg)}">{speakerName(seg, meeting.speakers)}</span>
            <span>{seg.text}</span>
          </div>
        {:else}
          <p class="faint">Nothing was transcribed.</p>
        {/each}
      </div>
    {/if}
  {/if}
</section>

<style>
  .screen { padding: 18px 30px 30px; }
  .back { background: none; border: 0; color: var(--ink-3); cursor: pointer; padding: 0; margin-bottom: 10px; font-weight: 700; }
  .back:hover { color: var(--ink); }
  .small { font-size: 13px; }
  .mlabels { margin-top: 10px; }
  .pausemark { text-align: center; color: var(--ink-3); font-size: 12px; font-weight: 700; padding: 8px 0; border-bottom: 1px solid #22405c; }
  .tabs { display: flex; gap: 4px; border-bottom: 1px solid var(--line); margin: 18px 0 16px; }
  .tab { background: none; border: 0; color: var(--ink-2); font-weight: 800; font-size: 14px; padding: 9px 14px; border-bottom: 3px solid transparent; cursor: pointer; display: flex; gap: 7px; align-items: center; }
  .tab.on { color: var(--ink); border-bottom-color: var(--coral); }
  .tab img { width: 22px; height: 22px; border-radius: 6px; }
  .body { max-width: 760px; }
  .note { margin-top: 14px; font-size: 12.5px; color: var(--ink-2); max-width: 760px; }
  details { margin-top: 16px; max-width: 760px; }
  summary { cursor: pointer; color: var(--ink-2); font-weight: 700; }
  .partial { border-left: 3px solid var(--line); padding-left: 12px; margin: 10px 0; }
  .tx { padding: 4px 16px; }
  .line { display: grid; grid-template-columns: 54px 96px 1fr; gap: 8px; padding: 7px 0; border-bottom: 1px solid #22405c; line-height: 1.5; }
  .line:last-child { border-bottom: 0; }
  .ts { color: var(--ink-3); font-variant-numeric: tabular-nums; font-size: 12.5px; padding-top: 2px; }
  .line .who { padding-top: 2px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .notes h3 { margin: 16px 0 6px; font-size: 15px; }
  .notes h3:first-child { margin-top: 0; }
  .notes ul { margin: 0; padding-left: 20px; line-height: 1.7; }
  .tchip { background: var(--navy-3); border: 1px solid var(--line); color: var(--ink-2); border-radius: 8px; font-size: 11px; font-weight: 700; padding: 0 6px; margin-left: 6px; cursor: pointer; font-variant-numeric: tabular-nums; }
  .tchip:hover { border-color: var(--teal); color: var(--ink); }
  .who-due { margin-left: 8px; font-size: 12px; color: var(--teal); font-weight: 700; }
  .flagged { margin-top: 14px; }
  .flag { border-left: 3px solid var(--warn, #e0b25a); padding-left: 10px; margin: 8px 0; }
  .tasks { max-width: 760px; display: flex; flex-direction: column; gap: 10px; }
  .task { padding: 12px 14px; display: flex; flex-direction: column; gap: 4px; }
  .task .ttl { font-weight: 800; }
  .quote { background: none; border: 0; border-left: 3px solid var(--line); color: var(--ink-2); text-align: left; padding: 2px 0 2px 8px; cursor: pointer; font-size: 13px; }
  .quote:hover { border-left-color: var(--teal); color: var(--ink); }
  .line.focus { background: #24425f; border-radius: 6px; }
  .count { margin-left: 4px; font-size: 11px; background: var(--navy-3); border-radius: 10px; padding: 1px 7px; }
  .voices { max-width: 760px; display: flex; flex-direction: column; gap: 10px; align-items: flex-start; }
  .voice { display: flex; gap: 12px; width: 100%; padding: 12px 14px; }
  .swatch { width: 12px; height: 12px; border-radius: 50%; margin-top: 10px; flex: none; }
  .vtxt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .vtxt input { max-width: 280px; font-weight: 700; }
  .again { display: flex; align-items: center; gap: 10px; }
  .sample { font-size: 13px; color: var(--ink-2); border-left: 3px solid var(--line); padding-left: 8px; }
</style>
