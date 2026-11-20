//! Post-call steps (spec 003; 005 and 012 add to it). Each step records itself in
//! `meta.steps_done` when it succeeds, so a failed one can be re-run alone.

use anyhow::Result;
use std::time::Instant;

use crate::config::Config;
use crate::model::{Segment, Track};
use crate::queue::{self, SliceQueue, ALWAYS_MERGE, MERGE_GAP_S, MERGE_MAX_S};
use crate::servers::{Kind, ModelServer};
use crate::slicer::{Slicer, SlicerConfig};
use crate::store::Store;
use crate::whisper::{RawSegment, WhisperClient};
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

/// Transcribe a whole recording as one `track` (imports), through the same slicer and queue as a
/// live session (spec 019) in rebuild mode. Gives the GPU back between requests if a recording
/// starts.
pub fn transcribe_samples(cfg: &Config, samples: &[i16], track: Track, hooks: &Hooks) -> Result<Vec<Segment>> {
    Ok(transcribe_tracks(cfg, &[(track, samples)], hooks, &mut None, (STEP_TRANSCRIBE, 0.0, 1.0))?.0)
}

/// [`slice_and_transcribe`] with Whisper, loaded only once there's speech and kept loaded in
/// `asr` for the next caller; progress is reported as `step` over `from..to`.
fn transcribe_tracks(cfg: &Config, tracks: &[(Track, &[i16])], hooks: &Hooks, asr: &mut Asr, (step, from, to): (&str, f64, f64)) -> Result<(Vec<Segment>, usize)> {
    let progress = |done: f64| (hooks.progress)(step, from + (to - from) * done);
    slice_and_transcribe(tracks, progress, |samples, prompt| {
        if (hooks.yield_gpu)() {
            *asr = None; // stops the server
            hooks.wait_for_gpu();
        }
        if asr.is_none() {
            let server = ModelServer::start(Kind::Whisper, cfg, cfg.live_devices().0)?;
            let client = WhisperClient::new(&server.base_url)?;
            *asr = Some((server, client));
        }
        let Some((_, client)) = &asr else { unreachable!() };
        client.transcribe(audio::wav_bytes(samples), &cfg.language, prompt)
    })
}

/// Slice whole tracks at their own pauses (rebuild mode: no holding) and send them through one
/// queue to `transcribe` (samples, prompt), earliest start first, merging same-track neighbours
/// whenever possible (§6). Returns the lines by start and how many mic lines were dropped or cut
/// as echoes; `progress` gets the share of audio done before each request.
fn slice_and_transcribe(
    tracks: &[(Track, &[i16])],
    progress: impl Fn(f64),
    mut transcribe: impl FnMut(&[i16], &str) -> Result<Vec<RawSegment>>,
) -> Result<(Vec<Segment>, usize)> {
    let sr = audio::SAMPLE_RATE as usize;
    let mut queue = SliceQueue::default();
    // the call first, so mic slices are compared with the call audio for echoes
    let mut ordered: Vec<&(Track, &[i16])> = tracks.iter().collect();
    ordered.sort_by_key(|(t, _)| *t != Track::Call);
    for &&(track, samples) in &ordered {
        if track == Track::Call {
            queue.call_audio(samples);
        }
        let mut slicer = Slicer::new(track, SlicerConfig::rebuild());
        for (i, piece) in samples.chunks(10 * sr).enumerate() {
            slicer.push((i * 10) as f64, piece).into_iter().for_each(|s| queue.push(s));
        }
        slicer.finish().into_iter().for_each(|s| queue.push(s));
    }
    let total = queue.queued_s();
    let (mut lines, mut echoes) = (Vec::new(), 0);
    while let Some(req) = queue.next(ALWAYS_MERGE, MERGE_GAP_S, MERGE_MAX_S) {
        progress((1.0 - (queue.queued_s() + (req.end_s - req.start_s)) / total.max(1e-9)).clamp(0.0, 1.0));
        let (new, n) = queue::transcribe(&req, &lines, &mut transcribe)?;
        queue.done(&req);
        echoes += n;
        for line in new {
            tracks::insert_by_start(&mut lines, line);
        }
    }
    Ok((lines, echoes))
}

/// What the per-track rebuild did.
#[derive(Debug, PartialEq)]
pub enum Rebuild {
    Done { lines: usize, echoes: usize, took_s: f64 },
    /// Nothing rebuilt — the message says why; the transcript is left as it was.
    Skipped(String),
}

