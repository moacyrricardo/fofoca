//! Post-call steps (spec 003; 005 and 012 add to it). Each step records itself in
//! `meta.steps_done` when it succeeds, so a failed one can be re-run alone.

use anyhow::Result;
use std::time::Instant;

use crate::config::Config;
use crate::store::Store;
use crate::{audio, diarize, speakers};

mod tracks;
pub use tracks::*;

pub const STEP_SPEAKERS: &str = "speakers";
pub const STEP_NOTES: &str = "notes";

/// Record a finished step in `meta.steps_done`.
pub fn mark_done(store: &Store, id: &str, step: &str) -> Result<()> {
    store.update_meta(id, |m| {
        if !m.steps_done.iter().any(|s| s == step) {
            m.steps_done.push(step.into());
        }
    })?;
    Ok(())
}

pub enum StepResult {
    Done { speakers: usize, took_s: f64 },
    /// Nothing to do or not possible right now — the message says why (shown as a notice).
    Skipped(String),
}

pub const STEP_CONVERT: &str = "converting";
pub const STEP_TRANSCRIBE: &str = "transcribing";

/// Callbacks for long offline work (imports, spec 012).
pub struct Hooks<'a> {
    /// `(step, fraction done 0..=1)`.
    pub progress: &'a dyn Fn(&str, f64),
    /// True while a live recording needs the GPU: model servers are stopped until it's false.
    pub yield_gpu: &'a dyn Fn() -> bool,
}

