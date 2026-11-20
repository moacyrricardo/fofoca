//! Headless driver for fo-foca's core — used for testing and benchmarking without the UI.
//!
//! ```text
//! fofoca-cli config                         show config path and values
//! fofoca-cli transcribe <file.wav>          transcribe a 16 kHz mono WAV in chunks
//! fofoca-cli bench-latency <file.wav> [--min S] [--ref ref.txt]
//!                                           replay a WAV in simulated real time through the live
//!                                           chunker; print per-line latency, backlog and WER
//! fofoca-cli record --seconds N [--title T] [--label L]… [--summary-at S] [--no-summary | --discard]
//! fofoca-cli summarize <meeting-id>         write a final summary for a stored meeting
//! fofoca-cli diarize <meeting-id>           (re-)run speaker separation on the call track
//! fofoca-cli notes <meeting-id>             (re-)write grounded notes for a stored meeting
//! fofoca-cli eval-notes <file.wav> <facts.json>
//!                                           compare free-text summary vs grounded notes
//! fofoca-cli diarize-wav <file.wav> [--threshold T] [--max N]   print speaker turns of a WAV
//! fofoca-cli list [--label L]               list stored meetings (optionally only those labelled L)
//! fofoca-cli labels                         known labels with meeting counts
//! fofoca-cli label <meeting-id> add|rm <label>
//! fofoca-cli calendar <list|add LINK|PATH|remove ID|upcoming [DAYS]|labels ID [LABEL…]>
//! ```

use anyhow::{bail, Context, Result};
use std::sync::Arc;
use std::time::{Duration, Instant};

use fofoca_core::audio::{self, SAMPLE_RATE};
use fofoca_core::config::config_path;
use fofoca_core::llm::LlmClient;
use fofoca_core::model::{Summary, SummaryKind};
use fofoca_core::prompts::{self, mmss};
use fofoca_core::servers::{Kind, ModelServer};
use fofoca_core::whisper::WhisperClient;
use fofoca_core::{Config, Session, SessionEvent, StartInfo, Store};

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cfg = Config::load_or_create()?;
    let store = Store::new(cfg.meetings_dir())?;
    match args.first().map(String::as_str) {
        Some("config") => {
            println!("# {}\n{}", config_path().display(), toml_pretty(&cfg)?);
            Ok(())
        }
        Some("transcribe") => transcribe(&cfg, args.get(1).context("usage: transcribe <file.wav>")?),
        Some("bench-latency") => bench_latency(&cfg, &args[1..]),
        Some("diarize-wav") => diarize_wav(&cfg, &args[1..]),
        Some("notes") => write_notes(&cfg, &store, args.get(1).context("usage: notes <meeting-id>")?),
        Some("eval-notes") => eval_notes(&cfg, &args[1..]),
        Some("diarize") => diarize_meeting(&cfg, &store, args.get(1).context("usage: diarize <meeting-id>")?),
        Some("record") => record(cfg, store, &args[1..]),
        Some("summarize") => summarize(&cfg, &store, args.get(1).context("usage: summarize <meeting-id>")?),
        Some("calendar") => calendar(&args[1..]),
        Some("list") => {
            let label = args.iter().position(|a| a == "--label").map(|i| args.get(i + 1).context("usage: list --label <label>")).transpose()?;
            for m in store.list(label.map(String::as_str))? {
                let labels = if m.labels.is_empty() { String::new() } else { format!("  [{}]", m.labels.join(", ")) };
                println!("{}  {:>6}  {}{}{}", m.id, mmss(m.duration_s), m.title, labels, if m.finished { "" } else { "  (unfinished)" });
            }
            Ok(())
        }
        Some("labels") => {
            for (l, n) in store.known_labels()? {
                println!("{n:>4}  {l}");
            }
            Ok(())
        }
        Some("label") => label(&store, &args[1..]),
        _ => {
            eprintln!("usage: fofoca-cli <config|transcribe|record|summarize|list|labels|label|calendar> …");
            Ok(())
        }
    }
}