/// Rebuild a meeting's transcript from its audio after the call (spec 018): transcribe each track
/// on its own (mic = You, system audio = Call) through the slice queue (spec 019), drop mic
/// echoes, and replace the live transcript (kept as `transcript.live.jsonl`). Missing tracks are
/// skipped; with no audio at all, nothing changes.
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
    let present: Vec<(Track, &[i16])> = [(Track::You, mic.as_slice()), (Track::Call, sys.as_slice())].into_iter().filter(|(_, s)| !s.is_empty()).collect();
    let (segments, echoes) = transcribe_tracks(cfg, &present, hooks, asr, (STEP_TRANSCRIPT, 0.0, 1.0))?;
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

    /// Spec 019 §6: the rebuild goes through the slicers and one queue. Two call turns 2 s apart
    /// go out as one merged request, your reply between them as its own, the quiet echo of the
    /// call on the mic is held until the call has its text and then dropped, and the lines come
    /// back in start order with meeting times.
    #[test]
    fn rebuild_slices_both_tracks_through_one_queue() {
        const SR: usize = audio::SAMPLE_RATE as usize;
        let voice = |len: f64, turns: &[(f64, f64)], amp: i16| -> Vec<i16> {
            (0..(len * SR as f64) as usize)
                .map(|i| {
                    let t = i as f64 / SR as f64;
                    let on = turns.iter().any(|&(a, b)| t >= a && t < b) && i % (SR / 4) < SR * 15 / 100;
                    let noise = ((i * 7919) % 301) as i16 - 150;
                    if on { noise.saturating_add(if i % 2 == 0 { amp } else { -amp }) } else { noise }
                })
                .collect()
        };
        let call = voice(30.0, &[(1.0, 6.0), (8.0, 12.0)], 6000);
        // your reply at 14–18 s, and the call heard through the speakers at −20 dB during 1–6 s
        let mut mic = voice(30.0, &[(14.0, 18.0)], 6000);
        for (m, c) in mic.iter_mut().zip(voice(30.0, &[(1.0, 6.0)], 600)).skip(SR / 2).take(6 * SR) {
            *m = m.saturating_add(c);
        }
        let mut requests = Vec::new();
        let (lines, echoes) = slice_and_transcribe(&[(Track::You, &mic), (Track::Call, &call)], |_| {}, |samples, prompt| {
            let secs = samples.len() as f64 / SR as f64;
            requests.push((secs.round(), prompt.to_string()));
            let text = match requests.len() {
                1 => "Bom dia, vamos começar pelo relatório.",
                2 => "bom dia vamos começar pelo relatório",
                _ => "Pode ser, eu mando hoje.",
            };
            Ok(vec![RawSegment { start: 0.1, end: secs - 0.1, text: text.into(), words: crate::whisper::approx_words(0.1, secs - 0.1, text) }])
        })
        .unwrap();
        assert_eq!(requests.len(), 3, "{requests:?}");
        assert!(requests[0].0 >= 11.0, "the two call turns in one request: {requests:?}");
        assert_eq!(requests[1].1, "", "the echo starts before any text");
        assert_eq!(requests[2].1, "Bom dia, vamos começar pelo relatório.", "the reply's prompt is what came before it");
        assert_eq!(echoes, 1);
        let got: Vec<(Track, f64)> = lines.iter().map(|l| (l.track, l.start.round())).collect();
        assert_eq!(got, vec![(Track::Call, 1.0), (Track::You, 14.0)]);
    }

    /// Runs Whisper (spec 018): the per-track rebuild on a real mic + call pair, e.g. a recorded
    /// meeting's `mic.wav` and `system.wav`.
    /// `FOFOCA_MIC_WAV=<mic.wav> FOFOCA_CALL_WAV=<system.wav> cargo test -p fofoca-core -- --ignored per_track_rebuild --nocapture`
    #[test]
    #[ignore]
    fn per_track_rebuild_on_a_real_recording() {
        let (mic, call) = (std::env::var("FOFOCA_MIC_WAV").expect("set FOFOCA_MIC_WAV"), std::env::var("FOFOCA_CALL_WAV").expect("set FOFOCA_CALL_WAV"));
        let root = std::env::temp_dir().join(format!("fofoca-rebuild-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let meta = store.create(&StartInfo { title: "per track".into(), ..Default::default() }).unwrap();
        std::fs::copy(&mic, store.dir(&meta.id).join("mic.wav")).unwrap();
        std::fs::copy(&call, store.dir(&meta.id).join("system.wav")).unwrap();
        // a live transcript as the mixed path made it: one line, guessed as Call
        let live = Segment { start: 0.0, end: 5.0, text: "linha ao vivo".into(), track: Track::Call, words: vec![], speaker: None };
        store.append_segment(&meta.id, &live).unwrap();
        let (progress, yield_gpu) = no_hooks();
        let hooks = Hooks { progress: &progress, yield_gpu: &yield_gpu };
        let r = retranscribe(&store, &Config::default(), &meta.id, &hooks).unwrap();
        eprintln!("{r:?}");
        let m = store.load(&meta.id).unwrap();
        let (mic_s, call_s) = (audio::read_wav(mic.as_ref()).unwrap(), audio::read_wav(call.as_ref()).unwrap());
        let level = |v: &[i16], s: &Segment| {
            let (a, b) = ((s.start * 16_000.0) as usize, (s.end * 16_000.0) as usize);
            audio::rms(&v[a.min(v.len())..b.min(v.len())])
        };
        let span = |v: &[i16], s: &Segment| v[((s.start * 16_000.0) as usize).min(v.len())..((s.end * 16_000.0) as usize).min(v.len())].to_vec();
        let (mut louder, mut voiced) = (0, 0);
        for s in &m.segments {
            let (own, other) = match s.track {
                Track::You => (&mic_s, &call_s),
                Track::Call => (&call_s, &mic_s),
            };
            let (lo, lt) = (level(own, s), level(other, s));
            louder += usize::from(lo > lt);
            voiced += usize::from(audio::has_speech(&span(own, s)));
            eprintln!("{:6.1}-{:6.1} {:?} own {lo:.3} other {lt:.3}  {}", s.start, s.end, s.track, s.text);
        }
        // A line can span several of one person's turns (VAD drops the other person's turns from
        // each track), so "louder over the whole line" is only a diagnostic.
        eprintln!("{louder}/{} lines louder on their own track", m.segments.len());
        assert!(m.segments.iter().any(|s| s.track == Track::You) && m.segments.iter().any(|s| s.track == Track::Call), "both tracks speak");
        assert!(m.segments.windows(2).all(|w| w[0].start <= w[1].start), "merged by time");
        assert_eq!(voiced, m.segments.len(), "every line has speech on its own track where it says it is");
        assert_eq!(store.live_transcript(&meta.id).unwrap(), vec![live], "live copy kept");
        assert_eq!(store.base_transcript(&meta.id).unwrap(), m.segments);
        assert!(m.meta.steps_done.contains(&STEP_TRANSCRIPT.to_string()));
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
