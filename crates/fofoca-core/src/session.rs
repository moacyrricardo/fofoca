//! A live recording session (spec 001; layouts, pacing and record-only from spec 015).
//!
//! ```text
//! pw-record mic ─▶ mic.wav + mic slicer ───┐  slices at each track's own pauses (spec 019)
//! pw-record sys ─▶ system.wav + call slicer ┴─▶ queue ─▶ one worker ─▶ whisper-server (GPU) ─▶ lines
//!                                                                 ▲  swap (~3 s)
//!                        "summary" ─▶ stop whisper ───────────────┴─▶ llama-server (GPU) ─▶ summary ─▶ restart whisper
//! ```
//!
//! Each track is transcribed on its own (spec 018), so a line's track is the file it came from.
//! Each track is cut at its own pauses and silence is never sent (spec 019): one request per
//! turn, about as many as transcribing the two tracks mixed.
//!
//! Capture never stops while models swap: audio piles up in the track buffers (the backlog) and
//! is transcribed once Whisper is back. Other layouts (spec 015) run Whisper and/or the summary
//! model on the CPU, or both on the GPU at once, and then nothing is swapped. With the live
//! transcript off ("record only") the audio is only written, and transcribed after the call.

use anyhow::Result;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::{self, WavAppender, SAMPLE_RATE};
use crate::capture::{Capture, CaptureStopper, Source, TrackBuffer};
use crate::config::{Config, Device};
use crate::llm::{Completion, LlmClient};
use crate::notes;
use crate::model::{MeetingMeta, Pause, Segment, StartInfo, Summary, SummaryKind, Track};
use crate::pipeline::{self, StepResult};
use crate::prompts;
use crate::queue::{self, SliceQueue, BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S};
use crate::servers::{Kind, ModelServer};
use crate::slicer::{Slicer, SlicerConfig};
use crate::store::Store;
use crate::whisper::WhisperClient;

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ModelState {
    Off,
    Loading,
    Ready,
    Busy,
}

/// Everything the UI needs to follow a session. Serialized with a `type` tag.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionEvent {
    Started { meeting: MeetingMeta },
    Status { elapsed_s: f64, backlog_s: f64, mic_level: f32, call_level: f32, asr: ModelState, llm: ModelState, paused: bool },
    /// Recording paused at meeting time `at_s`: audio from now on is dropped (spec 010).
    Paused { at_s: f64 },
    Resumed { at_s: f64 },
    Segment { segment: Segment },
    /// Capture stopped; post-call work may still be queued behind model loading.
    Stopping,
    /// A post-call step started: `transcribing`, `transcript` (per-track rebuild, spec 018),
    /// `speakers`, `summarizing`, `saving`.
    Step { step: String },
    /// How far a long post-call step is, 0..=1 (the per-track rebuild).
    Progress { step: String, progress: f64 },
    Summary { summary: Summary },
    Finished { meeting: MeetingMeta },
    /// The recording was thrown away: its folder (audio, transcript, summaries) is deleted.
    Discarded { meeting_id: String },
    /// Informational message for the user (not an error).
    Notice { message: String },
    Error { message: String },
}

pub type EventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

enum Command {
    Summarize,
    Pause,
    Resume,
    Stop { summarize: bool },
    Discard,
}

/// Below this many transcribed words a summary would only be invented — don't ask the LLM.
pub const MIN_WORDS_TO_SUMMARIZE: usize = 25;

pub struct Session {
    emit: EventSink,
    tx: Sender<Command>,
    stoppers: [CaptureStopper; 2],
    buffers: [Arc<TrackBuffer>; 2],
    shared: Arc<Shared>,
    worker: Option<JoinHandle<()>>,
    pub meeting: MeetingMeta,
}

struct Shared {
    asr: Mutex<ModelState>,
    llm: Mutex<ModelState>,
    running: AtomicBool,
    /// Seconds of audio sliced but not transcribed yet: queued, open or held (spec 019).
    queued_s: Mutex<f64>,
    /// When the current pause began, and the total of earlier pauses — the timer excludes both.
    paused: Mutex<(Option<Instant>, Duration)>,
}