impl Hooks<'_> {
    pub(crate) fn wait_for_gpu(&self) {
        while (self.yield_gpu)() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

/// Import a recording, then run the same chain as an explicit retranscription.
pub fn run_import(store: &Store, cfg: &Config, id: &str, src: &std::path::Path, hooks: &Hooks) -> Result<()> {
    (hooks.progress)(STEP_CONVERT, 0.0);
    let wav = store.dir(id).join("system.wav");
    crate::import::convert(src, &wav)?;
    let duration = hound::WavReader::open(wav)?.duration() as f64 / audio::SAMPLE_RATE as f64;
    store.update_meta(id, |m| m.duration_s = duration)?;
    let progress = |step: &str, p| (hooks.progress)(if step == STEP_TRACKS { STEP_TRANSCRIBE } else { step }, p);
    retranscribe(store, cfg, id, &Hooks { progress: &progress, yield_gpu: hooks.yield_gpu })?;
    Ok(())
}

pub struct ChainReport {
    pub tracks: TrackCounts,
    pub speakers: String,
    pub notes: String,
}

/// Called by the queue and the CLI. Explicit requests ignore the automatic transcript toggle.
pub fn retranscribe(store: &Store, cfg: &Config, id: &str, hooks: &Hooks) -> Result<ChainReport> {
    store.update_meta(id, |m| m.processed = false)?;
    let counts = match transcribe_tracks(store, cfg, id, None, hooks)? {
        TrackResult::Skipped(reason) => anyhow::bail!(reason),
        TrackResult::Done(counts) => counts,
    };
    let (speakers, notes) = finish_transcript(store, cfg, id, hooks, true)?;
    Ok(ChainReport { tracks: counts, speakers, notes })
}

/// Drop citations from the previous transcript before running optional consumers.
/// Even a disabled or failing notes step must not display obsolete checked notes.
pub fn finish_transcript(store: &Store, cfg: &Config, id: &str, hooks: &Hooks, apply_retention: bool) -> Result<(String, String)> {
    store.clear_transcript_results(id)?;
    let mut failures = vec![];
    let mut speakers = String::from("off");
    if cfg.workflow.speakers_after && (apply_retention || store.dir(id).join("system.wav").is_file()) {
        hooks.wait_for_gpu();
        (hooks.progress)(STEP_SPEAKERS, 0.);
        match separate_speakers(store, cfg, id) {
            Ok(StepResult::Done { speakers: n, .. }) => speakers = format!("{n} voices"),
            Ok(StepResult::Skipped(reason)) => failures.push(reason),
            Err(e) => failures.push(format!("speakers failed: {e:#}")),
        }
    }
    let mut notes = String::from("off");
    if cfg.workflow.notes_after {
        let words: usize = store.load(id)?.segments.iter().map(|s| s.text.split_whitespace().count()).sum();
        if words < crate::session::MIN_WORDS_TO_SUMMARIZE {
            notes = String::from("too few words");
        } else {
            hooks.wait_for_gpu();
            (hooks.progress)(STEP_NOTES, 0.);
            match write_notes(store, cfg, id) {
                Ok(()) => notes = String::from("written"),
                Err(e) => failures.push(format!("notes failed: {e:#}")),
            }
        }
    }
    store.update_meta(id, |m| {
        if apply_retention { m.finished = true; }
        m.processed = failures.is_empty();
    })?;
    anyhow::ensure!(failures.is_empty(), "{}", failures.join("; "));
    if apply_retention {
        crate::retention::apply(store, &cfg.audio, chrono::Local::now().fixed_offset(), Some(id), false)?;
    }
    Ok((speakers, notes))
}


/// Load the summary model and write checked notes (+ the Markdown copy as the final summary).
pub fn write_notes(store: &Store, cfg: &Config, id: &str) -> Result<()> {
    use crate::model::{Summary, SummaryKind};
    use crate::servers::{Kind, ModelServer};
    let m = store.load(id)?;
    let server = ModelServer::start(Kind::Llm, cfg, cfg.workflow.notes_device)?;
    let n = crate::notes::extract(&crate::llm::LlmClient::new(&server.base_url)?, &m.segments, &m.speakers, &m.meta, cfg)?;
    drop(server);
    store.save_notes(id, &n)?;
    let text = crate::notes::to_markdown(&n, &cfg.language);
    store.add_summary(id, &Summary { kind: SummaryKind::Final, at_s: m.meta.duration_s, text, model: n.model.clone(), gen_tokens_per_s: n.gen_tokens_per_s })?;
    mark_done(store, id, STEP_NOTES)
}

/// Separate the voices on the call track (CPU) and put them on the transcript. Re-running starts
/// again from the live transcript; names already given to `s1`, `s2`, … are kept.
pub fn separate_speakers(store: &Store, cfg: &Config, id: &str) -> Result<StepResult> {
    let dc = &cfg.diarization;
    if !dc.segmentation_model.exists() || !dc.embedding_model.exists() {
        return Ok(StepResult::Skipped(format!(
            "Speaker separation skipped: models not found in {}.",
            dc.segmentation_model.parent().map(|p| p.display().to_string()).unwrap_or_default()
        )));
    }
    let wav = store.dir(id).join("system.wav");
    if !wav.exists() {
        return Ok(StepResult::Skipped("Speaker separation skipped: the call audio is no longer stored.".into()));
    }
    let t0 = Instant::now();
    let samples = audio::read_wav(&wav)?;
    let meta = store.load(id)?.meta;
    let opts = diarize::Options { threshold: dc.threshold, max_speakers: (meta.attendees.len() + 1).max(4) };
    let turns = diarize::separate(&samples, &dc.segmentation_model, &dc.embedding_model, &opts)?;
    let segments = speakers::assign(&store.base_transcript(id)?, &turns);
    let roster = speakers::roster(&segments, &store.speakers(id));
    store.replace_transcript(id, &segments)?;
    store.save_speakers(id, &roster)?;
    mark_done(store, id, STEP_SPEAKERS)?;
    Ok(StepResult::Done { speakers: roster.len(), took_s: t0.elapsed().as_secs_f64() })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Segment, StartInfo, Track};
    use crate::whisper::approx_words;

    /// Runs the real models. `FOFOCA_DIAR_WAV=<16 kHz mono wav> cargo test -- --ignored speakers_step`
    /// (pyannote-rs' public `6_speakers.wav` works); models from the default config paths.
    #[test]
    #[ignore]
    fn speakers_step_on_a_real_recording() {
        let wav = std::env::var("FOFOCA_DIAR_WAV").expect("set FOFOCA_DIAR_WAV");
        let root = std::env::temp_dir().join(format!("fofoca-pipeline-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let meta = store.create(&StartInfo { title: "diar".into(), ..Default::default() }).unwrap();
        std::fs::copy(&wav, store.dir(&meta.id).join("system.wav")).unwrap();
        let secs = audio::read_wav(std::path::Path::new(&wav)).unwrap().len() as f64 / audio::SAMPLE_RATE as f64;
        // a fake live transcript: one 5 s Call line after another
        let mut t = 0.0;
        while t < secs {
            let end = (t + 5.0).min(secs);
            let text = "palavra ".repeat(10);
            let seg = Segment { start: t, end, words: approx_words(t, end, &text), text, track: Track::Call, speaker: None };
            store.append_segment(&meta.id, &seg).unwrap();
            t = end;
        }
        let before = store.load(&meta.id).unwrap().segments.len();
        let cfg = Config::default();
        match separate_speakers(&store, &cfg, &meta.id).unwrap() {
            StepResult::Done { speakers, took_s } => {
                let m = store.load(&meta.id).unwrap();
                eprintln!("{speakers} voices in {took_s:.1}s; {before} lines -> {}", m.segments.len());
                for s in &m.segments {
                    eprintln!("{:6.1}-{:6.1} {:?}", s.start, s.end, s.speaker);
                }
                assert!(speakers >= 2, "a multi-speaker file has more than one voice");
                assert!(m.segments.iter().all(|s| s.speaker.is_some()));
                assert!(m.meta.steps_done.contains(&STEP_SPEAKERS.to_string()));
                assert_eq!(store.live_transcript(&meta.id).unwrap().len(), before);
            }
            StepResult::Skipped(msg) => panic!("skipped: {msg}"),
        }
        std::fs::remove_dir_all(&root).unwrap();
    }
}
