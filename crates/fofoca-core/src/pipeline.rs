//! Post-call steps (spec 003; 005 and 012 add to it). Each step records itself in
//! `meta.steps_done` when it succeeds, so a failed one can be re-run alone.

use anyhow::Result;
use std::time::Instant;

use crate::config::Config;
use crate::store::Store;
use crate::{audio, diarize, speakers};

pub const STEP_SPEAKERS: &str = "speakers";

pub enum StepResult {
    Done { speakers: usize, took_s: f64 },
    /// Nothing to do or not possible right now — the message says why (shown as a notice).
    Skipped(String),
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
    store.update_meta(id, |m| {
        if !m.steps_done.iter().any(|s| s == STEP_SPEAKERS) {
            m.steps_done.push(STEP_SPEAKERS.into());
        }
    })?;
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