impl Shared {
    fn paused_for(&self) -> Duration {
        let p = self.paused.lock().unwrap();
        p.1 + p.0.map(|t| t.elapsed()).unwrap_or_default()
    }
    fn is_paused(&self) -> bool {
        self.paused.lock().unwrap().0.is_some()
    }
}

impl Session {
    /// Create the meeting folder, start both captures immediately and start loading Whisper.
    pub fn start(cfg: Config, store: Store, info: &StartInfo, on_event: EventSink) -> Result<Session> {
        let meeting = store.create(info)?;
        let mic = Capture::start(Source::Microphone)?;
        let sys = Capture::start(Source::SystemAudio)?;
        on_event(SessionEvent::Started { meeting: meeting.clone() });

        let shared = Arc::new(Shared {
            asr: Mutex::new(ModelState::Off),
            llm: Mutex::new(ModelState::Off),
            running: AtomicBool::new(true),
            queued_s: Mutex::new(0.0),
            paused: Mutex::new((None, Duration::ZERO)),
        });
        let (tx, rx) = mpsc::channel();
        let stoppers = [mic.stopper(), sys.stopper()];
        let buffers = [mic.buffer.clone(), sys.buffer.clone()];
        let ticker = spawn_ticker(shared.clone(), mic.buffer.clone(), sys.buffer.clone(), on_event.clone());
        let mut worker = Worker {
            dir: store.dir(&meeting.id),
            cfg,
            store,
            meeting: meeting.clone(),
            emit: on_event.clone(),
            shared: shared.clone(),
            mic,
            sys,
            asr: None,
            llm: None,
            segments: Vec::new(),
            speakers: Default::default(),
            post_ok: true,
            offset_s: 0.0,
            last_summary_at: 0.0,
            aligned: false,
        };
        let handle = std::thread::Builder::new().name("session".into()).spawn(move || {
            worker.run(rx);
            let _ = ticker.join();
        })?;
        Ok(Session { emit: on_event, tx, stoppers, buffers, shared, worker: Some(handle), meeting })
    }

    /// Stop keeping audio until `resume` — takes effect immediately, even mid model swap.
    /// What was already captured is transcribed right away.
    pub fn pause(&self) {
        {
            let mut p = self.shared.paused.lock().unwrap();
            if p.0.is_some() {
                return;
            }
            p.0 = Some(Instant::now());
        }
        for b in &self.buffers {
            b.set_discard(true);
        }
        let _ = self.tx.send(Command::Pause);
    }

    pub fn resume(&self) {
        {
            let mut p = self.shared.paused.lock().unwrap();
            let Some(since) = p.0.take() else { return };
            p.1 += since.elapsed();
        }
        for b in &self.buffers {
            b.set_discard(false);
        }
        let _ = self.tx.send(Command::Resume);
    }

    pub fn is_paused(&self) -> bool {
        self.shared.is_paused()
    }

    /// Ask for a "summary so far" (swaps Whisper out and the LLM in, then back).
    pub fn summarize(&self) {
        let _ = self.tx.send(Command::Summarize);
    }

    /// Stop recording. Returns immediately; the session finishes transcription, writes the final
    /// summary if `summarize` (and enough was said), and emits `Finished`.
    pub fn stop(&self, summarize: bool) {
        self.stop_capture();
        (self.emit)(SessionEvent::Stopping);
        let _ = self.tx.send(Command::Stop { summarize });
    }

    /// Stop recording and delete everything recorded so far. Emits `Discarded`.
    pub fn discard(&self) {
        self.stop_capture();
        let _ = self.tx.send(Command::Discard);
    }

    fn stop_capture(&self) {
        for s in &self.stoppers {
            s.stop();
        }
    }

