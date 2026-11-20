//! Post-call steps (spec 003; 005 and 012 add to it). Each step records itself in
//! `meta.steps_done` when it succeeds, so a failed one can be re-run alone.

use anyhow::Result;
use std::time::Instant;

use crate::config::Config;
use crate::model::{Segment, Track};
use crate::servers::{Kind, ModelServer};
use crate::store::Store;
use crate::whisper::WhisperClient;
use crate::{audio, diarize, speakers, tracks};

/// The per-track rebuild after the call (spec 018); runs before speakers and notes.
pub const STEP_TRANSCRIPT: &str = "transcript";
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

/// A loaded Whisper server and its client, handed between steps so it loads once.
pub type Asr = Option<(ModelServer, WhisperClient)>;

/// Transcribe a whole recording like a live session would (quiet-point chunks, VAD, prompt
/// carry-over), as one `track`. Gives the GPU back between chunks if a recording starts.
pub fn transcribe_samples(cfg: &Config, samples: &[i16], track: Track, hooks: &Hooks) -> Result<Vec<Segment>> {
    transcribe_track(cfg, samples, track, hooks, &mut None, (STEP_TRANSCRIBE, 0.0, 1.0))
}

/// [`transcribe_samples`] with a Whisper server that may already be loaded (kept loaded for the
/// next caller), reporting progress as `step` over `from..to`.
fn transcribe_track(cfg: &Config, samples: &[i16], track: Track, hooks: &Hooks, asr: &mut Asr, (step, from, to): (&str, f64, f64)) -> Result<Vec<Segment>> {
    let sr = audio::SAMPLE_RATE as usize;
    let chunk = cfg.chunk_seconds as usize * sr;
    let (mut pos, mut prompt, mut out) = (0, String::new(), Vec::new());
    while pos < samples.len() {
        if (hooks.yield_gpu)() {
            *asr = None; // stops the server
            hooks.wait_for_gpu();
        }
        (hooks.progress)(step, from + (to - from) * pos as f64 / samples.len() as f64);
        let rest = &samples[pos..];
        let n = audio::quiet_cut(&rest[..rest.len().min(chunk + 1)], &[], chunk, 3 * sr);
        let offset = pos as f64 / sr as f64;
        // Silent stretches aren't sent (Whisper hallucinates on them; a quiet mic is mostly
        // noise), and Whisper isn't even loaded until something is.
        if audio::has_speech(&rest[..n]) {
            if asr.is_none() {
                let server = ModelServer::start(Kind::Whisper, cfg, cfg.live_devices().0)?;
                let client = WhisperClient::new(&server.base_url)?;
                *asr = Some((server, client));
            }
            let Some((_, client)) = &asr else { unreachable!() };
            for s in client.transcribe(audio::wav_bytes(&rest[..n]), &cfg.language, &prompt)? {
                prompt = s.text.clone();
                out.push(tracks::segment(s, track, offset));
            }
        }
        pos += n;
    }
    Ok(out)
}

/// What the per-track rebuild did.
#[derive(Debug, PartialEq)]
pub enum Rebuild {
    Done { lines: usize, echoes: usize, took_s: f64 },
    /// Nothing rebuilt — the message says why; the transcript is left as it was.
    Skipped(String),
}

/// Rebuild a meeting's transcript from its audio after the call (spec 018): transcribe each whole
/// track on its own (mic = You, system audio = Call), merge by time, drop mic echoes, and replace
/// the live transcript (kept as `transcript.live.jsonl`). Missing tracks are skipped; with no
/// audio at all, nothing changes.
pub fn retranscribe(store: &Store, cfg: &Config, id: &str, hooks: &Hooks) -> Result<Rebuild> {
    retranscribe_with(store, cfg, id, hooks, &mut None)
}

