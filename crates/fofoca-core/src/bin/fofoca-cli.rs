//! Headless driver for fo-foca's core — used for testing and benchmarking without the UI.
//!
//! ```text
//! fofoca-cli config                         show config path and values
//! fofoca-cli transcribe <file.wav>          transcribe a 16 kHz mono WAV in chunks
//! fofoca-cli record --seconds N [--title T] [--summary-at S] [--no-summary | --discard]
//! fofoca-cli summarize <meeting-id>         write a final summary for a stored meeting
//! fofoca-cli list                           list stored meetings
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
use fofoca_core::{Config, Session, SessionEvent, Store};

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
        Some("record") => record(cfg, store, &args[1..]),
        Some("summarize") => summarize(&cfg, &store, args.get(1).context("usage: summarize <meeting-id>")?),
        Some("list") => {
            for m in store.list()? {
                println!("{}  {:>6}  {}{}", m.id, mmss(m.duration_s), m.title, if m.finished { "" } else { "  (unfinished)" });
            }
            Ok(())
        }
        _ => {
            eprintln!("usage: fofoca-cli <config|transcribe|record|summarize|list> …");
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
    let session = Session::start(cfg, store, &title, sink)?;
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

fn summarize(cfg: &Config, store: &Store, id: &str) -> Result<()> {
    let meeting = store.load(id)?;
    if meeting.segments.is_empty() {
        bail!("meeting {id} has no transcript");
    }
    let t0 = Instant::now();
    let server = ModelServer::start(Kind::Llm, cfg)?;
    eprintln!("llm loaded in {:.1}s", t0.elapsed().as_secs_f64());
    let text = prompts::transcript_text(&meeting.segments, &cfg.language);
    let c = LlmClient::new(&server.base_url)?.complete(&prompts::final_summary(&text, &cfg.language), 700)?;
    println!("{}", c.text);
    store.add_summary(
        id,
        &Summary { kind: SummaryKind::Final, at_s: meeting.meta.duration_s, text: c.text, model: cfg.llm_name(), gen_tokens_per_s: c.gen_tokens_per_s },
    )?;
    eprintln!("done in {:.1}s ({:.1} tok/s)", t0.elapsed().as_secs_f64(), c.gen_tokens_per_s);
    Ok(())
}

fn toml_pretty(cfg: &Config) -> Result<String> {
    Ok(toml::to_string_pretty(cfg)?)
}
