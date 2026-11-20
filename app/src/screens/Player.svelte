<script lang="ts">
  // Replay bar (spec 014): controls plus the speaker lane, which is also the seek bar.
  import { cycleRate, player, seek, setMuted, skip, toggle } from "../lib/player.svelte";
  import { laneBlocks, timeAtX } from "../lib/replay";
  import { mmss, speakerColor, speakerName } from "../lib/format";
  import type { Segment, SpeakerInfo } from "../lib/types";

  let { segments, speakers = {}, pauses = [], onseek }: {
    segments: Segment[]; speakers?: Record<string, SpeakerInfo>; pauses?: number[]; onseek?: () => void;
  } = $props();

  const blocks = $derived(laneBlocks(segments, player.duration));
  const legend = $derived.by(() => {
    const seen = new Map<string, { name: string; color: string }>();
    for (const s of segments) {
      const key = s.speaker ?? s.track;
      if (!seen.has(key)) seen.set(key, { name: speakerName(s, speakers), color: speakerColor(s) });
    }
    return [...seen.values()];
  });
  const pct = $derived(player.duration > 0 ? (player.time / player.duration) * 100 : 0);

  let track: HTMLDivElement;
  let dragging = $state(false);
  let hover = $state<{ x: number; t: number } | null>(null);

  function at(e: PointerEvent): number {
    const r = track.getBoundingClientRect();
    return timeAtX(e.clientX - r.left, r.width, player.duration);
  }
  function down(e: PointerEvent) {
    track.setPointerCapture(e.pointerId);
    dragging = true;
    seek(at(e));
    onseek?.();
  }
  function move(e: PointerEvent) {
    const r = track.getBoundingClientRect();
    hover = { x: Math.min(Math.max(0, e.clientX - r.left), r.width), t: at(e) };
    if (dragging) seek(at(e));
  }
  function up(e: PointerEvent) {
    if (!dragging) return;
    dragging = false;
    track.releasePointerCapture(e.pointerId);
  }
</script>

