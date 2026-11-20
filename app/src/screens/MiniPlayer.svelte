<script lang="ts">
  // Mini player (spec 014): keeps playback reachable while another tab of the meeting is open.
  import { player, toggle } from "../lib/player.svelte";
  import { mmss } from "../lib/format";

  let { line, ongo }: { line: string; ongo: () => void } = $props();
  const pct = $derived(player.duration > 0 ? (player.time / player.duration) * 100 : 0);
</script>

<div class="mini">
  <button class="play" onclick={toggle} title={player.playing ? "Pause" : "Play"}>{player.playing ? "❚❚" : "▶"}</button>
  <span class="ts">{mmss(player.time)}</span>
  <span class="txt">{line}</span>
  <span class="bar"><i style="width:{pct}%"></i></span>
  <button class="link" onclick={ongo}>Transcript</button>
</div>

<style>
  /* centred in the content area (the window minus App's sidebar) */
  .mini { position: fixed; bottom: 16px; left: calc(50% + var(--side-w, 0px) / 2); transform: translateX(-50%); width: max-content; max-width: min(640px, calc(100vw - var(--side-w, 0px) - 32px)); display: flex; align-items: center; gap: 10px; background: #0f2234; border: 1px solid #3f8d93; border-radius: 30px; padding: 6px 14px 6px 6px; box-shadow: 0 10px 26px #0008; z-index: 4; font-size: 13px; }
  .play { width: 30px; height: 30px; border-radius: 50%; border: 0; background: var(--teal); color: #08262a; font-size: 12px; font-weight: 900; cursor: pointer; display: grid; place-items: center; flex: none; }
  .ts { color: var(--ink-3); font-variant-numeric: tabular-nums; font-weight: 700; }
  .txt { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; min-width: 0; }
  .bar { width: 120px; height: 4px; background: #12263a; border-radius: 3px; overflow: hidden; flex: none; }
  .bar i { display: block; height: 100%; background: var(--teal); }
  .link { background: none; border: 0; color: var(--teal); cursor: pointer; font-weight: 700; padding: 0; font-size: inherit; white-space: nowrap; }
</style>
