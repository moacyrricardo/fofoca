//! A live recording session (spec 001, "swap" layout).
//!
//! ```text
//! pw-record mic ─┐                      ┌─ mic.wav / system.wav
//! pw-record sys ─┴─▶ 30 s chunks ─▶ mix ─▶ whisper-server (GPU) ─▶ segments (You/Call by energy)
//!                                                    ▲  swap (~3 s)
//!                        "summary" ─▶ stop whisper ──┴─▶ llama-server (GPU) ─▶ summary ─▶ restart whisper
//! ```
//!
//! Capture never stops while models swap: audio piles up in the track buffers (the backlog) and
//! is transcribed once Whisper is back.

use anyhow::Result;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::audio::{self, WavAppender, SAMPLE_RATE};
use crate::capture::{Capture, CaptureStopper, Source};
use crate::config::Config;
use crate::llm::LlmClient;
use crate::model::{MeetingMeta, Segment, Summary, SummaryKind, Word};
use crate::prompts;
use crate::servers::{Kind, ModelServer};
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
    Status { elapsed_s: f64, backlog_s: f64, mic_level: f32, call_level: f32, asr: ModelState, llm: ModelState },
    Segment { segment: Segment },
    /// A post-call step started: `transcribing`, `summarizing`, `saving`.
    Step { step: String },
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
    Stop { summarize: bool },
    Discard,
}

/// Below this many transcribed words a summary would only be invented — don't ask the LLM.
pub const MIN_WORDS_TO_SUMMARIZE: usize = 25;

pub struct Session {
    tx: Sender<Command>,
    stoppers: [CaptureStopper; 2],
    worker: Option<JoinHandle<()>>,
    pub meeting: MeetingMeta,
}

struct Shared {
    asr: Mutex<ModelState>,
    llm: Mutex<ModelState>,
    running: AtomicBool,
}

impl Session {
    /// Create the meeting folder, start both captures immediately and start loading Whisper.
    pub fn start(cfg: Config, store: Store, title: &str, attendees: &[String], labels: &[String], on_event: EventSink) -> Result<Session> {
        let meeting = store.create(title, attendees, labels)?;
        let mic = Capture::start(Source::Microphone)?;
        let sys = Capture::start(Source::SystemAudio)?;
        on_event(SessionEvent::Started { meeting: meeting.clone() });

        let shared = Arc::new(Shared {
            asr: Mutex::new(ModelState::Off),
            llm: Mutex::new(ModelState::Off),
            running: AtomicBool::new(true),
        });
        let (tx, rx) = mpsc::channel();
        let stoppers = [mic.stopper(), sys.stopper()];
        let ticker = spawn_ticker(shared.clone(), mic.buffer.clone(), sys.buffer.clone(), on_event.clone());
        let mut worker = Worker {
            dir: store.dir(&meeting.id),
            cfg,
            store,
            meeting: meeting.clone(),
            emit: on_event,
            shared,
            mic,
            sys,
            asr: None,
            segments: Vec::new(),
            offset_s: 0.0,
        };
        let handle = std::thread::Builder::new().name("session".into()).spawn(move || {
            worker.run(rx);
            let _ = ticker.join();
        })?;
        Ok(Session { tx, stoppers, worker: Some(handle), meeting })
    }

    /// Ask for a "summary so far" (swaps Whisper out and the LLM in, then back).
    pub fn summarize(&self) {
        let _ = self.tx.send(Command::Summarize);
    }