/// [`retranscribe`] reusing a Whisper server the caller already has loaded (a live session).
pub fn retranscribe_with(store: &Store, cfg: &Config, id: &str, hooks: &Hooks, asr: &mut Asr) -> Result<Rebuild> {
    let t0 = Instant::now();
    let dir = store.dir(id);
    let read = |name: &str| -> Result<Vec<i16>> {
        let p = dir.join(name);
        if p.exists() { audio::read_wav(&p) } else { Ok(Vec::new()) }
    };
    let (mic, sys) = (read("mic.wav")?, read("system.wav")?);
    let total = (mic.len() + sys.len()) as f64;
    if total == 0.0 {
        return Ok(Rebuild::Skipped("The transcript wasn't rebuilt per track: the audio is no longer stored.".into()));
    }
    let split = mic.len() as f64 / total;
    let you = transcribe_track(cfg, &mic, Track::You, hooks, asr, (STEP_TRANSCRIPT, 0.0, split))?;
    let call = transcribe_track(cfg, &sys, Track::Call, hooks, asr, (STEP_TRANSCRIPT, split, 1.0))?;
    let (segments, echoes) = tracks::merge(you, call);
    if segments.is_empty() && !store.live_transcript(id).unwrap_or_default().is_empty() {
        return Ok(Rebuild::Skipped("The per-track transcript came out empty; the live transcript is kept.".into()));
    }
    store.rebuild_transcript(id, &segments)?;
    mark_done(store, id, STEP_TRANSCRIPT)?;
    if echoes > 0 {
        eprintln!("fofoca: {id}: dropped {echoes} mic line(s) echoing the call");
    }
    Ok(Rebuild::Done { lines: segments.len(), echoes, took_s: t0.elapsed().as_secs_f64() })
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
    // one track only: everything in an imported file is "the call"
    let segments = transcribe_samples(cfg, &samples, Track::Call, hooks)?;
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

/// Most voices speaker separation may find on the call track (spec 018). With ≥ 2 people on the
/// calendar invite, everyone but the user: the user is on the mic track, not the call. With 0–1
/// (imports, ad-hoc calls) there's nothing to go by, so allow a few.
pub fn max_speakers(attendees: usize) -> usize {
    if attendees >= 2 {
        attendees - 1
    } else {
        (attendees + 1).max(4)
    }
}

/// Separate the voices on the call track (CPU) and put them on the transcript. Re-running starts
/// again from the base transcript (the per-track rebuild, else the live one); names already given
/// to `s1`, `s2`, … are kept.
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
    let opts = diarize::Options { threshold: dc.threshold, max_speakers: max_speakers(meta.attendees.len()) };
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

    #[test]
    fn speaker_cap_comes_from_the_invite() {
        assert_eq!(max_speakers(2), 1, "you and one other person: one voice on the call");
        assert_eq!(max_speakers(5), 4);
        assert_eq!(max_speakers(1), 4, "no usable invite: as before");
        assert_eq!(max_speakers(0), 4);
    }

    fn no_hooks() -> (impl Fn(&str, f64), impl Fn() -> bool) {
        (|_: &str, _: f64| {}, || false)
    }

    fn write_wav(path: &std::path::Path, samples: &[i16]) {
        let mut w = audio::WavAppender::create(path).unwrap();
        w.append(samples).unwrap();
        w.finish().unwrap();
    }

    #[test]
    fn rebuild_without_audio_or_speech_never_fails_and_keeps_the_live_transcript() {
        let root = std::env::temp_dir().join(format!("fofoca-rebuild-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let mut cfg = Config::default();
        // never started: nothing below has speech
        cfg.whisper.server = root.join("no-whisper-server");
        let (progress, yield_gpu) = no_hooks();
        let hooks = Hooks { progress: &progress, yield_gpu: &yield_gpu };
        let live = Segment { start: 0.0, end: 1.0, text: "Oi".into(), track: Track::Call, words: vec![], speaker: None };

        // audio deleted (retention, spec 011)
        let gone = store.create(&StartInfo::default()).unwrap();
        store.append_segment(&gone.id, &live).unwrap();
        assert!(matches!(retranscribe(&store, &cfg, &gone.id, &hooks).unwrap(), Rebuild::Skipped(_)));
        assert_eq!(store.load(&gone.id).unwrap().segments, vec![live.clone()]);

        // only quiet noise on the mic and digital silence on the call: nothing comes out, and an
        // empty rebuild doesn't wipe the live lines
        let quiet = store.create(&StartInfo { title: "q".into(), ..Default::default() }).unwrap();
        store.append_segment(&quiet.id, &live).unwrap();
        let noise: Vec<i16> = (0..16_000 * 3).map(|i| ((i * 7919) % 301) as i16 - 150).collect();
        write_wav(&store.dir(&quiet.id).join("mic.wav"), &noise);
        write_wav(&store.dir(&quiet.id).join("system.wav"), &[0; 16_000 * 3]);
        assert!(matches!(retranscribe(&store, &cfg, &quiet.id, &hooks).unwrap(), Rebuild::Skipped(_)));
        assert_eq!(store.load(&quiet.id).unwrap().segments, vec![live]);
        assert!(!store.load(&quiet.id).unwrap().meta.steps_done.contains(&STEP_TRANSCRIPT.to_string()));

        // record-only with a missing mic track: the call track alone is used; silent → empty, done
        let ro = store.create(&StartInfo { title: "ro".into(), ..Default::default() }).unwrap();
        write_wav(&store.dir(&ro.id).join("system.wav"), &[0; 16_000 * 3]);
        match retranscribe(&store, &cfg, &ro.id, &hooks).unwrap() {
            Rebuild::Done { lines, echoes, .. } => assert_eq!((lines, echoes), (0, 0)),
            r => panic!("{r:?}"),
        }
        assert!(store.load(&ro.id).unwrap().meta.steps_done.contains(&STEP_TRANSCRIPT.to_string()));
        std::fs::remove_dir_all(&root).unwrap();
    }

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