fn transcribe(cfg: &Config, path: &str) -> Result<()> {
    let samples = audio::read_wav(path.as_ref())?;
    let t0 = Instant::now();
    let server = ModelServer::start(Kind::Whisper, cfg)?;
    eprintln!("whisper loaded in {:.1}s", t0.elapsed().as_secs_f64());
    let client = WhisperClient::new(&server.base_url)?;
    let chunk = (cfg.chunk_seconds * SAMPLE_RATE) as usize;
    let t1 = Instant::now();
    let mut prompt = String::new();
    let mut pos = 0;
    while pos < samples.len() {
        let rest = &samples[pos..];
        let n = audio::quiet_cut(&rest[..rest.len().min(chunk + 1)], &[], chunk, 3 * SAMPLE_RATE as usize);
        let offset = pos as f64 / SAMPLE_RATE as f64;
        for s in client.transcribe(audio::wav_bytes(&rest[..n]), &cfg.language, &prompt)? {
            println!("[{}] {}", mmss(offset + s.start), s.text);
            prompt = s.text;
        }
        pos += n;
    }
    let audio_s = samples.len() as f64 / SAMPLE_RATE as f64;
    let took = t1.elapsed().as_secs_f64();
    eprintln!("transcribed {audio_s:.1}s of audio in {took:.1}s ({:.2}x real time)", took / audio_s);
    Ok(())
}

/// (Re-)run speaker separation for a stored meeting and print the named transcript.
fn diarize_meeting(cfg: &Config, store: &Store, id: &str) -> Result<()> {
    match fofoca_core::pipeline::separate_speakers(store, cfg, id)? {
        fofoca_core::pipeline::StepResult::Done { speakers, took_s } => {
            let m = store.load(id)?;
            println!("{}", prompts::transcript_text(&m.segments, &m.speakers, &cfg.language));
            eprintln!("{speakers} voices on the call, {took_s:.1}s");
        }
        fofoca_core::pipeline::StepResult::Skipped(msg) => eprintln!("{msg}"),
    }
    Ok(())
}

/// Run speaker separation on a 16 kHz mono WAV and print the turns (for tuning, spec 003).
fn diarize_wav(cfg: &Config, args: &[String]) -> Result<()> {
    let path = args.first().context("usage: diarize-wav <file.wav> [--threshold T] [--max N]")?;
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let opts = fofoca_core::diarize::Options {
        threshold: flag("--threshold").map(|s| s.parse()).transpose()?.unwrap_or(cfg.diarization.threshold),
        max_speakers: flag("--max").map(|s| s.parse()).transpose()?.unwrap_or(8),
    };
    let samples = audio::read_wav(path.as_ref())?;
    let t0 = Instant::now();
    let turns = fofoca_core::diarize::separate(&samples, &cfg.diarization.segmentation_model, &cfg.diarization.embedding_model, &opts)?;
    let took = t0.elapsed().as_secs_f64();
    for t in &turns {
        println!("{:7.2} – {:7.2}  speaker {}", t.start, t.end, t.speaker);
    }
    let speakers: std::collections::BTreeSet<_> = turns.iter().map(|t| t.speaker).collect();
    let audio_s = samples.len() as f64 / SAMPLE_RATE as f64;
    eprintln!("{} turns, {} speakers, {took:.1}s for {audio_s:.1}s of audio ({:.3}x)", turns.len(), speakers.len(), took / audio_s);
    Ok(())
}