    /// Stop recording. Returns immediately; the session finishes transcription, writes the final
    /// summary if `summarize` (and enough was said), and emits `Finished`.
    pub fn stop(&self, summarize: bool) {
        self.stop_capture();
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
            let backlog = mic.len().min(sys.len()) as f64 / SAMPLE_RATE as f64;
            emit(SessionEvent::Status {
                elapsed_s: started.elapsed().as_secs_f64(),
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
    segments: Vec<Segment>,
    /// Meeting time (s) at which the next chunk starts.
    offset_s: f64,
}

impl Worker {
    fn run(&mut self, rx: Receiver<Command>) {
        let result = self.run_inner(rx);
        self.asr = None;
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
        match self.store.update_meta(&self.meeting.id, |m| {
            m.duration_s = duration;
            m.finished = true;
        }) {
            Ok(m) => self.meeting = m,
            Err(_) => {
                self.meeting.duration_s = duration;
                self.meeting.finished = true;
                let _ = self.store.save_meta(&self.meeting);
            }
        }
        (self.emit)(SessionEvent::Finished { meeting: self.meeting.clone() });
    }

    fn run_inner(&mut self, rx: Receiver<Command>) -> Result<Outcome> {
        let mut mic_wav = WavAppender::create(&self.dir.join("mic.wav"))?;
        let mut sys_wav = WavAppender::create(&self.dir.join("system.wav"))?;
        self.start_asr();
        let chunk = (self.cfg.chunk_seconds.max(5) * SAMPLE_RATE) as usize;
        let mut stopping = false;
        let mut final_summary = true;
        let mut pending_summary = false;

        loop {
            while let Ok(cmd) = rx.try_recv() {
                match cmd {
                    Command::Summarize => pending_summary = true,
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

            let available = self.mic.buffer.len().min(self.sys.buffer.len());
            let n = if available >= chunk {
                let (m, s) = (self.mic.buffer.peek(chunk), self.sys.buffer.peek(chunk));
                audio::quiet_cut(&m, &s, chunk, 3 * SAMPLE_RATE as usize)
            } else if stopping {
                self.mic.buffer.len().max(self.sys.buffer.len())
            } else {
                0
            };
            if n > 0 {
                let m = self.mic.buffer.take(n);
                let s = self.sys.buffer.take(n);
                mic_wav.append(&m)?;
                sys_wav.append(&s)?;
                self.transcribe_chunk(&m, &s);
                continue; // drain the backlog before anything else
            }
            if stopping {
                break;
            }
            if pending_summary {
                pending_summary = false;
                self.summarize(SummaryKind::Partial);
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        mic_wav.finish()?;
        sys_wav.finish()?;
        if !final_summary {
            // the user chose to stop without a summary
        } else if self.word_count() >= MIN_WORDS_TO_SUMMARIZE {
            (self.emit)(SessionEvent::Step { step: "summarizing".into() });
            self.summarize(SummaryKind::Final);
        } else {
            self.notice("Too little was said for a summary — the transcript is saved.");
        }
        (self.emit)(SessionEvent::Step { step: "saving".into() });
        Ok(Outcome::Finished)
    }

    fn transcribe_chunk(&mut self, mic: &[i16], sys: &[i16]) {
        let len_s = mic.len().max(sys.len()) as f64 / SAMPLE_RATE as f64;
        let offset = self.offset_s;
        self.offset_s += len_s;
        // Skip near-silent chunks: Whisper hallucinates on silence and it costs ~10 s of GPU.
        if audio::rms(mic).max(audio::rms(sys)) < 0.003 {
            return;
        }
        if self.asr.is_none() {
            self.start_asr();
        }
        let Some((_, client)) = &self.asr else { return };
        self.set(Kind::Whisper, ModelState::Busy);
        let prompt = tail_chars(&self.segments.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" "), 200);
        let result = client.transcribe(audio::wav_bytes(&audio::mix(mic, sys)), &self.cfg.language, &prompt);
        self.set(Kind::Whisper, ModelState::Ready);
        match result {
            Ok(raw) => {
                for r in raw {
                    let seg = Segment {
                        start: offset + r.start,
                        end: offset + r.end,
                        track: audio::attribute(mic, sys, r.start, r.end),
                        words: r
                            .words
                            .into_iter()
                            .map(|w| Word { start: offset + w.start, end: offset + w.end, ..w })
                            .collect(),
                        text: r.text,
                    };
                    if let Err(e) = self.store.append_segment(&self.meeting.id, &seg) {
                        self.error(format!("saving transcript: {e:#}"));
                    }
                    self.segments.push(seg.clone());
                    (self.emit)(SessionEvent::Segment { segment: seg });
                }
            }
            Err(e) => self.error(format!("transcription failed for {:.0}–{:.0} s: {e:#}", offset, offset + len_s)),
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
        if let Some((server, _)) = self.asr.take() {
            server.stop();
        }
        self.set(Kind::Whisper, ModelState::Off);
        self.set(Kind::Llm, ModelState::Loading);
        let result = ModelServer::start(Kind::Llm, &self.cfg).and_then(|server| {
            self.set(Kind::Llm, ModelState::Busy);
            let text = prompts::transcript_text(&self.segments, &self.cfg.language);
            let prompt = match kind {
                SummaryKind::Partial => prompts::partial(&text, &self.cfg.language),
                SummaryKind::Final => prompts::final_summary(&text, &self.cfg.language),
            };
            let out = LlmClient::new(&server.base_url)?.complete(&prompt, if kind == SummaryKind::Final { 700 } else { 350 });
            server.stop();
            out
        });
        self.set(Kind::Llm, ModelState::Off);
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
        if kind == SummaryKind::Partial {
            self.start_asr();
        }
    }

    fn start_asr(&mut self) {
        self.set(Kind::Whisper, ModelState::Loading);
        match ModelServer::start(Kind::Whisper, &self.cfg).and_then(|s| {
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

fn tail_chars(s: &str, n: usize) -> String {
    let count = s.chars().count();
    s.chars().skip(count.saturating_sub(n)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_is_char_safe() {
        assert_eq!(tail_chars("ação", 2), "ão");
        assert_eq!(tail_chars("ab", 10), "ab");
    }

    #[test]
    fn events_serialize_with_type_tag() {
        let e = SessionEvent::Step { step: "saving".into() };
        assert_eq!(serde_json::to_string(&e).unwrap(), r#"{"type":"step","step":"saving"}"#);
    }
}