    /// Block until the session has fully finished.
    pub fn wait(mut self) {
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

fn spawn_ticker(
    shared: Arc<Shared>,
    mic: Arc<crate::capture::TrackBuffer>,
    sys: Arc<crate::capture::TrackBuffer>,
    emit: EventSink,
) -> JoinHandle<()> {
    let started = Instant::now();
    std::thread::spawn(move || {
        while shared.running.load(Ordering::Relaxed) {
            // captured but not sliced yet (e.g. during a model swap), plus sliced but not transcribed
            let backlog = mic.len().min(sys.len()) as f64 / SAMPLE_RATE as f64 + *shared.queued_s.lock().unwrap();
            emit(SessionEvent::Status {
                elapsed_s: started.elapsed().saturating_sub(shared.paused_for()).as_secs_f64(),
                paused: shared.is_paused(),
                backlog_s: backlog,
                mic_level: mic.level(),
                call_level: sys.level(),
                asr: *shared.asr.lock().unwrap(),
                llm: *shared.llm.lock().unwrap(),
            });
            std::thread::sleep(Duration::from_millis(250));
        }
    })
}

enum Outcome {
    Finished,
    Discarded,
}

struct Worker {
    cfg: Config,
    store: Store,
    dir: std::path::PathBuf,
    meeting: MeetingMeta,
    emit: EventSink,
    shared: Arc<Shared>,
    mic: Capture,
    sys: Capture,
    asr: Option<(ModelServer, WhisperClient)>,
    /// The summary model, kept loaded between summaries when it doesn't have to make room for
    /// Whisper (spec 015: loading it on the CPU takes ~1 min from disk).
    llm: Option<(ModelServer, Device)>,
    segments: Vec<Segment>,
    /// Every post-call step that needs the audio succeeded (spec 011).
    post_ok: bool,
    /// Named voices once speakers are separated (spec 003).
    speakers: std::collections::BTreeMap<String, crate::model::SpeakerInfo>,
    /// Meeting time (s) of the audio taken from the captures so far.
    offset_s: f64,
    /// Meeting time of the last "summary so far" (for the automatic cadence, spec 015).
    last_summary_at: f64,
    /// The later capture's start was padded so both tracks share t = 0 (spec 018).
    aligned: bool,
}

impl Worker {
    fn run(&mut self, rx: Receiver<Command>) {
        let result = self.run_inner(rx);
        self.asr = None;
        self.llm = None;
        self.set(Kind::Whisper, ModelState::Off);
        self.shared.running.store(false, Ordering::Relaxed);
        let discarded = match result {
            Ok(Outcome::Discarded) => true,
            Ok(Outcome::Finished) => false,
            Err(e) => {
                self.error(format!("{e:#}"));
                false
            }
        };
        if discarded {
            if let Err(e) = std::fs::remove_dir_all(&self.dir) {
                self.error(format!("could not delete the discarded recording: {e}"));
            }
            (self.emit)(SessionEvent::Discarded { meeting_id: self.meeting.id.clone() });
            return;
        }
        let duration = self.offset_s;
        let processed = self.post_ok;
        match self.store.update_meta(&self.meeting.id, |m| {
            m.duration_s = duration;
            m.finished = true;
            m.processed = processed;
        }) {
            Ok(m) => self.meeting = m,
            Err(_) => {
                self.meeting.duration_s = duration;
                self.meeting.finished = true;
                let _ = self.store.save_meta(&self.meeting);
            }
        }
        // Spec 011: with "delete after processing", the audio goes now.
        match crate::retention::apply(&self.store, &self.cfg.audio, chrono::Local::now().fixed_offset(), Some(&self.meeting.id), false) {
            Ok(freed) if !freed.is_empty() => {
                self.notice("Audio deleted after processing, as set in Settings — the transcript and notes are kept.");
                if let Ok(m) = self.store.load(&self.meeting.id) {
                    self.meeting = m.meta;
                }
            }
            Ok(_) => {}
            Err(e) => self.error(format!("deleting audio: {e:#}")),
        }
        (self.emit)(SessionEvent::Finished { meeting: self.meeting.clone() });
    }

    fn run_inner(&mut self, rx: Receiver<Command>) -> Result<Outcome> {
        let mut mic_wav = WavAppender::create(&self.dir.join("mic.wav"))?;
        let mut sys_wav = WavAppender::create(&self.dir.join("system.wav"))?;
        let live = self.cfg.workflow.live_transcript;
        let pace = self.cfg.pace();
        if live {
            self.start_asr();
        }
        let mut stopping = false;
        let mut final_summary = true;
        let mut pending_summary = false;
        let mut flush = false;
        // one slicer per track, one queue, this thread as the only Whisper user (spec 019)
        let scfg = SlicerConfig::for_config(&self.cfg);
        let mut slicers = [Slicer::new(Track::You, scfg), Slicer::new(Track::Call, scfg)];
        let mut queue = SliceQueue::default();

        loop {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    Command::Summarize => pending_summary = true,
                    Command::Pause => {
                        // Capture is already discarding; what's buffered goes before the pause.
                        let buffered = self.mic.buffer.len().max(self.sys.buffer.len());
                        let at_s = self.offset_s + buffered as f64 / SAMPLE_RATE as f64;
                        flush = true;
                        let pause = Pause { at_s, from: chrono::Local::now().to_rfc3339(), to: None };
                        self.update_meta(|m| m.pauses.push(pause));
                        (self.emit)(SessionEvent::Paused { at_s });
                    }
                    Command::Resume => {
                        let now = chrono::Local::now().to_rfc3339();
                        self.update_meta(|m| {
                            if let Some(p) = m.pauses.last_mut().filter(|p| p.to.is_none()) {
                                p.to = Some(now);
                            }
                        });
                        (self.emit)(SessionEvent::Resumed { at_s: self.offset_s });
                    }
                    Command::Discard => {
                        self.mic.stop();
                        self.sys.stop();
                        let _ = mic_wav.finish();
                        let _ = sys_wav.finish();
                        return Ok(Outcome::Discarded);
                    }
                    Command::Stop { summarize } if !stopping => {
                        stopping = true;
                        final_summary = summarize;
                        self.mic.stop();
                        self.sys.stop();
                        (self.emit)(SessionEvent::Step { step: "transcribing".into() });
                    }
                    Command::Stop { .. } => {}
                }
            }

            if !self.aligned {
                self.align_tracks();
            }
            // Take what both tracks have, the same amount from each so they stay on one
            // timeline; everything once capture has stopped or paused.
            let n = if stopping || flush {
                self.mic.buffer.len().max(self.sys.buffer.len())
            } else {
                self.mic.buffer.len().min(self.sys.buffer.len())
            };
            if n > 0 {
                self.aligned = true; // too late to pad once audio has been written
                let m = self.mic.buffer.take(n);
                let s = self.sys.buffer.take(n);
                mic_wav.append(&m)?;
                sys_wav.append(&s)?;
                let len = m.len().max(s.len());
                if live {
                    // the call first: a quiet mic slice under call audio without text yet is held
                    // as a likely echo until that call audio is transcribed (spec 019)
                    let t = self.offset_s;
                    queue.call_audio(&s);
                    queue.call_audio(&vec![0; len - s.len()]); // keep the levels on the timeline
                    slicers[1].push(t, &s).into_iter().for_each(|x| queue.push(x));
                    queue.set_call_open(slicers[1].pending_from());
                    slicers[0].push(t, &m).into_iter().for_each(|x| queue.push(x));
                }
                self.offset_s += len as f64 / SAMPLE_RATE as f64;
            }
            if stopping || flush {
                // a pause or the end closes every slice: nothing is held across it
                flush = false;
                slicers[1].finish().into_iter().for_each(|x| queue.push(x));
                queue.set_call_open(None);
                slicers[0].finish().into_iter().for_each(|x| queue.push(x));
            }
            *self.shared.queued_s.lock().unwrap() = queue.queued_s() + slicers.iter().map(Slicer::pending_s).sum::<f64>();
            if let Some(every) = pace.summary_every_s.filter(|_| !stopping) {
                // automatic "summary so far" (spec 015) — only once there's something to summarize
                if self.offset_s - self.last_summary_at >= every as f64 && self.word_count() >= MIN_WORDS_TO_SUMMARIZE {
                    pending_summary = true;
                }
            }
            if pending_summary && !stopping {
                // Before the next request, not once the queue is empty: with Whisper near real
                // time the queue rarely empties, and slices keep queuing meanwhile (spec 019 §2).
                pending_summary = false;
                self.last_summary_at = self.offset_s;
                self.summarize(SummaryKind::Partial);
                continue;
            }
            if live && self.transcribe_next(&mut queue) {
                continue; // work through the backlog before sleeping
            }
            if stopping {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        mic_wav.finish()?;
        sys_wav.finish()?;
        // Record-only has no transcript yet, so it always runs; after a live transcript it's the
        // `transcript_after` step (spec 018).
        if !live || self.cfg.workflow.transcript_after {
            self.rebuild_transcript();
        }
        // Whisper isn't needed any more; free the GPU before the CPU and summary steps.
        self.asr = None;
        self.set(Kind::Whisper, ModelState::Off);
        if !self.cfg.workflow.speakers_after {
            // turned off in Settings (spec 015)
        } else if !self.segments.is_empty() {
            (self.emit)(SessionEvent::Step { step: pipeline::STEP_SPEAKERS.into() });
            match pipeline::separate_speakers(&self.store, &self.cfg, &self.meeting.id) {
                Ok(StepResult::Done { .. }) => match self.store.load(&self.meeting.id) {
                    Ok(m) => {
                        self.segments = m.segments;
                        self.speakers = m.speakers;
                    }
                    Err(e) => self.error(format!("reloading the transcript: {e:#}")),
                },
                Ok(StepResult::Skipped(msg)) => {
                    self.post_ok = false;
                    self.notice(&msg);
                }
                Err(e) => {
                    self.post_ok = false;
                    self.error(format!("speaker separation failed (the transcript is kept as is): {e:#}"));
                }
            }
        }
        if !final_summary || !self.cfg.workflow.notes_after {
            // the user stopped without a summary, or turned notes off in Settings
        } else if self.word_count() >= MIN_WORDS_TO_SUMMARIZE {
            (self.emit)(SessionEvent::Step { step: "summarizing".into() });
            self.summarize(SummaryKind::Final);
        } else {
            self.notice("Too little was said for a summary — the transcript is saved.");
        }
        (self.emit)(SessionEvent::Step { step: "saving".into() });
        Ok(Outcome::Finished)
    }

    /// Transcribe each saved track on its own with the GPU free of live work and replace the live
    /// transcript (spec 018) — also how record-only (spec 015) gets its transcript. Reuses the
    /// live Whisper if it's loaded. A failure keeps the live transcript and the audio.
    fn rebuild_transcript(&mut self) {
        (self.emit)(SessionEvent::Step { step: pipeline::STEP_TRANSCRIPT.into() });
        let emit = self.emit.clone();
        let progress = move |step: &str, progress: f64| emit(SessionEvent::Progress { step: step.into(), progress });
        let hooks = pipeline::Hooks { progress: &progress, yield_gpu: &|| false };
        let mut asr = self.asr.take();
        self.set(Kind::Whisper, ModelState::Busy);
        let result = pipeline::retranscribe_with(&self.store, &self.cfg, &self.meeting.id, &hooks, &mut asr);
        self.set(Kind::Whisper, if asr.is_some() { ModelState::Ready } else { ModelState::Off });
        self.asr = asr;
        match result {
            Ok(pipeline::Rebuild::Done { .. }) => match self.store.load(&self.meeting.id) {
                Ok(m) => self.segments = m.segments,
                Err(e) => self.error(format!("reloading the transcript: {e:#}")),
            },
            Ok(pipeline::Rebuild::Skipped(msg)) => self.notice(&msg),
            Err(e) => {
                self.post_ok = false;
                self.error(format!("transcribing each track failed (the live transcript is kept): {e:#}"));
            }
        }
    }

    /// Send the next request from the queue (spec 019) to Whisper and save and show its lines.
    /// Returns false when nothing was ready.
    fn transcribe_next(&mut self, queue: &mut SliceQueue) -> bool {
        let Some(req) = queue.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S) else { return false };
        if self.asr.is_none() {
            self.start_asr();
        }
        let Some((_, client)) = &self.asr else {
            queue.done(&req); // the error is shown; the audio is kept
            return true;
        };
        self.set(Kind::Whisper, ModelState::Busy);
        let lang = self.cfg.language.clone();
        let out = queue::transcribe(&req, &self.segments, |samples, prompt| client.transcribe(audio::wav_bytes(samples), &lang, prompt));
        self.set(Kind::Whisper, ModelState::Ready);
        queue.done(&req);
        let (lines, echoes) = match out {
            Ok(x) => x,
            Err(e) => {
                self.error(format!("transcription failed for {:.0}–{:.0} s ({}): {e:#}", req.start_s, req.end_s, track_name(req.track)));
                return true;
            }
        };
        if echoes > 0 {
            eprintln!("fofoca: {:.0}–{:.0} s: dropped or cut {echoes} mic line(s) echoing the call", req.start_s, req.end_s);
        }
        for seg in lines {
            if let Err(e) = self.store.append_segment(&self.meeting.id, &seg) {
                self.error(format!("saving transcript: {e:#}"));
            }
            // placed by start: the summary input reads them in that order (spec 019)
            crate::tracks::insert_by_start(&mut self.segments, seg.clone());
            (self.emit)(SessionEvent::Segment { segment: seg });
        }
        true
    }

    /// Once both captures have delivered audio, pad the one that started later with that much
    /// silence, so mic.wav and system.wav (and every chunk) start at the same moment (spec 018).
    fn align_tracks(&mut self) {
        let (mic, sys) = (&self.mic.buffer, &self.sys.buffer);
        if mic.first_sample_at().is_none() || sys.first_sample_at().is_none() {
            return;
        }
        self.aligned = true;
        if let Some((late, n)) = crate::capture::start_lag(mic.first_sample_at(), sys.first_sample_at()) {
            let (buffer, name) = match late {
                Source::Microphone => (mic, "microphone"),
                Source::SystemAudio => (sys, "call audio"),
            };
            buffer.pad_front(n);
            eprintln!("fofoca: the {name} started {:.0} ms after the other track; padded its start", n as f64 * 1000.0 / SAMPLE_RATE as f64);
        }
    }

    /// Change meta.json on disk (re-read first, see `Store::update_meta`) and keep our copy.
    fn update_meta(&mut self, f: impl FnOnce(&mut MeetingMeta)) {
        match self.store.update_meta(&self.meeting.id, f) {
            Ok(m) => self.meeting = m,
            Err(e) => self.error(format!("saving meeting info: {e:#}")),
        }
    }

    fn word_count(&self) -> usize {
        self.segments.iter().map(|s| s.text.split_whitespace().count()).sum()
    }

    fn notice(&self, message: &str) {
        (self.emit)(SessionEvent::Notice { message: message.into() });
    }

    /// Swap Whisper out, run the LLM, swap Whisper back in (partial only).
    fn summarize(&mut self, kind: SummaryKind) {
        if kind == SummaryKind::Partial && self.word_count() < MIN_WORDS_TO_SUMMARIZE {
            self.notice("Not enough has been said yet to summarize.");
            return;
        }
        // Spec 015: Whisper gives the GPU up only when both models need it and don't share it.
        let swapped = kind == SummaryKind::Final || self.cfg.live_swaps();
        if swapped {
            if let Some((server, _)) = self.asr.take() {
                server.stop();
            }
            self.set(Kind::Whisper, ModelState::Off);
        }
        let device = if kind == SummaryKind::Final { self.cfg.workflow.notes_device } else { self.cfg.live_devices().1 };
        self.set(Kind::Llm, ModelState::Loading);
        let mut grounded = None;
        let mut fallback = None;
        let server = match self.llm.take() {
            Some((s, d)) if d == device => Ok(s),
            other => {
                drop(other); // a different device: stop it before loading again
                ModelServer::start(Kind::Llm, &self.cfg, device)
            }
        };
        let mut keep = None;
        let result = server.and_then(|server| {
            self.set(Kind::Llm, ModelState::Busy);
            let lang = self.cfg.language.as_str();
            let client = LlmClient::new(&server.base_url)?;
            let text = prompts::transcript_text(&self.segments, &self.speakers, lang);
            let out = match kind {
                SummaryKind::Partial => client.complete(&prompts::partial(&text, lang), 350),
                // Spec 005: checked notes with citations; plain summary only if extraction fails.
                SummaryKind::Final => match notes::extract(&client, &self.segments, &self.speakers, &self.meeting, &self.cfg) {
                    Ok(n) => {
                        let c = Completion { text: notes::to_markdown(&n, lang), gen_tokens_per_s: n.gen_tokens_per_s };
                        grounded = Some(n);
                        Ok(c)
                    }
                    Err(e) => {
                        fallback = Some(format!("{e:#}"));
                        client.complete(&prompts::final_summary(&text, lang), 700)
                    }
                },
            };
            if kind == SummaryKind::Partial && !swapped {
                keep = Some(server);
            } else {
                server.stop();
            }
            out
        });
        let kept = keep.is_some();
        self.llm = keep.map(|s| (s, device));
        self.set(Kind::Llm, if kept { ModelState::Ready } else { ModelState::Off });
        if let Some(n) = grounded {
            match self.store.save_notes(&self.meeting.id, &n) {
                Ok(()) => {
                    let _ = pipeline::mark_done(&self.store, &self.meeting.id, pipeline::STEP_NOTES);
                }
                Err(e) => self.error(format!("saving notes: {e:#}")),
            }
        }
        if kind == SummaryKind::Final && (fallback.is_some() || result.is_err()) {
            self.post_ok = false;
        }
        if let Some(why) = fallback {
            self.notice(&format!("Couldn't write checked notes ({why}); wrote a plain summary instead."));
        }
        match result {
            Ok(c) => {
                let summary = Summary {
                    kind,
                    at_s: self.offset_s,
                    text: c.text,
                    model: self.cfg.llm_name(),
                    gen_tokens_per_s: c.gen_tokens_per_s,
                };
                if let Err(e) = self.store.add_summary(&self.meeting.id, &summary) {
                    self.error(format!("saving summary: {e:#}"));
                }
                (self.emit)(SessionEvent::Summary { summary });
            }
            Err(e) => self.error(format!("summary failed: {e:#}")),
        }
        if kind == SummaryKind::Partial && swapped && self.cfg.workflow.live_transcript {
            self.start_asr();
        }
    }

    fn start_asr(&mut self) {
        self.set(Kind::Whisper, ModelState::Loading);
        match ModelServer::start(Kind::Whisper, &self.cfg, self.cfg.live_devices().0).and_then(|s| {
            let c = WhisperClient::new(&s.base_url)?;
            Ok((s, c))
        }) {
            Ok(pair) => {
                self.asr = Some(pair);
                self.set(Kind::Whisper, ModelState::Ready);
            }
            Err(e) => {
                self.set(Kind::Whisper, ModelState::Off);
                self.error(format!("could not start Whisper (audio is still being recorded): {e:#}"));
            }
        }
    }

    fn set(&self, kind: Kind, state: ModelState) {
        let slot = match kind {
            Kind::Whisper => &self.shared.asr,
            Kind::Llm => &self.shared.llm,
        };
        *slot.lock().unwrap() = state;
    }

    fn error(&self, message: String) {
        (self.emit)(SessionEvent::Error { message });
    }
}

fn track_name(t: Track) -> &'static str {
    match t {
        Track::You => "microphone",
        Track::Call => "call audio",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_serialize_with_type_tag() {
        let e = SessionEvent::Step { step: "saving".into() };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"type":"step","step":"saving"}"#);
    }
}
