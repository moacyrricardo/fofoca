<script lang="ts">
  import type { ConfigView } from "../lib/types";
  import settingsIcon from "../assets/settings.png";

  let { config }: { config: ConfigView | null } = $props();
</script>

<section class="screen">
  <h1>Settings</h1>
  <div class="muted">fo-foca runs everything on this computer.</div>

  <div class="recom">
    <img src={settingsIcon} alt="" />
    <div>
      <div class="faint label">CURRENT SETUP</div>
      <b class="big">Swap layout</b>
      <div class="muted small">Whisper stays on the GPU while you talk; the summary model is swapped in for summaries (~3 s).
        Automatic hardware detection and overrides arrive with Auto setup.</div>
    </div>
  </div>

  {#if config}
    <div class="grid">
      <div class="card">
        <h2>Speech → text</h2>
        <dl>
          <dt>Model</dt><dd>{config.whisper_model}</dd>
          <dt>GPU</dt><dd>whisper.cpp device {config.config.whisper.device}</dd>
          <dt>Voice detection</dt><dd>{config.config.whisper.vad_model ? "on (Silero)" : "off"}</dd>
          <dt>Language</dt><dd>{config.config.language}</dd>
          <dt>Chunk</dt><dd>{config.config.chunk_seconds} s</dd>
        </dl>
      </div>
      <div class="card">
        <h2>Summaries</h2>
        <dl>
          <dt>Model</dt><dd>{config.llm_model}</dd>
          <dt>GPU</dt><dd>{config.config.llm.device}</dd>
          <dt>Context</dt><dd>{config.config.llm.context.toLocaleString()} tokens</dd>
          <dt>Threads</dt><dd>{config.config.llm.threads}</dd>
        </dl>
      </div>
    </div>
    <div class="card files">
      <div><span class="faint">Config file</span> <code>{config.path}</code></div>
      <div><span class="faint">Meetings folder</span> <code>{config.meetings_dir}</code></div>
    </div>
  {:else}
    <p class="faint">Loading…</p>
  {/if}
</section>

<style>
  .screen { padding: 26px 30px 30px; }
  .recom { display: flex; gap: 14px; align-items: center; background: linear-gradient(120deg, #1e4650, #1b3a4f); border: 1px solid #3f8d93; border-radius: 16px; padding: 14px 16px; margin: 18px 0 16px; max-width: 900px; }
  .recom img { width: 64px; height: 64px; border-radius: 14px; }
  .label { font-size: 12px; font-weight: 800; letter-spacing: .5px; }
  .big { font-size: 17px; }
  .small { font-size: 13px; }
  .grid { display: grid; grid-template-columns: 1fr 1fr; gap: 16px; max-width: 900px; }
  dl { display: grid; grid-template-columns: 130px 1fr; gap: 8px 12px; margin: 0; font-size: 13.5px; }
  dt { color: var(--ink-3); }
  dd { margin: 0; overflow-wrap: anywhere; }
  .files { margin-top: 16px; max-width: 900px; display: flex; flex-direction: column; gap: 6px; font-size: 13px; }
  code { font-size: 12.5px; color: var(--ink-2); }
</style>
