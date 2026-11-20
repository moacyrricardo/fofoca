//! Post-call steps (spec 003; 005 and 012 add to it). Each step records itself in
//! `meta.steps_done` when it succeeds, so a failed one can be re-run alone.

use anyhow::Result;
use std::time::Instant;

use crate::config::Config;
use crate::store::Store;
use crate::model::{Segment, Track, Word};
use crate::{audio, diarize, speakers};

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
    fn wait_for_gpu(&self) {
        while (self.yield_gpu)() {
            std::thread::sleep(std::time::Duration::from_secs(1));
        }
    }
}

/// Transcribe a whole recording like a live session would (quiet-point chunks, VAD, prompt
/// carry-over), on the Call track. Gives the GPU back between chunks if a recording starts.
pub fn transcribe_samples(cfg: &Config, samples: &[i16], hooks: &Hooks) -> Result<Vec<Segment>> {
    use crate::servers::{Kind, ModelServer};
    use crate::whisper::WhisperClient;
    let sr = audio::SAMPLE_RATE as usize;
    let chunk = cfg.chunk_seconds as usize * sr;
    let (mut pos, mut prompt, mut out) = (0, String::new(), Vec::new());
    let mut asr: Option<(ModelServer, WhisperClient)> = None;
    while pos < samples.len() {
        if (hooks.yield_gpu)() {
            asr = None; // stops the server
            hooks.wait_for_gpu();
        }
        if asr.is_none() {
            let server = ModelServer::start(Kind::Whisper, cfg, cfg.live_devices().0)?;
            let client = WhisperClient::new(&server.base_url)?;
            asr = Some((server, client));
        }
        let Some((_, client)) = &asr else { unreachable!() };
        (hooks.progress)(STEP_TRANSCRIBE, pos as f64 / samples.len() as f64);
        let rest = &samples[pos..];
        let n = audio::quiet_cut(&rest[..rest.len().min(chunk + 1)], &[], chunk, 3 * sr);
        let offset = pos as f64 / sr as f64;
        if audio::rms(&rest[..n]) >= 0.003 {
            for s in client.transcribe(audio::wav_bytes(&rest[..n]), &cfg.language, &prompt)? {
                prompt = s.text.clone();
                out.push(Segment {
                    start: offset + s.start,
                    end: offset + s.end,
                    words: s.words.into_iter().map(|w| Word { start: offset + w.start, end: offset + w.end, ..w }).collect(),
                    text: s.text,
                    track: Track::Call,
                    speaker: None,
                });
            }
        }
        pos += n;
    }
    Ok(out)
}

/// Import a recording made elsewhere into meeting `id` (created by the caller): convert, transcribe,
/// separate speakers, write checked notes, apply the audio policy. Each step is recorded so the
/// meeting page can show progress; a failing step after transcription doesn't lose the transcript.
pub fn run_import(store: &Store, cfg: &Config, id: &str, src: &std::path::Path, hooks: &Hooks) -> Result<()> {
    (hooks.progress)(STEP_CONVERT, 0.0);
    let wav = store.dir(id).join("system.wav");
    crate::import::convert(src, &wav)?;
    let samples = audio::read_wav(&wav)?;
    let duration = samples.len() as f64 / audio::SAMPLE_RATE as f64;
    let segments = transcribe_samples(cfg, &samples, hooks)?;
    for s in &segments {
        store.append_segment(id, s)?;
    }
    store.update_meta(id, |m| m.duration_s = duration)?;
    mark_done(store, id, STEP_TRANSCRIBE)?;

    // `processed` = every step turned on in Settings succeeded (spec 015)
    let mut ok = true;
    if cfg.workflow.speakers_after {
        (hooks.progress)(STEP_SPEAKERS, 0.0);
        match separate_speakers(store, cfg, id) {
            Ok(StepResult::Done { .. }) => {}
            Ok(StepResult::Skipped(_)) | Err(_) => ok = false,
        }
    }
    let words: usize = segments.iter().map(|s| s.text.split_whitespace().count()).sum();
    if cfg.workflow.notes_after && words >= crate::session::MIN_WORDS_TO_SUMMARIZE {
        (hooks.progress)(STEP_NOTES, 0.0);
        hooks.wait_for_gpu();
        // a failure here keeps the transcript and speakers; the notes can be written again later
        if write_notes(store, cfg, id).is_err() {
            ok = false;
        }
    }
    store.update_meta(id, |m| {
        m.finished = true;
        m.processed = ok;
    })?;
    crate::retention::apply(store, &cfg.audio, chrono::Local::now().fixed_offset(), Some(id), false)?;
    Ok(())
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
    let segments = speakers::assign(&store.live_transcript(id)?, &turns);
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