/// Spec 010 measurement. Time is simulated: audio "arrives" at real speed while each Whisper
/// request (real, on the configured device) takes as long as it takes, so the numbers match a live
/// session without needing a call. `--min 0` = fixed chunks only.
fn bench_latency(cfg: &Config, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: bench-latency <file.wav> [--min SECONDS] [--ref reference.txt]";
    let path = args.first().filter(|a| !a.starts_with("--")).context(USAGE)?;
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let min_s: u32 = flag("--min").map(|s| s.parse()).transpose()?.unwrap_or(cfg.min_chunk_seconds);
    let samples = audio::read_wav(path.as_ref())?;
    let sr = SAMPLE_RATE as usize;
    let (chunk, min) = ((cfg.chunk_seconds * SAMPLE_RATE) as usize, (min_s * SAMPLE_RATE) as usize);
    let server = ModelServer::start(Kind::Whisper, cfg)?;
    let client = WhisperClient::new(&server.base_url)?;
    let (mut t, mut pos, mut prompt) = (0.0f64, 0usize, String::new());
    let (mut latencies, mut text, mut requests, mut busy) = (Vec::new(), Vec::new(), 0, 0.0);
    let mut checked_at = 0usize;
    while pos < samples.len() {
        let end = ((t * sr as f64) as usize).min(samples.len());
        let buf = &samples[pos..end];
        let n = if buf.len() >= chunk {
            audio::quiet_cut(&buf[..buf.len().min(chunk + 1)], &[], chunk, 3 * sr)
        } else if end == samples.len() {
            buf.len()
        } else if min > 0 && buf.len() >= min && buf.len() >= checked_at + sr / 2 {
            checked_at = buf.len();
            audio::pause_cut(buf, &[], min, chunk).unwrap_or(0)
        } else {
            0
        };
        if n == 0 {
            t += 0.1;
            continue;
        }
        checked_at = 0;
        let offset = pos as f64 / sr as f64;
        let t0 = Instant::now();
        let segs = client.transcribe(audio::wav_bytes(&buf[..n]), &cfg.language, &prompt)?;
        let took = t0.elapsed().as_secs_f64();
        t += took;
        busy += took;
        requests += 1;
        for s in segs {
            let lat = t - (offset + s.end);
            println!("[{}] +{lat:4.1}s  {}", mmss(offset + s.start), s.text);
            latencies.push(lat);
            text.push(s.text.clone());
            prompt = s.text;
        }
        eprintln!("  chunk {:5.1}s at {}  whisper {took:4.1}s  backlog {:4.1}s", n as f64 / sr as f64, mmss(offset), (end - pos - n) as f64 / sr as f64);
        pos += n;
    }
    latencies.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| latencies.get(((latencies.len() as f64 - 1.0) * p).round() as usize).copied().unwrap_or(0.0);
    let audio_s = samples.len() as f64 / sr as f64;
    eprintln!(
        "min_chunk {min_s}s: {requests} requests, whisper busy {busy:.1}s for {audio_s:.1}s of audio; line latency median {:.1}s, p90 {:.1}s, max {:.1}s; done {:.1}s after the audio ended",
        pct(0.5), pct(0.9), pct(1.0), t - audio_s
    );
    if let Some(r) = flag("--ref") {
        eprintln!("WER {:.1}%", 100.0 * wer(&std::fs::read_to_string(r)?, &text.join(" ")));
    }
    Ok(())
}