<div class="player">
  <div class="prow">
    <button class="play" onclick={toggle} title={player.playing ? "Pause (Space)" : "Play (Space)"}>{player.playing ? "❚❚" : "▶"}</button>
    <button class="skip" onclick={() => skip(-10)} title="Hear the last 10 s again">↺ 10 s</button>
    <div class="time">{mmss(player.time)} <span class="faint">/ {mmss(player.duration)}</span></div>
    <!-- svelte-ignore a11y_no_static_element_interactions -->
    <div class="track" class:dragging bind:this={track} onpointerdown={down} onpointermove={move} onpointerup={up} onpointercancel={up} onpointerleave={() => (hover = null)}>
      <div class="lane">
        {#each blocks as b (b.seg)}<div class="seg" style="left:{b.left}%;width:{b.width}%;background:{speakerColor(segments[b.seg])}"></div>{/each}
        <div class="played" style="width:{pct}%"></div>
      </div>
      {#each pauses as p}{#if player.duration > 0 && p <= player.duration}<div class="pausetick" style="left:{(p / player.duration) * 100}%" title="paused at {mmss(p)}"></div>{/if}{/each}
      <div class="knob" style="left:{pct}%"></div>
      {#if hover}<div class="hovertime" style="left:{hover.x}px">{mmss(hover.t)}</div>{/if}
    </div>
    <button class="speed" onclick={cycleRate} title="Playback speed">{player.rate}×</button>
    {#if player.hasMic}
      <button class="mute" aria-pressed={player.muted.you} onclick={() => setMuted("you", !player.muted.you)} title="Mute your microphone track">🎙 You</button>
    {/if}
    <button class="mute" aria-pressed={player.muted.call} onclick={() => setMuted("call", !player.muted.call)} title="Mute the call track">🔊 Call</button>
    <label class="follow"><input type="checkbox" bind:checked={player.follow} /> Follow</label>
  </div>
  <div class="legend">
    {#each legend as l}<span style="--c:{l.color}">{l.name}</span>{/each}
    <span class="keys">drag the bar to move · <kbd>Space</kbd> play · <kbd>←</kbd><kbd>→</kbd> 5 s · click a line to play from it</span>
  </div>
  {#if player.error}<div class="err small">Couldn't play the audio: {player.error}</div>{/if}
</div>

<style>
  .player { position: sticky; top: 0; z-index: 2; background: var(--navy-2); margin: 0 -16px 4px; padding: 12px 16px 10px; border-bottom: 1px solid var(--line); border-radius: 14px 14px 0 0; }
  .prow { display: flex; align-items: center; gap: 10px; }
  .play { width: 38px; height: 38px; border-radius: 50%; border: 0; background: var(--teal); color: #08262a; font-size: 15px; font-weight: 900; cursor: pointer; display: grid; place-items: center; flex: none; }
  .play:hover { background: #7fd0d4; }
  .skip { background: none; border: 1.5px solid var(--line); color: var(--ink-2); border-radius: 9px; font-weight: 800; font-size: 12px; padding: 4px 8px; cursor: pointer; white-space: nowrap; }
  .skip:hover { border-color: var(--teal); color: var(--ink); }
  .time { font-variant-numeric: tabular-nums; font-weight: 800; font-size: 13px; min-width: 96px; white-space: nowrap; }
  .time .faint { font-weight: 600; }
  .track { position: relative; flex: 1; min-width: 120px; height: 26px; cursor: pointer; touch-action: none; }
  .lane { position: absolute; left: 0; right: 0; height: 8px; top: 9px; background: #12263a; border-radius: 6px; overflow: hidden; }
  .seg { position: absolute; top: 0; bottom: 0; opacity: .55; }
  .played { position: absolute; left: 0; top: 0; bottom: 0; background: #ffffff22; }
  .knob { position: absolute; top: 5px; width: 16px; height: 16px; margin-left: -8px; border-radius: 50%; background: var(--ink); box-shadow: 0 0 0 3px #64bfc455; pointer-events: none; }
  .track.dragging .knob { box-shadow: 0 0 0 5px #64bfc466; }
  .pausetick { position: absolute; top: 4px; width: 2px; height: 18px; background: var(--ink-3); }
  .hovertime { position: absolute; top: -22px; transform: translateX(-50%); background: #0f2234; border: 1px solid var(--line); border-radius: 6px; font-size: 11px; font-weight: 800; padding: 1px 6px; pointer-events: none; white-space: nowrap; }
  .speed { background: var(--navy-3); border: 1px solid var(--line); color: var(--ink); border-radius: 8px; font-weight: 800; font-size: 12px; padding: 4px 8px; cursor: pointer; min-width: 46px; }
  .mute { background: transparent; border: 1.5px solid var(--line); color: var(--ink); border-radius: 8px; font-weight: 800; font-size: 12px; padding: 4px 8px; cursor: pointer; white-space: nowrap; }
  .mute:hover { border-color: var(--teal); }
  .mute[aria-pressed="true"] { color: var(--ink-3); text-decoration: line-through; border-style: dashed; }
  .follow { display: inline-flex; gap: 6px; align-items: center; font-size: 12.5px; color: var(--ink-2); cursor: pointer; white-space: nowrap; }
  .follow input { accent-color: var(--teal); }
  .legend { display: flex; gap: 14px; margin-top: 8px; font-size: 11.5px; color: var(--ink-3); flex-wrap: wrap; }
  .legend span::before { content: ""; display: inline-block; width: 9px; height: 9px; border-radius: 3px; margin-right: 5px; background: var(--c); vertical-align: -1px; }
  .legend .keys { margin-left: auto; }
  .legend .keys::before { display: none; }
  kbd { font-family: inherit; font-size: 10.5px; border: 1px solid var(--line); border-bottom-width: 2px; border-radius: 4px; padding: 0 4px; color: var(--ink-2); }
  .err { color: var(--coral); margin-top: 6px; }
  .small { font-size: 12.5px; }
</style>
