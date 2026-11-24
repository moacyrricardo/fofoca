<script lang="ts">
  import { api, preview } from "../lib/api";
  import { onDestroy, tick } from "svelte";
  import { markdown, minutes, mmss, pausesBefore, speakerColor, speakerName, when } from "../lib/format";
  import type { JobEvent, Meeting } from "../lib/types";
  import { JOB_STEPS } from "../lib/api";
  import summaryIcon from "../assets/summary.png";
  import transcriptionIcon from "../assets/transcription.png";
  import LabelEditor from "./LabelEditor.svelte";
  import Player from "./Player.svelte";
  import MiniPlayer from "./MiniPlayer.svelte";
  import { load as loadAudio, play, player, seek, skip, toggle, unload as unloadAudio } from "../lib/player.svelte";
  import { lineAt, wordAt, wordTimes } from "../lib/replay";

  let { id, job, recording, onback, notify, initialTab, backLabel = "Meetings" }: {
    id: string; recording: boolean; job?: JobEvent; onback: () => void; notify: (t: string, e?: boolean) => void;
    /** Open on this tab (spec 016: a task opens its meeting on Tasks). */
    initialTab?: "tasks"; backLabel?: string;
  } = $props();
  const processing = $derived(!!job && !["done", "failed"].includes(job.step));

  const jobFinished = $derived(job?.step === "done" || job?.step === "failed");
  let requesting = $state(false);
  async function retranscribe() {
    requesting = true;
    try { await api.retranscribe(id); }
    catch (e) { notify(String(e), true); }
    finally { requesting = false; }
  }
  let loadedId: string | undefined;
  let meeting = $state<Meeting | null>(null);
  let tab = $state<"summary" | "tasks" | "transcript" | "speakers">("summary");
  let separating = $state(false);
  let writing = $state(false);
  /** Transcript line highlighted after following a time chip. */
  let focusAt = $state<number | null>(null);

  async function jump(t: number) {
    tab = "transcript";
    focusAt = t;
    if (replay) playFrom(t);
    await tick();
    document.getElementById(`line-${t}`)?.scrollIntoView({ block: "center", behavior: "smooth" });
  }

  // ── Replay (spec 014) ────────────────────────────────────────────────────────────────────
  /** "loading" until we know; "none" when the meeting has no audio to play. */
  let audio = $state<"loading" | "ok" | "none">("loading");
  const replay = $derived(audio === "ok" && player.meetingId === id);
  /** Playback has started at least once, so the playing line is shown. */
  const active = $derived(replay && (player.playing || player.time > 0));
  const starts = $derived(meeting?.segments.map((s) => s.start) ?? []);
  const nowIdx = $derived(active ? lineAt(starts, player.time) : -1);
  const nowWords = $derived(nowIdx >= 0 && meeting ? wordTimes(meeting.segments[nowIdx]) : []);
  const nowWord = $derived(wordAt(nowWords, player.time));
  /** The user scrolled away from the playing line while Follow is on. */
  let away = $state(false);

  function playFrom(t: number) {
    seek(t);
    away = false;
    void play();
  }

  function scrollToNow(smooth = true) {
    if (!meeting || nowIdx < 0) return;
    document.getElementById(`line-${meeting.segments[nowIdx].start}`)?.scrollIntoView({ block: "center", behavior: smooth ? "smooth" : "auto" });
  }
  $effect(() => {
    void nowIdx;
    if (tab === "transcript" && player.follow && player.playing && !away) scrollToNow();
  });

  function scrolledByUser() {
    if (tab === "transcript" && player.follow && player.playing) away = true;
  }
  function onkey(e: KeyboardEvent) {
    const el = e.target as HTMLElement | null;
    if (["PageUp", "PageDown", "Home", "End"].includes(e.key)) return scrolledByUser();
    if (!replay || tab !== "transcript" || e.ctrlKey || e.altKey || e.metaKey) return;
    if (el && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT", "BUTTON"].includes(el.tagName))) return;
    if (e.key === " ") toggle();
    else if (e.key === "ArrowLeft") skip(-5);
    else if (e.key === "ArrowRight") skip(5);
    else return;
    e.preventDefault();
  }
  function clickLine(t: number) {
    if (!replay || window.getSelection()?.toString()) return; // selecting text isn't a click
    playFrom(t);
  }
  onDestroy(unloadAudio); // leaving the meeting page stops playback


  let confirmDelete = $state(false);
  async function keepAudio(keep: boolean) {
    if (!meeting) return;
    try {
      meeting = await api.setKeepAudio(meeting.meta.id, keep);
    } catch (e) {
      notify(String(e), true);
    }
  }
  async function deleteAudio() {
    if (!meeting) return;
    confirmDelete = false;
    try {
      unloadAudio();
      audio = "none";
      meeting = await api.deleteAudio(meeting.meta.id);
      notify("Audio deleted — the transcript and notes are kept.");
    } catch (e) {
      notify(String(e), true);
    }
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
    void jobFinished;
    const firstLoad = loadedId !== want;
    if (firstLoad) meeting = null;
    let current = true;
    if (player.meetingId !== want) {
      unloadAudio();
      audio = "loading";
    }
    api.meeting(want).then((m) => {
      if (current && want === id) {
        loadedId = want;
        meeting = m;
        if (recording || m.meta.audio_deleted_at) audio = "none";
        else api.audioTracks(want).then((urls) => {
          if (!current || want !== id) return;
          loadAudio(want, urls, Math.max(m.meta.duration_s, m.segments.at(-1)?.end ?? 0));
          audio = urls.mic || urls.call ? "ok" : "none";
        }).catch((e) => { audio = "none"; notify(String(e), true); });
        if (firstLoad) tab = initialTab ?? (m.notes || m.summaries.some((s) => s.kind === "final") ? "summary" : "transcript");
        if (firstLoad && preview && ["#speakers", "#transcript", "#tasks"].includes(location.hash)) tab = location.hash.slice(1) as typeof tab; // browser preview only
      }
    }).catch((e) => notify(String(e), true));
    return () => { current = false; };
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

<svelte:window onkeydown={onkey} onwheel={scrolledByUser} ontouchmove={scrolledByUser} />

<section class="screen">
  <button class="back" onclick={onback}>← {backLabel}</button>
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
    {#if processing && job}
      <div class="card importing"><span class="spin"></span><div><b>Processing recording</b>
        <div class="faint small">{JOB_STEPS[job.step] ?? job.step}{["tracks", "transcribing"].includes(job.step) ? ` · ${Math.round(job.progress * 100)}%` : ""}. Recording a meeting pauses this job.</div></div></div>
    {:else if meeting.meta.imported_from}
      <div class="faint small">⤓ Imported from {meeting.meta.imported_from}</div>
    {/if}
    <div class="mlabels"><LabelEditor labels={meeting.meta.labels ?? []} onchange={saveLabels} /></div>
    {#if meeting.meta.finished}
      <div class="audio faint small">
        {#if meeting.meta.audio_deleted_at}
          🔇 Audio deleted {when(meeting.meta.audio_deleted_at)} — transcript and notes are kept.
        {:else}
          🎙 Audio kept on this computer.
          <label class="keep"><input type="checkbox" checked={meeting.meta.keep_audio === true} onchange={(e) => keepAudio((e.target as HTMLInputElement).checked)} /> Keep it regardless of the Storage setting</label>
          {#if confirmDelete}
            <span class="sure">Delete the recording? <button class="link danger" onclick={deleteAudio}>Delete</button> · <button class="link" onclick={() => (confirmDelete = false)}>Cancel</button></span>
          {:else}
            <button class="link" disabled={processing} onclick={() => (confirmDelete = true)}>Delete audio now</button>
          {/if}
        {/if}
      </div>
    {/if}

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
        <button class="btn ghost sm" onclick={writeNotes} disabled={writing || processing}>{writing ? "Writing checked notes… (loads the summary model)" : "Write checked notes"}</button>
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
          <button class="btn ghost sm" onclick={separate} disabled={separating || processing || !!meeting.meta.audio_deleted_at} title={meeting.meta.audio_deleted_at ? "The audio was deleted" : ""}>{separating ? "Separating voices…" : voices.length ? "↻ Separate again" : "Separate speakers"}</button>
          {#if voices.length}<span class="faint small">keeps the names given above</span>{/if}
        </div>
      </div>
    {:else}
      <button class="btn ghost sm" onclick={retranscribe}
        disabled={audio !== "ok" || recording || processing || requesting || separating || writing}
        title={meeting.meta.audio_deleted_at || (!recording && audio === "none") ? "The audio was deleted" : undefined}>Transcribe again from the audio</button>
      <div class="card tx">
        {#if replay}
          <Player segments={meeting.segments} speakers={meeting.speakers} pauses={pauseAts} onseek={() => (away = false)} />
        {:else if audio === "none" && meeting.meta.finished && meeting.segments.length}
          <div class="gone">🔇 {meeting.meta.audio_deleted_at
            ? `The audio of this meeting was deleted on ${new Date(meeting.meta.audio_deleted_at).toLocaleDateString([], { day: "numeric", month: "short" })} — the transcript stays, replay isn't available.`
            : "No audio was kept for this meeting — replay isn't available."}</div>
        {/if}
        {#each meeting.segments as seg, i (i)}
          {#each pausesBefore(pauseAts, meeting.segments, i) as p}<div class="pausemark">— paused at {mmss(p)} —</div>{/each}
          <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
          <div class="line" id={`line-${seg.start}`} class:focus={focusAt === seg.start && !active} class:now={i === nowIdx}
            class:playable={replay} class:muted={replay && player.muted[seg.track]} onclick={() => clickLine(seg.start)}
            title={replay ? "Play from here" : undefined}>
            <span class="ts">{mmss(seg.start)}</span>
            <span class="who" style="color:{speakerColor(seg)}">{speakerName(seg, meeting.speakers)}</span>
            <span>{#if i === nowIdx}{#each nowWords as w, wi}<span class="w" class:said={wi < nowWord} class:cur={wi === nowWord}>{w.text}</span>{" "}{/each}{:else}{seg.text}{/if}</span>
          </div>
        {:else}
          <p class="faint">Nothing was transcribed.</p>
        {/each}
        {#if active && player.follow && away}
          <button class="btn ghost sm backto" onclick={() => { away = false; scrollToNow(); }}>↓ Back to playing line</button>
        {/if}
      </div>
    {/if}
    {#if active && tab !== "transcript"}
      {@const seg = nowIdx >= 0 ? meeting.segments[nowIdx] : null}
      <div class="minispace"></div>
      <MiniPlayer line={seg ? `${speakerName(seg, meeting.speakers)}: ${seg.text}` : ""} ongo={() => { tab = "transcript"; away = false; tick().then(() => scrollToNow(false)); }} />
    {/if}
  {/if}
</section>

<style>
  .screen { padding: 18px 30px 30px; }
  .back { background: none; border: 0; color: var(--ink-3); cursor: pointer; padding: 0; margin-bottom: 10px; font-weight: 700; }
  .back:hover { color: var(--ink); }
  .small { font-size: 13px; }
  .mlabels { margin-top: 10px; }
  .importing { display: flex; gap: 12px; align-items: center; margin-top: 12px; max-width: 760px; }
  .audio { margin-top: 10px; display: flex; flex-wrap: wrap; gap: 6px 14px; align-items: center; }
  .keep { display: inline-flex; gap: 6px; align-items: center; cursor: pointer; }
  .keep input { accent-color: var(--teal); }
  .link { background: none; border: 0; color: var(--teal); cursor: pointer; font-weight: 700; padding: 0; font-size: inherit; }
  .link.danger { color: var(--coral); }
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
  /* replay (spec 014) */
  .gone { color: var(--ink-3); font-size: 13px; padding: 10px 0; border-bottom: 1px solid #22405c; }
  .line.playable { cursor: pointer; }
  .line .ts { position: relative; }
  .line.playable:hover .ts::before { content: "▶"; position: absolute; left: -14px; top: 2px; font-size: 10px; color: var(--teal); }
  .line.now { background: #24425f; border-radius: 6px; }
  .line.now .ts { color: var(--teal); font-weight: 800; }
  .line.muted { opacity: .45; }
  .w { border-radius: 4px; transition: background .12s, color .12s; }
  .line.now .w { color: var(--ink-2); }
  .line.now .w.said { color: var(--ink); }
  .w.cur { background: #64bfc433; color: #fff; box-shadow: 0 0 0 2px #64bfc433; }
  .minispace { height: 64px; } /* room for the fixed mini player under the last lines */
  .backto { position: sticky; bottom: 10px; float: right; margin: 8px 0; z-index: 2; }
  .count { margin-left: 4px; font-size: 11px; background: var(--navy-3); border-radius: 10px; padding: 1px 7px; }
  .voices { max-width: 760px; display: flex; flex-direction: column; gap: 10px; align-items: flex-start; }
  .voice { display: flex; gap: 12px; width: 100%; padding: 12px 14px; }
  .swatch { width: 12px; height: 12px; border-radius: 50%; margin-top: 10px; flex: none; }
  .vtxt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 4px; }
  .vtxt input { max-width: 280px; font-weight: 700; }
  .again { display: flex; align-items: center; gap: 10px; }
  .sample { font-size: 13px; color: var(--ink-2); border-left: 3px solid var(--line); padding-left: 8px; }
</style>