/// Word error rate after lower-casing and dropping punctuation (good for comparing settings, not
/// for absolute numbers: digits vs spelled-out numbers count as errors).
fn wer(reference: &str, hypothesis: &str) -> f64 {
    let words = |s: &str| -> Vec<String> {
        s.split_whitespace()
            .map(|w| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase())
            .filter(|w| !w.is_empty())
            .collect()
    };
    let (r, h) = (words(reference), words(hypothesis));
    let mut prev: Vec<usize> = (0..=h.len()).collect();
    for (i, rw) in r.iter().enumerate() {
        let mut cur = vec![i + 1; h.len() + 1];
        for (j, hw) in h.iter().enumerate() {
            cur[j + 1] = (prev[j] + usize::from(rw != hw)).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[h.len()] as f64 / r.len().max(1) as f64
}

fn record(cfg: Config, store: Store, args: &[String]) -> Result<()> {
    let flag = |name: &str| args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned();
    let seconds: u64 = flag("--seconds").unwrap_or_else(|| "60".into()).parse()?;
    let summary_at: Option<u64> = flag("--summary-at").map(|s| s.parse()).transpose()?;
    let title = flag("--title").unwrap_or_default();
    let sink = Arc::new(|e: SessionEvent| match &e {
        SessionEvent::Status { .. } => {}
        SessionEvent::Segment { segment } => println!("[{}] {:?}: {}", mmss(segment.start), segment.track, segment.text),
        SessionEvent::Summary { summary } => println!("--- {:?} summary ({:.1} tok/s) ---\n{}\n---", summary.kind, summary.gen_tokens_per_s, summary.text),
        other => eprintln!("{}", serde_json::to_string(other).unwrap_or_default()),
    });
    let labels: Vec<String> = args.windows(2).filter(|w| w[0] == "--label").map(|w| w[1].clone()).collect();
    let session = Session::start(cfg, store, &StartInfo { title, labels, ..Default::default() }, sink)?;
    let t0 = Instant::now();
    let mut asked = false;
    while t0.elapsed() < Duration::from_secs(seconds) {
        if let Some(at) = summary_at {
            if !asked && t0.elapsed() >= Duration::from_secs(at) {
                session.summarize();
                asked = true;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    if args.iter().any(|a| a == "--discard") {
        session.discard();
    } else {
        session.stop(!args.iter().any(|a| a == "--no-summary"));
    }
    session.wait();
    Ok(())
}

fn label(store: &Store, args: &[String]) -> Result<()> {
    const USAGE: &str = "usage: label <meeting-id> add|rm <label>";
    let (id, op, l) = match args {
        [id, op, l, ..] => (id, op.as_str(), l),
        _ => bail!(USAGE),
    };
    let mut labels = store.load(id)?.meta.labels;
    let n = fofoca_core::labels::normalize(l).context("empty label")?;
    match op {
        "add" => labels.push(n),
        "rm" => labels.retain(|x| *x != n),
        _ => bail!(USAGE),
    }
    let m = store.set_labels(id, &labels)?;
    println!("{}: {}", m.id, m.labels.join(", "));
    Ok(())
}

fn write_notes(cfg: &Config, store: &Store, id: &str) -> Result<()> {
    let m = store.load(id)?;
    if m.segments.is_empty() {
        bail!("meeting {id} has no transcript");
    }
    let server = ModelServer::start(Kind::Llm, cfg)?;
    let n = fofoca_core::notes::extract(&LlmClient::new(&server.base_url)?, &m.segments, &m.speakers, &m.meta, cfg)?;
    println!("{}", fofoca_core::notes::to_markdown(&n, &cfg.language));
    for f in &n.flagged {
        eprintln!("couldn't verify ({}): {} — {}", f.section, f.text, f.reason);
    }
    store.save_notes(id, &n)?;
    fofoca_core::pipeline::mark_done(store, id, fofoca_core::pipeline::STEP_NOTES)?;
    Ok(())
}

/// Spec 005 evaluation: transcribe a recording, then write a free-text summary and grounded notes
/// with the same model and count (a) numbers the transcript doesn't contain and (b) reference
/// facts covered.
fn eval_notes(cfg: &Config, args: &[String]) -> Result<()> {
    use fofoca_core::grounding::numbers_in;
    const USAGE: &str = "usage: eval-notes <file.wav> <facts.json>";
    let (wav, facts_path) = (args.first().context(USAGE)?, args.get(1).context(USAGE)?);
    let facts: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(facts_path)?)?;
    let facts = facts["facts"].as_array().context("facts.json needs a \"facts\" array")?.clone();

    let segments = transcribe_segments(cfg, &audio::read_wav(wav.as_ref())?)?;
    let meta = fofoca_core::MeetingMeta {
        id: "eval".into(),
        title: "eval".into(),
        started_at: chrono::Local::now().to_rfc3339(),
        duration_s: 0.0,
        finished: true,
        attendees: vec!["Rodrigo".into(), "Fernanda".into()],
        labels: vec![],
        calendar_uid: None,
        pauses: vec![],
        steps_done: vec![],
    };
    let speakers = Default::default();
    let transcript = prompts::transcript_text(&segments, &speakers, &cfg.language);
    let said = numbers_in(&transcript);

    let server = ModelServer::start(Kind::Llm, cfg)?;
    let client = LlmClient::new(&server.base_url)?;
    let t0 = Instant::now();
    let free = client.complete(&prompts::final_summary(&transcript, &cfg.language), 700)?.text;
    let t_free = t0.elapsed().as_secs_f64();
    let t1 = Instant::now();
    let notes = fofoca_core::notes::extract(&client, &segments, &speakers, &meta, cfg)?;
    let t_notes = t1.elapsed().as_secs_f64();
    drop(server);

    let report = |name: &str, lines: Vec<String>, secs: f64| {
        let invented: Vec<String> = lines
            .iter()
            .flat_map(|l| numbers_in(l))
            .filter(|n| !said.iter().any(|s| (s - n).abs() < 1e-6))
            .map(|n| n.to_string())
            .collect();
        let covered = facts
            .iter()
            .filter(|f| {
                let nums: Vec<f64> = f["numbers"].as_array().map(|a| a.iter().filter_map(|x| x.as_f64()).collect()).unwrap_or_default();
                let keys: Vec<String> = f["keywords"].as_array().map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_lowercase)).collect()).unwrap_or_default();
                lines.iter().any(|l| {
                    let low = l.to_lowercase();
                    let ln = numbers_in(l);
                    keys.iter().any(|k| low.contains(k.as_str())) && nums.iter().all(|n| ln.iter().any(|x| (x - n).abs() < 1e-6))
                })
            })
            .count();
        println!("== {name} ({secs:.1}s)\n{}\n", lines.join("\n"));
        eprintln!("{name}: {} lines, numbers not in the transcript: {} {:?}, facts covered {covered}/{}", lines.len(), invented.len(), invented, facts.len());
    };
    report("free-text summary", free.lines().filter(|l| !l.trim().is_empty()).map(str::to_string).collect(), t_free);
    let mut kept: Vec<String> = notes.summary.iter().chain(&notes.decisions).map(|i| i.text.clone()).collect();
    kept.extend(notes.action_items.iter().map(|a| a.text.clone()));
    report("grounded notes (kept)", kept, t_notes);
    for f in &notes.flagged {
        eprintln!("  flagged ({}): {} — {}", f.section, f.text, f.reason);
    }
    Ok(())
}

/// Transcribe a whole 16 kHz mono WAV like a live session would (quiet-point chunks, VAD).
fn transcribe_segments(cfg: &Config, samples: &[i16]) -> Result<Vec<fofoca_core::Segment>> {
    let server = ModelServer::start(Kind::Whisper, cfg)?;
    let client = WhisperClient::new(&server.base_url)?;
    let chunk = (cfg.chunk_seconds * SAMPLE_RATE) as usize;
    let (mut pos, mut prompt, mut out) = (0, String::new(), Vec::new());
    while pos < samples.len() {
        let rest = &samples[pos..];
        let n = audio::quiet_cut(&rest[..rest.len().min(chunk + 1)], &[], chunk, 3 * SAMPLE_RATE as usize);
        let offset = pos as f64 / SAMPLE_RATE as f64;
        for s in client.transcribe(audio::wav_bytes(&rest[..n]), &cfg.language, &prompt)? {
            prompt = s.text.clone();
            out.push(fofoca_core::Segment {
                start: offset + s.start,
                end: offset + s.end,
                words: s.words.into_iter().map(|w| fofoca_core::Word { start: offset + w.start, end: offset + w.end, ..w }).collect(),
                text: s.text,
                track: fofoca_core::Track::Call,
                speaker: None,
            });
        }
        pos += n;
    }
    Ok(out)
}

fn summarize(cfg: &Config, store: &Store, id: &str) -> Result<()> {
    let meeting = store.load(id)?;
    if meeting.segments.is_empty() {
        bail!("meeting {id} has no transcript");
    }
    let t0 = Instant::now();
    let server = ModelServer::start(Kind::Llm, cfg)?;
    eprintln!("llm loaded in {:.1}s", t0.elapsed().as_secs_f64());
    let text = prompts::transcript_text(&meeting.segments, &meeting.speakers, &cfg.language);
    let c = LlmClient::new(&server.base_url)?.complete(&prompts::final_summary(&text, &cfg.language), 700)?;
    println!("{}", c.text);
    store.add_summary(
        id,
        &Summary { kind: SummaryKind::Final, at_s: meeting.meta.duration_s, text: c.text, model: cfg.llm_name(), gen_tokens_per_s: c.gen_tokens_per_s },
    )?;
    eprintln!("done in {:.1}s ({:.1} tok/s)", t0.elapsed().as_secs_f64(), c.gen_tokens_per_s);
    Ok(())
}

fn calendar(args: &[String]) -> Result<()> {
    use fofoca_core::calendar::{Calendars, Keyring};
    let cals = Calendars::new(fofoca_core::config::calendars_path(), Keyring)?;
    match args.first().map(String::as_str) {
        Some("add") => {
            let s = cals.add(args.get(1).context("usage: calendar add <link|path>")?)?;
            println!("added {} ({}, {})", s.label, s.id, s.location);
        }
        Some("remove") => cals.remove(args.get(1).context("usage: calendar remove <id>")?)?,
        Some("labels") => {
            let id = args.get(1).context("usage: calendar labels <id> [label…]")?;
            let s = cals.set_default_labels(id, &args[2..])?;
            if let Some(src) = s.sources.iter().find(|x| x.id == *id) {
                println!("{}: {}", src.label, src.default_labels.join(", "));
            }
        }
        Some("upcoming") => {
            let days: i64 = args.get(1).map(|d| d.parse()).transpose()?.unwrap_or(7);
            let now = chrono::Utc::now();
            let (events, errors) = cals.upcoming(now, now + chrono::Duration::days(days));
            for e in events {
                println!("{}  {}  [{}]  {}  {}", &e.start[..16], e.title, e.calendar, e.call_link.unwrap_or_default(), e.attendees.join(", "));
            }
            for e in errors {
                eprintln!("error: {e}");
            }
        }
        _ => {
            let s = cals.settings();
            for src in &s.sources {
                println!("{}  {:?}  {}  {}  [{}]", src.id, src.kind, src.label, src.location, src.default_labels.join(", "));
            }
            println!("only events with a call link: {}", s.only_with_call_link);
        }
    }
    Ok(())
}

fn toml_pretty(cfg: &Config) -> Result<String> {
    Ok(toml::to_string_pretty(cfg)?)
}
