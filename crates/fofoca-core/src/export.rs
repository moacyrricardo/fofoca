//! Export a meeting to a folder (spec 023): `<dest>/<YYYY-MM-DD Title>/` with the transcript
//! (Markdown and/or SRT), the notes, the tasks and the audio, plus an optional zip next to it.
//!
//! The renderers are pure functions of a loaded [`Meeting`] and the transcription language
//! (decision 1): the files are written in the language the meeting was transcribed in.

use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::config::{collapse_home, Config};
use crate::grounding::Action;
use crate::model::{Meeting, MeetingMeta, Pause, Segment, SummaryKind, Word};
use crate::store::Store;
use crate::speakers::display_name;
use crate::whisper::approx_words;

/// A cue longer than this many characters (two subtitle lines) is split (decision 7).
const CUE_CHARS: usize = 84;
/// A cue longer than this many seconds is split (decision 7).
const CUE_SECONDS: f64 = 7.0;
/// One subtitle line.
const LINE_CHARS: usize = 42;
/// Longest title part of a folder name, in bytes.
const TITLE_BYTES: usize = 200;

// ------------------------------------------------------------------ names

/// `YYYY-MM-DD Title`: the date in the meeting's own offset, the title made safe for a file name.
pub fn folder_name(meta: &MeetingMeta) -> String {
    let date = chrono::DateTime::parse_from_rfc3339(&meta.started_at)
        .map(|d| d.format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| meta.started_at.chars().take(10).collect());
    let title = safe_name(&meta.title, TITLE_BYTES);
    format!("{date} {}", if title.is_empty() { "Meeting" } else { &title })
}

/// A file-name-safe version of `s`: `/ \ : * ? " < > |` become `-`, control characters become
/// spaces, leading/trailing dots and spaces go, accents stay. Capped at `max_bytes` on a character
/// boundary. May be empty.
pub fn safe_name(s: &str, max_bytes: usize) -> String {
    let replaced: String = s
        .chars()
        .map(|c| if r#"/\:*?"<>|"#.contains(c) { '-' } else if c.is_control() { ' ' } else { c })
        .collect();
    let trim = |t: &str| t.trim_matches(|c: char| c == '.' || c.is_whitespace()).to_string();
    let trimmed = trim(&replaced);
    let mut end = 0;
    for (i, c) in trimmed.char_indices() {
        if i + c.len_utf8() > max_bytes {
            break;
        }
        end = i + c.len_utf8();
    }
    trim(&trimmed[..end])
}

// ------------------------------------------------------------------ shared pieces

/// `mm:ss` under an hour, `h:mm:ss` from there on (decision 6).
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0) as u64;
    if s < 3600 {
        format!("{:02}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    }
}

/// A YAML double-quoted string (JSON strings are valid YAML).
fn quoted(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".into())
}

/// YAML front matter (decision 5) and the `# Title` heading every `.md` file starts with.
pub fn front_matter(meta: &MeetingMeta, model: Option<&str>) -> String {
    let list = |v: &[String]| format!("[{}]", v.iter().map(|s| quoted(s)).collect::<Vec<_>>().join(", "));
    let date = chrono::DateTime::parse_from_rfc3339(&meta.started_at)
        .map(|d| d.to_rfc3339_opts(chrono::SecondsFormat::Secs, false))
        .unwrap_or_else(|_| meta.started_at.clone());
    let mut out = format!(
        "---\ntitle: {}\ndate: {date}\nduration: {}\nattendees: {}\nlabels: {}\n",
        quoted(&meta.title),
        quoted(&clock(meta.duration_s)),
        list(&meta.attendees),
        list(&meta.labels),
    );
    if let Some(model) = model {
        out.push_str(&format!("model: {}\n", quoted(model)));
    }
    out.push_str(&format!("---\n\n# {}\n\n", meta.title.trim()));
    out
}

fn refs(times: &[f64]) -> String {
    times.iter().map(|t| format!(" [{}]", clock(*t))).collect()
}

/// `text — owner · due [time]`, the owner and due date only when known.
fn action_line(a: &Action) -> String {
    let who = [a.owner.as_deref(), a.due.as_deref()].into_iter().flatten().collect::<Vec<_>>().join(" · ");
    format!("{}{}{}", a.text, if who.is_empty() { String::new() } else { format!(" — {who}") }, refs(&a.refs))
}

// ------------------------------------------------------------------ transcript.md

fn pause_line(p: &Pause, pt: bool) -> String {
    let parse = |s: &str| chrono::DateTime::parse_from_rfc3339(s).ok();
    let secs = match (parse(&p.from), p.to.as_deref().and_then(parse)) {
        (Some(a), Some(b)) => Some((b - a).num_seconds().max(0)),
        _ => None,
    };
    match (secs, pt) {
        (Some(s), true) => format!("— pausa de {} min —\n\n", ((s as f64) / 60.0).round().max(1.0)),
        (Some(s), false) => format!("— paused {} min —\n\n", ((s as f64) / 60.0).round().max(1.0)),
        (None, true) => "— pausa —\n\n".into(),
        (None, false) => "— paused —\n\n".into(),
    }
}

/// One `**[time] Speaker:** text` line per segment, with a line for each recording pause.
pub fn transcript_md(m: &Meeting, language: &str) -> String {
    let pt = language == "pt";
    let mut out = front_matter(&m.meta, None);
    let mut prev = f64::NEG_INFINITY;
    for seg in &m.segments {
        for p in m.meta.pauses.iter().filter(|p| p.at_s > prev && p.at_s <= seg.start) {
            out.push_str(&pause_line(p, pt));
        }
        out.push_str(&format!("**[{}] {}:** {}\n\n", clock(seg.start), display_name(seg, &m.speakers, language), seg.text.trim()));
        prev = seg.start;
    }
    for p in m.meta.pauses.iter().filter(|p| p.at_s > prev) {
        out.push_str(&pause_line(p, pt));
    }
    format!("{}\n", out.trim_end())
}

// ------------------------------------------------------------------ transcript.srt

/// `HH:MM:SS,mmm`.
pub fn srt_time(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn words_text(words: &[&Word]) -> String {
    words.iter().map(|w| w.text.as_str()).collect::<String>().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A segment as one or more `(start, end, text)` cues, split at its word timings when it is longer
/// than two subtitle lines or `CUE_SECONDS`.
fn cues(seg: &Segment) -> Vec<(f64, f64, String)> {
    let text = seg.text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= CUE_CHARS && seg.end - seg.start <= CUE_SECONDS {
        return vec![(seg.start, seg.end, text)];
    }
    let words = if seg.words.is_empty() { approx_words(seg.start, seg.end, &text) } else { seg.words.clone() };
    let mut out = Vec::new();
    let mut cur: Vec<&Word> = Vec::new();
    for w in &words {
        if let Some(first) = cur.first() {
            let mut next = cur.clone();
            next.push(w);
            if words_text(&next).chars().count() > CUE_CHARS || w.end - first.start > CUE_SECONDS {
                out.push((first.start, cur[cur.len() - 1].end, words_text(&cur)));
                cur.clear();
            }
        }
        cur.push(w);
    }
    if let Some(first) = cur.first() {
        out.push((first.start, cur[cur.len() - 1].end, words_text(&cur)));
    }
    if out.is_empty() {
        out.push((seg.start, seg.end, text));
    }
    out
}

/// Break a cue into two lines at the space nearest the middle when it is longer than one line.
fn two_lines(text: &str) -> String {
    if text.chars().count() <= LINE_CHARS {
        return text.to_string();
    }
    let mid = text.len() / 2;
    match text.char_indices().filter(|(_, c)| *c == ' ').min_by_key(|(i, _)| i.abs_diff(mid)) {
        Some((i, _)) => format!("{}\n{}", &text[..i], &text[i + 1..]),
        None => text.to_string(),
    }
}

/// Numbered SRT cues, each starting with `Speaker: `. Overlapping You/Call cues are kept as they
/// are (decision 7).
pub fn transcript_srt(m: &Meeting, language: &str) -> String {
    let mut out = String::new();
    let mut n = 0;
    for seg in &m.segments {
        let who = display_name(seg, &m.speakers, language);
        for (start, end, text) in cues(seg) {
            n += 1;
            out.push_str(&format!("{n}\n{} --> {}\n{}\n\n", srt_time(start), srt_time(end.max(start)), two_lines(&format!("{who}: {text}"))));
        }
    }
    out
}

// ------------------------------------------------------------------ notes.md and tasks.md

/// Where notes.md comes from (decision 2).
#[derive(serde::Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NotesSource {
    /// The checked notes (`notes.json`).
    Checked,
    /// The latest final summary's text.
    Summary,
}

pub fn notes_source(m: &Meeting) -> Option<NotesSource> {
    if m.notes.is_some() {
        Some(NotesSource::Checked)
    } else if m.summaries.iter().any(|s| s.kind == SummaryKind::Final) {
        Some(NotesSource::Summary)
    } else {
        None
    }
}

/// The checked notes with their times, ending with the items that couldn't be verified; or the
/// latest final summary when there are no checked notes. `None` when there is neither. Partial
/// summaries are never exported.
pub fn notes_md(m: &Meeting, language: &str) -> Option<String> {
    let pt = language == "pt";
    let Some(n) = &m.notes else {
        let s = m.summaries.iter().rfind(|s| s.kind == SummaryKind::Final)?;
        return Some(format!("{}{}\n", front_matter(&m.meta, Some(&s.model)), s.text.trim()));
    };
    let mut out = front_matter(&m.meta, Some(&n.model));
    let mut section = |title: &str, lines: Vec<String>| {
        if !lines.is_empty() {
            out.push_str(&format!("## {title}\n\n{}\n\n", lines.iter().map(|l| format!("- {l}")).collect::<Vec<_>>().join("\n")));
        }
    };
    section(if pt { "Resumo" } else { "Summary" }, n.summary.iter().map(|i| format!("{}{}", i.text, refs(&i.refs))).collect());
    section(if pt { "Decisões" } else { "Decisions" }, n.decisions.iter().map(|i| format!("{}{}", i.text, refs(&i.refs))).collect());
    section(if pt { "Próximos passos" } else { "Next steps" }, n.action_items.iter().map(action_line).collect());
    section(
        if pt { "Não verificado" } else { "Couldn't verify" },
        n.flagged.iter().map(|f| format!("{} — {}{}", f.text, f.reason, refs(&f.refs))).collect(),
    );
    Some(format!("{}\n", out.trim_end()))
}

/// Next steps as an unchecked checklist, without transcript quotes (decision 4). `None` when the
/// meeting has no tasks.
pub fn tasks_md(m: &Meeting) -> Option<String> {
    let items = &m.notes.as_ref()?.action_items;
    if items.is_empty() {
        return None;
    }
    let lines: Vec<String> = items.iter().map(|a| format!("- [ ] {}", action_line(a))).collect();
    Some(format!("{}{}\n", front_matter(&m.meta, None), lines.join("\n")))
}

// ------------------------------------------------------------------ the export run

/// Which transcript files to write.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptFormat {
    Markdown,
    Srt,
    Both,
}

/// What to export and where. `dest` is the folder the meeting's folder is created in.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ExportOptions {
    pub transcript: Option<TranscriptFormat>,
    pub notes: bool,
    pub tasks: bool,
    pub audio: bool,
    pub zip: bool,
    pub dest: PathBuf,
}

/// The part of the meeting a file comes from.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FilePart {
    TranscriptMd,
    TranscriptSrt,
    Notes,
    Tasks,
    Audio,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PlannedFile {
    pub part: FilePart,
    /// Relative to the meeting's folder, `/`-separated (`audio/mic.wav`).
    pub path: String,
    pub bytes: u64,
}

/// What this meeting can export, whatever was picked: the dialog greys out the rest with the reason.
#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Parts {
    pub transcript_lines: usize,
    /// `None`: no checked notes and no final summary ("No summary for this meeting").
    pub notes: Option<NotesSource>,
    pub tasks: usize,
    /// Audio tracks on disk (0 when deleted, spec 011, or never kept).
    pub audio_tracks: usize,
    pub audio_deleted_at: Option<String>,
    pub imported: bool,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ExportPlan {
    pub dest: PathBuf,
    /// `dest` with `~` for the home folder.
    pub dest_display: String,
    /// The meeting's folder name, with the `(2)` suffix when that name is taken.
    pub folder_name: String,
    pub folder: PathBuf,
    pub zip: Option<PathBuf>,
    /// The files the options pick, with their sizes.
    pub files: Vec<PlannedFile>,
    pub parts: Parts,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ExportStep {
    Writing,
    Copying,
    Zipping,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ExportProgress {
    pub step: ExportStep,
    /// The file being written (relative), or the zip's name.
    pub file: String,
    /// Bytes written so far and in all (the zip counts its files again).
    pub done: u64,
    pub total: u64,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct ExportDone {
    pub folder: PathBuf,
    pub zip: Option<PathBuf>,
    pub files: usize,
    pub bytes: u64,
}

/// The export was cancelled (decision 15); everything it wrote is gone.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("export cancelled")
    }
}

impl std::error::Error for Cancelled {}

pub fn is_cancelled(e: &anyhow::Error) -> bool {
    e.downcast_ref::<Cancelled>().is_some()
}

enum Source {
    Text(String),
    File(PathBuf),
}

struct Entry {
    file: PlannedFile,
    source: Source,
}

/// What the meeting can export, and the entries the options pick, rendered from one snapshot.
fn entries(store: &Store, m: &Meeting, language: &str, opts: &ExportOptions) -> Result<(Vec<Entry>, Parts)> {
    let tracks = store.audio_tracks(&m.meta.id)?;
    let imported = m.meta.imported_from.is_some();
    let mut audio: Vec<(String, PathBuf)> = Vec::new();
    if m.meta.audio_deleted_at.is_none() {
        // Imports keep their single track under the original file name (decision 14).
        let stem = m.meta.imported_from.as_deref().and_then(|f| Path::new(f).file_stem()).map(|s| safe_name(&s.to_string_lossy(), TITLE_BYTES)).filter(|s| !s.is_empty());
        if let Some(p) = tracks.mic {
            audio.push(("audio/mic.wav".into(), p));
        }
        if let Some(p) = tracks.call {
            audio.push((format!("audio/{}.wav", if imported { stem.as_deref().unwrap_or("call") } else { "call" }), p));
        }
    }
    let notes = notes_md(m, language);
    let tasks = tasks_md(m);
    let parts = Parts {
        transcript_lines: m.segments.len(),
        notes: notes_source(m),
        tasks: m.notes.as_ref().map_or(0, |n| n.action_items.len()),
        audio_tracks: audio.len(),
        audio_deleted_at: m.meta.audio_deleted_at.clone(),
        imported,
    };
    let text = |part, path: &str, s: String| Entry { file: PlannedFile { part, path: path.into(), bytes: s.len() as u64 }, source: Source::Text(s) };
    let mut out = Vec::new();
    if matches!(opts.transcript, Some(TranscriptFormat::Markdown | TranscriptFormat::Both)) {
        out.push(text(FilePart::TranscriptMd, "transcript.md", transcript_md(m, language)));
    }
    if matches!(opts.transcript, Some(TranscriptFormat::Srt | TranscriptFormat::Both)) {
        out.push(text(FilePart::TranscriptSrt, "transcript.srt", transcript_srt(m, language)));
    }
    if opts.notes {
        if let Some(s) = notes {
            out.push(text(FilePart::Notes, "notes.md", s));
        }
    }
    if opts.tasks {
        if let Some(s) = tasks {
            out.push(text(FilePart::Tasks, "tasks.md", s));
        }
    }
    if opts.audio {
        for (path, src) in audio {
            let bytes = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
            out.push(Entry { file: PlannedFile { part: FilePart::Audio, path, bytes }, source: Source::File(src) });
        }
    }
    Ok((out, parts))
}

fn name_taken(dest: &Path, name: &str) -> bool {
    dest.join(name).exists() || dest.join(format!("{name}.zip")).exists()
}

/// `base`, or `base (2)`, `(3)`… when a folder or zip of that name already exists in `dest`
/// (decision 10).
pub fn unique_name(dest: &Path, base: &str) -> String {
    let mut name = base.to_string();
    let mut n = 1;
    while name_taken(dest, &name) {
        n += 1;
        name = format!("{base} ({n})");
    }
    name
}

/// What an export with these options would write, without writing anything.
pub fn plan(store: &Store, cfg: &Config, id: &str, opts: &ExportOptions) -> Result<ExportPlan> {
    let m = store.load(id)?;
    let (entries, parts) = entries(store, &m, &cfg.language, opts)?;
    let folder_name = unique_name(&opts.dest, &folder_name(&m.meta));
    Ok(ExportPlan {
        dest: opts.dest.clone(),
        dest_display: collapse_home(&opts.dest),
        folder: opts.dest.join(&folder_name),
        zip: opts.zip.then(|| opts.dest.join(format!("{folder_name}.zip"))),
        folder_name,
        files: entries.into_iter().map(|e| e.file).collect(),
        parts,
    })
}

/// Removes what an unfinished export wrote, unless disarmed.
struct Cleanup(Vec<PathBuf>);

impl Drop for Cleanup {
    fn drop(&mut self) {
        for p in &self.0 {
            if p.is_dir() {
                let _ = std::fs::remove_dir_all(p);
            } else if p.exists() {
                let _ = std::fs::remove_file(p);
            }
        }
    }
}

const CHUNK: usize = 1 << 20;

/// Copy in 1 MB chunks, checking `cancel` between them.
fn copy(from: &mut impl Read, to: &mut impl Write, cancel: &AtomicBool, mut on_chunk: impl FnMut(u64)) -> Result<u64> {
    let mut buf = vec![0u8; CHUNK];
    let mut total = 0;
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        let n = from.read(&mut buf)?;
        if n == 0 {
            return Ok(total);
        }
        to.write_all(&buf[..n])?;
        total += n as u64;
        on_chunk(n as u64);
    }
}

/// Export a meeting: render from one snapshot, open the audio files before writing anything, write
/// into a hidden temp folder in `dest`, zip from it when asked, then rename both into place. On
/// error or cancel, the temp folder and the partial zip are removed.
pub fn run(store: &Store, cfg: &Config, id: &str, opts: &ExportOptions, progress: &dyn Fn(ExportProgress), cancel: &AtomicBool) -> Result<ExportDone> {
    let m = store.load(id)?;
    let (entries, parts) = entries(store, &m, &cfg.language, opts)?;
    anyhow::ensure!(!opts.notes || parts.notes.is_some(), "this meeting has no summary or checked notes");
    anyhow::ensure!(!opts.tasks || parts.tasks > 0, "this meeting has no tasks");
    anyhow::ensure!(!opts.audio || parts.audio_tracks > 0, "this meeting's audio isn't kept");
    anyhow::ensure!(!entries.is_empty(), "nothing to export: pick at least one part");
    // Opened first: deleting the audio (spec 011) during the export can't break the copy.
    let mut handles = Vec::new();
    for e in &entries {
        handles.push(match &e.source {
            Source::File(p) => Some(File::open(p).with_context(|| format!("opening {}", p.display()))?),
            Source::Text(_) => None,
        });
    }
    std::fs::create_dir_all(&opts.dest).with_context(|| format!("creating {}", opts.dest.display()))?;
    let base = folder_name(&m.meta);
    let name = unique_name(&opts.dest, &base);
    let tmp = opts.dest.join(format!(".{name}.fofoca-export-{}", std::process::id()));
    let part = opts.dest.join(format!("{name}.zip.part"));
    let mut cleanup = Cleanup(vec![tmp.clone(), part.clone()]);
    if tmp.exists() {
        std::fs::remove_dir_all(&tmp)?;
    }
    std::fs::create_dir_all(tmp.join("audio"))?;

    let files: u64 = entries.iter().map(|e| e.file.bytes).sum();
    let total = if opts.zip { files * 2 } else { files };
    let done = std::cell::Cell::new(0u64);
    let report = |step, file: &str| progress(ExportProgress { step, file: file.into(), done: done.get(), total });
    for (e, handle) in entries.iter().zip(handles.iter_mut()) {
        if cancel.load(Ordering::Relaxed) {
            return Err(Cancelled.into());
        }
        let path = tmp.join(&e.file.path);
        match (&e.source, handle) {
            (Source::Text(s), _) => {
                report(ExportStep::Writing, &e.file.path);
                std::fs::write(&path, s).with_context(|| format!("writing {}", path.display()))?;
                done.set(done.get() + e.file.bytes);
            }
            (Source::File(_), Some(f)) => {
                report(ExportStep::Copying, &e.file.path);
                let mut out = BufWriter::new(File::create(&path).with_context(|| format!("writing {}", path.display()))?);
                let mut since = 0;
                copy(f, &mut out, cancel, |n| {
                    done.set(done.get() + n);
                    since += n;
                    if since >= 4 * CHUNK as u64 {
                        since = 0;
                        report(ExportStep::Copying, &e.file.path);
                    }
                })?;
                out.flush()?;
            }
            (Source::File(_), None) => unreachable!("audio handles are opened above"),
        }
    }
    if !entries.iter().any(|e| e.file.part == FilePart::Audio) {
        std::fs::remove_dir(tmp.join("audio"))?;
    }

    if opts.zip {
        let zip_name = format!("{name}.zip");
        report(ExportStep::Zipping, &zip_name);
        let mut zip = zip::ZipWriter::new(BufWriter::new(File::create(&part)?));
        let mut since = 0;
        for e in &entries {
            let method = if e.file.path.ends_with(".wav") { zip::CompressionMethod::Stored } else { zip::CompressionMethod::Deflated };
            let options = zip::write::SimpleFileOptions::default().compression_method(method).large_file(e.file.bytes >= u32::MAX as u64);
            zip.start_file(format!("{name}/{}", e.file.path), options)?;
            copy(&mut File::open(tmp.join(&e.file.path))?, &mut zip, cancel, |n| {
                done.set(done.get() + n);
                since += n;
                if since >= 4 * CHUNK as u64 {
                    since = 0;
                    report(ExportStep::Zipping, &zip_name);
                }
            })?;
        }
        zip.finish()?.flush()?;
    }
    if cancel.load(Ordering::Relaxed) {
        return Err(Cancelled.into());
    }

    // Something may have taken the name meanwhile: pick again, the zip follows.
    let name = if name_taken(&opts.dest, &name) { unique_name(&opts.dest, &base) } else { name };
    let folder = opts.dest.join(&name);
    std::fs::rename(&tmp, &folder).with_context(|| format!("moving the export to {}", folder.display()))?;
    let zip = if opts.zip {
        let z = opts.dest.join(format!("{name}.zip"));
        std::fs::rename(&part, &z)?;
        Some(z)
    } else {
        None
    };
    cleanup.0.clear();
    Ok(ExportDone { folder, zip, files: entries.len(), bytes: files })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grounding::{Flagged, Item, Notes};
    use crate::model::{SpeakerInfo, Summary, Track};
    use std::collections::BTreeMap;

    fn meta(title: &str) -> MeetingMeta {
        MeetingMeta {
            id: "2026-11-20-141500".into(),
            title: title.into(),
            started_at: "2026-11-20T14:15:00-03:00".into(),
            duration_s: 664.0,
            transcript_source: None,
            finished: true,
            attendees: vec!["Ana Souza".into(), "Bruno Reis".into()],
            labels: vec!["trabalho".into()],
            calendar_uid: None,
            pauses: vec![],
            steps_done: vec![],
            processed: true,
            keep_audio: None,
            audio_deleted_at: None,
            imported_from: None,
        }
    }

    fn seg(start: f64, end: f64, text: &str, track: Track, speaker: Option<&str>) -> Segment {
        Segment { start, end, text: text.into(), track, words: vec![], speaker: speaker.map(str::to_string) }
    }

    fn meeting(segments: Vec<Segment>) -> Meeting {
        let mut speakers = BTreeMap::new();
        speakers.insert("s1".to_string(), SpeakerInfo { name: Some("Ana".into()) });
        speakers.insert("s2".to_string(), SpeakerInfo::default());
        Meeting { meta: meta("Planejamento do lançamento"), segments, summaries: vec![], speakers, notes: None }
    }

    fn notes() -> Notes {
        Notes {
            summary: vec![Item { text: "O teste começa na segunda".into(), refs: vec![62.0] }],
            decisions: vec![Item { text: "Lançamento em 4 de dezembro".into(), refs: vec![134.0, 140.0] }],
            action_items: vec![
                Action { text: "Enviar o convite".into(), owner: Some("Ana".into()), due: Some("2026-11-27".into()), refs: vec![185.0] },
                Action { text: "Revisar os preços".into(), owner: None, due: None, refs: vec![358.0] },
            ],
            flagged: vec![Flagged { section: "summary".into(), text: "Custa R$ 30 mil".into(), reason: "30 isn't said in the cited lines".into(), refs: vec![300.0] }],
            model: "Qwen3.5-2B-Q4_K_M".into(),
            gen_tokens_per_s: 17.0,
        }
    }

    const HEAD: &str = "---\ntitle: \"Planejamento do lançamento\"\ndate: 2026-11-20T14:15:00-03:00\nduration: \"11:04\"\nattendees: [\"Ana Souza\", \"Bruno Reis\"]\nlabels: [\"trabalho\"]\n";

    #[test]
    fn folder_names_are_safe() {
        let name = |t: &str| folder_name(&meta(t));
        assert_eq!(name("Meeting 20/11 14:02"), "2026-11-20 Meeting 20-11 14-02");
        assert_eq!(name("1:1 Fernanda"), "2026-11-20 1-1 Fernanda");
        assert_eq!(name("Revisão — orçamento"), "2026-11-20 Revisão — orçamento");
        assert_eq!(name("  ...  "), "2026-11-20 Meeting");
        assert_eq!(name(""), "2026-11-20 Meeting");
        assert_eq!(name("a\tb\n"), "2026-11-20 a b");
        assert_eq!(name("..hidden.."), "2026-11-20 hidden");
        // 300 bytes of two-byte characters: capped at 200 bytes without splitting one
        let long = name(&"ç".repeat(150));
        assert_eq!(long, format!("2026-11-20 {}", "ç".repeat(100)));
        let odd = safe_name(&format!("a{}", "ç".repeat(150)), 200);
        assert_eq!(odd.len(), 199, "the 101st ç would cross 200 bytes");
        // the date is the meeting's own, not UTC's
        let mut late = meta("x");
        late.started_at = "2026-11-20T23:30:00-03:00".into();
        assert_eq!(folder_name(&late), "2026-11-20 x");
    }

    #[test]
    fn clocks() {
        assert_eq!(clock(65.4), "01:05");
        assert_eq!(clock(3599.0), "59:59");
        assert_eq!(clock(3600.0), "1:00:00");
        assert_eq!(clock(4510.0), "1:15:10");
        assert_eq!(srt_time(3725.0456), "01:02:05,046");
        assert_eq!(srt_time(0.0), "00:00:00,000");
    }

    #[test]
    fn transcript_md_in_pt_with_a_pause_and_past_an_hour() {
        let mut m = meeting(vec![
            seg(5.0, 9.0, " Bom dia.", Track::You, Some("you")),
            seg(12.0, 15.0, "Oi!", Track::Call, Some("s1")),
            seg(3700.0, 3705.0, "Fechado.", Track::Call, Some("s2")),
            seg(3710.0, 3712.0, "Até.", Track::Call, None),
        ]);
        m.meta.pauses = vec![Pause { at_s: 20.0, from: "2026-11-20T14:16:00-03:00".into(), to: Some("2026-11-20T14:19:10-03:00".into()) }];
        let want = format!(
            "{HEAD}---\n\n# Planejamento do lançamento\n\n\
             **[00:05] Você:** Bom dia.\n\n\
             **[00:12] Ana:** Oi!\n\n\
             — pausa de 3 min —\n\n\
             **[1:01:40] Pessoa 2:** Fechado.\n\n\
             **[1:01:50] Chamada:** Até.\n"
        );
        assert_eq!(transcript_md(&m, "pt"), want);
        let en = transcript_md(&m, "en");
        assert!(en.contains("**[00:05] You:** Bom dia.") && en.contains("— paused 3 min —") && en.contains("**[1:01:40] Person 2:**") && en.contains("**[1:01:50] Call:**"), "{en}");
    }

    #[test]
    fn a_pause_after_the_last_line_and_one_that_never_resumed() {
        let mut m = meeting(vec![seg(1.0, 2.0, "Oi", Track::You, None)]);
        m.meta.pauses = vec![Pause { at_s: 30.0, from: "2026-11-20T14:16:00-03:00".into(), to: None }];
        assert!(transcript_md(&m, "en").ends_with("**[00:01] You:** Oi\n\n— paused —\n"));
    }

    #[test]
    fn srt_splits_long_segments_at_word_timings_and_numbers_cues() {
        let w = |start: f64, text: &str| Word { start, end: start + 0.9, text: format!(" {text}"), p: 1.0 };
        let mut long = seg(10.0, 22.0, "", Track::Call, Some("s1"));
        long.words = (0..12).map(|i| w(10.0 + i as f64, &format!("palavra{i}"))).collect();
        long.text = long.words.iter().map(|w| w.text.as_str()).collect::<String>().trim().into();
        let m = meeting(vec![seg(1.0, 3.5, "Bom dia.", Track::You, Some("you")), long, seg(11.0, 12.0, "Isso.", Track::You, Some("you"))]);
        let want = "1\n00:00:01,000 --> 00:00:03,500\nYou: Bom dia.\n\n\
                    2\n00:00:10,000 --> 00:00:16,900\nAna: palavra0 palavra1 palavra2\npalavra3 palavra4 palavra5 palavra6\n\n\
                    3\n00:00:17,000 --> 00:00:21,900\nAna: palavra7 palavra8\npalavra9 palavra10 palavra11\n\n\
                    4\n00:00:11,000 --> 00:00:12,000\nYou: Isso.\n\n";
        assert_eq!(transcript_srt(&m, "en"), want);
    }

    #[test]
    fn srt_splits_by_length_without_word_timings() {
        let text = "um dois três quatro cinco seis sete oito nove dez onze doze treze catorze quinze dezesseis";
        let m = meeting(vec![seg(0.0, 6.0, text, Track::You, None)]);
        let srt = transcript_srt(&m, "pt");
        assert!(srt.starts_with("1\n00:00:00,000 --> "), "{srt}");
        assert!(srt.contains("\n2\n"), "over 84 characters: two cues\n{srt}");
        let rejoined: String = srt.lines().filter(|l| l.starts_with("Você: ") || (!l.contains("-->") && !l.is_empty() && l.parse::<u32>().is_err())).collect::<Vec<_>>().join(" ").replace("Você: ", "");
        assert_eq!(rejoined, text, "no word lost");
    }

    #[test]
    fn notes_md_renders_checked_notes_with_the_flagged_section() {
        let mut m = meeting(vec![]);
        m.notes = Some(notes());
        let want = format!(
            "{HEAD}model: \"Qwen3.5-2B-Q4_K_M\"\n---\n\n# Planejamento do lançamento\n\n\
             ## Resumo\n\n- O teste começa na segunda [01:02]\n\n\
             ## Decisões\n\n- Lançamento em 4 de dezembro [02:14] [02:20]\n\n\
             ## Próximos passos\n\n- Enviar o convite — Ana · 2026-11-27 [03:05]\n- Revisar os preços [05:58]\n\n\
             ## Não verificado\n\n- Custa R$ 30 mil — 30 isn't said in the cited lines [05:00]\n"
        );
        assert_eq!(notes_md(&m, "pt").unwrap(), want);
        let en = notes_md(&m, "en").unwrap();
        for h in ["## Summary", "## Decisions", "## Next steps", "## Couldn't verify"] {
            assert!(en.contains(h), "{h} in {en}");
        }
        assert_eq!(notes_source(&m), Some(NotesSource::Checked));
    }

    #[test]
    fn notes_md_falls_back_to_the_latest_final_summary_never_a_partial() {
        let mut m = meeting(vec![]);
        let s = |kind, text: &str| Summary { kind, at_s: 0.0, text: text.into(), model: "m".into(), gen_tokens_per_s: 0.0 };
        m.summaries = vec![s(SummaryKind::Partial, "parcial")];
        assert_eq!(notes_md(&m, "pt"), None);
        assert_eq!(notes_source(&m), None);
        m.summaries.push(s(SummaryKind::Final, "primeiro"));
        m.summaries.push(s(SummaryKind::Final, "**Resumo**\nsegundo\n"));
        m.summaries.push(s(SummaryKind::Partial, "outro parcial"));
        let want = format!("{HEAD}model: \"m\"\n---\n\n# Planejamento do lançamento\n\n**Resumo**\nsegundo\n");
        assert_eq!(notes_md(&m, "pt").unwrap(), want);
        assert_eq!(notes_source(&m), Some(NotesSource::Summary));
    }

    #[test]
    fn tasks_md_is_an_unchecked_checklist() {
        let mut m = meeting(vec![]);
        assert_eq!(tasks_md(&m), None);
        m.notes = Some(Notes { action_items: vec![], ..notes() });
        assert_eq!(tasks_md(&m), None, "checked notes without tasks");
        m.notes = Some(notes());
        let want = format!("{HEAD}---\n\n# Planejamento do lançamento\n\n- [ ] Enviar o convite — Ana · 2026-11-27 [03:05]\n- [ ] Revisar os preços [05:58]\n");
        assert_eq!(tasks_md(&m).unwrap(), want);
    }

    // ---------------------------------------------------------------- end to end, in temp dirs

    struct Fixture {
        root: PathBuf,
        store: Store,
        cfg: Config,
    }

    impl Fixture {
        fn new(name: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!("fofoca-export-{name}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            Fixture { store: Store::new(root.join("meetings")).unwrap(), root, cfg: Config::default() }
        }

        /// A finished recording with two lines, checked notes and, when asked, both audio tracks.
        fn meeting(&self, title: &str, audio: bool, imported_from: Option<&str>) -> String {
            let info = crate::model::StartInfo { title: title.into(), started_at: Some("2026-11-20T14:15:00-03:00".into()), imported_from: imported_from.map(str::to_string), ..Default::default() };
            let id = self.store.create(&info).unwrap().id;
            self.store.append_segment(&id, &seg(1.0, 3.0, "Bom dia.", Track::You, None)).unwrap();
            self.store.append_segment(&id, &seg(4.0, 6.0, "Oi!", Track::Call, None)).unwrap();
            self.store.save_notes(&id, &notes()).unwrap();
            self.store.update_meta(&id, |m| { m.finished = true; m.duration_s = 664.0 }).unwrap();
            if audio {
                if imported_from.is_none() {
                    std::fs::write(self.store.dir(&id).join("mic.wav"), wav(3 * CHUNK + 17, 1)).unwrap();
                }
                std::fs::write(self.store.dir(&id).join("system.wav"), wav(CHUNK / 2, 2)).unwrap();
            }
            id
        }

        fn dest(&self) -> PathBuf {
            self.root.join("Documentos/fo-foca")
        }

        fn opts(&self, zip: bool) -> ExportOptions {
            ExportOptions { transcript: Some(TranscriptFormat::Both), notes: true, tasks: true, audio: true, zip, dest: self.dest() }
        }

        fn listing(&self) -> Vec<String> {
            let mut out: Vec<String> = std::fs::read_dir(self.dest()).map(|d| d.map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect()).unwrap_or_default();
            out.sort();
            out
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn wav(len: usize, fill: u8) -> Vec<u8> {
        let mut v = b"RIFF".to_vec();
        v.resize(len, fill);
        v
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut out = Vec::new();
        for e in std::fs::read_dir(dir).unwrap() {
            let e = e.unwrap();
            let name = e.file_name().to_string_lossy().into_owned();
            if e.path().is_dir() {
                out.extend(files_in(&e.path()).into_iter().map(|f| format!("{name}/{f}")));
            } else {
                out.push(name);
            }
        }
        out.sort();
        out
    }

    #[test]
    fn exports_a_folder_and_a_zip_then_a_second_export_gets_a_suffix() {
        let f = Fixture::new("full");
        let id = f.meeting("Sync 20/11", true, None);
        let plan = plan(&f.store, &f.cfg, &id, &f.opts(true)).unwrap();
        assert_eq!(plan.folder_name, "2026-11-20 Sync 20-11");
        assert_eq!(plan.zip, Some(f.dest().join("2026-11-20 Sync 20-11.zip")));
        let paths: Vec<&str> = plan.files.iter().map(|p| p.path.as_str()).collect();
        assert_eq!(paths, ["transcript.md", "transcript.srt", "notes.md", "tasks.md", "audio/mic.wav", "audio/call.wav"]);
        assert_eq!(plan.files[4].bytes, (3 * CHUNK + 17) as u64);
        assert_eq!(plan.parts, Parts { transcript_lines: 2, notes: Some(NotesSource::Checked), tasks: 2, audio_tracks: 2, audio_deleted_at: None, imported: false });
        assert!(!f.dest().exists(), "planning writes nothing");

        let events = std::cell::RefCell::new(Vec::new());
        let done = run(&f.store, &f.cfg, &id, &f.opts(true), &|p| events.borrow_mut().push(p), &AtomicBool::new(false)).unwrap();
        assert_eq!(done.folder, plan.folder);
        assert_eq!(files_in(&done.folder), ["audio/call.wav", "audio/mic.wav", "notes.md", "tasks.md", "transcript.md", "transcript.srt"]);
        assert_eq!(std::fs::read(done.folder.join("audio/mic.wav")).unwrap(), wav(3 * CHUNK + 17, 1), "copied as stored");
        let meeting = f.store.load(&id).unwrap();
        assert_eq!(std::fs::read_to_string(done.folder.join("transcript.md")).unwrap(), transcript_md(&meeting, "pt"));
        assert_eq!(f.listing(), ["2026-11-20 Sync 20-11", "2026-11-20 Sync 20-11.zip"], "no temp folder or .part left");

        // the zip holds the folder: WAV stored, text deflated
        let mut z = zip::ZipArchive::new(File::open(done.zip.as_ref().unwrap()).unwrap()).unwrap();
        let mut names: Vec<(String, zip::CompressionMethod)> = (0..z.len()).map(|i| { let e = z.by_index(i).unwrap(); (e.name().to_string(), e.compression()) }).collect();
        names.sort_by(|a, b| a.0.cmp(&b.0));
        let (s, d) = (zip::CompressionMethod::Stored, zip::CompressionMethod::Deflated);
        let n = |p: &str| format!("2026-11-20 Sync 20-11/{p}");
        assert_eq!(names, [(n("audio/call.wav"), s), (n("audio/mic.wav"), s), (n("notes.md"), d), (n("tasks.md"), d), (n("transcript.md"), d), (n("transcript.srt"), d)]);
        let mut notes = String::new();
        z.by_name(&n("notes.md")).unwrap().read_to_string(&mut notes).unwrap();
        assert_eq!(notes, notes_md(&meeting, "pt").unwrap());

        // progress: files in order, then the zip; bytes never go back and end at the total
        let events = events.into_inner();
        let steps: Vec<(ExportStep, &str)> = events.iter().map(|e| (e.step, e.file.as_str())).collect();
        assert_eq!(steps.first(), Some(&(ExportStep::Writing, "transcript.md")));
        assert!(steps.contains(&(ExportStep::Copying, "audio/mic.wav")));
        assert_eq!(steps.last().unwrap().0, ExportStep::Zipping);
        assert!(events.windows(2).all(|w| w[0].done <= w[1].done));
        let total: u64 = plan.files.iter().map(|p| p.bytes).sum();
        assert!(events.iter().all(|e| e.total == 2 * total));

        // exporting again doesn't touch the first one
        let again = plan_and_run(&f, &id, true);
        assert_eq!(again.folder, f.dest().join("2026-11-20 Sync 20-11 (2)"));
        assert_eq!(again.zip, Some(f.dest().join("2026-11-20 Sync 20-11 (2).zip")));
        // a zip alone also takes the name
        std::fs::rename(&done.folder, f.root.join("moved")).unwrap();
        assert_eq!(super::plan(&f.store, &f.cfg, &id, &f.opts(false)).unwrap().folder_name, "2026-11-20 Sync 20-11 (3)");
    }

    fn plan_and_run(f: &Fixture, id: &str, zip: bool) -> ExportDone {
        let p = plan(&f.store, &f.cfg, id, &f.opts(zip)).unwrap();
        let done = run(&f.store, &f.cfg, id, &f.opts(zip), &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(done.folder, p.folder);
        done
    }

    #[test]
    fn deleted_audio_and_missing_parts() {
        let f = Fixture::new("noaudio");
        let id = f.meeting("Sem áudio", true, None);
        crate::retention::delete_audio(&f.store, &id).unwrap();
        let p = plan(&f.store, &f.cfg, &id, &f.opts(false)).unwrap();
        assert_eq!((p.parts.audio_tracks, p.parts.audio_deleted_at.is_some()), (0, true));
        assert!(p.files.iter().all(|x| x.part != FilePart::Audio));
        let err = run(&f.store, &f.cfg, &id, &f.opts(false), &|_| {}, &AtomicBool::new(false)).unwrap_err();
        assert!(err.to_string().contains("audio"), "{err}");
        assert!(f.listing().is_empty(), "a refused export writes nothing");

        let text_only = ExportOptions { audio: false, ..f.opts(false) };
        let done = run(&f.store, &f.cfg, &id, &text_only, &|_| {}, &AtomicBool::new(false)).unwrap();
        assert_eq!(files_in(&done.folder), ["notes.md", "tasks.md", "transcript.md", "transcript.srt"], "no empty audio/ folder");

        // no summary and no notes: notes.md can't be asked for
        let bare = f.store.create(&crate::model::StartInfo { title: "Rápida".into(), ..Default::default() }).unwrap().id;
        let p = plan(&f.store, &f.cfg, &bare, &f.opts(false)).unwrap();
        assert_eq!((p.parts.notes, p.parts.tasks, p.parts.audio_tracks, p.parts.audio_deleted_at.clone()), (None, 0, 0, None));
        assert!(run(&f.store, &f.cfg, &bare, &ExportOptions { audio: false, tasks: false, ..f.opts(false) }, &|_| {}, &AtomicBool::new(false)).is_err());
        let nothing = ExportOptions { transcript: None, notes: false, tasks: false, audio: false, ..f.opts(false) };
        assert!(run(&f.store, &f.cfg, &bare, &nothing, &|_| {}, &AtomicBool::new(false)).is_err());
    }

    #[test]
    fn an_import_exports_its_track_under_the_original_name() {
        let f = Fixture::new("import");
        let id = f.meeting("Cliente", true, Some("Gravação: cliente BH.m4a"));
        let p = plan(&f.store, &f.cfg, &id, &f.opts(false)).unwrap();
        assert!(p.parts.imported);
        let audio: Vec<&str> = p.files.iter().filter(|x| x.part == FilePart::Audio).map(|x| x.path.as_str()).collect();
        assert_eq!(audio, ["audio/Gravação- cliente BH.wav"]);
        let done = plan_and_run(&f, &id, false);
        assert!(done.folder.join("audio/Gravação- cliente BH.wav").is_file());
    }

    #[test]
    fn audio_deleted_during_the_export_is_still_copied() {
        let f = Fixture::new("racing");
        let id = f.meeting("Corrida", true, None);
        let dir = f.store.dir(&id);
        // the first progress event comes after the audio files were opened
        let deleted = std::cell::Cell::new(false);
        let done = run(&f.store, &f.cfg, &id, &f.opts(false), &|_| if !deleted.replace(true) { crate::retention::delete_audio(&f.store, &id).unwrap(); }, &AtomicBool::new(false)).unwrap();
        assert!(!dir.join("mic.wav").exists());
        assert_eq!(std::fs::read(done.folder.join("audio/mic.wav")).unwrap(), wav(3 * CHUNK + 17, 1));
    }

    #[test]
    fn cancel_removes_the_temp_folder_and_the_partial_zip() {
        let f = Fixture::new("cancel");
        let id = f.meeting("Longa", true, None);
        for at in [ExportStep::Copying, ExportStep::Zipping] {
            let cancel = AtomicBool::new(false);
            let partial = std::cell::Cell::new(false);
            let err = run(&f.store, &f.cfg, &id, &f.opts(true), &|p| {
                if p.step == at {
                    // something was written before the cancel lands
                    partial.set(partial.get() || f.listing().iter().any(|n| n.contains("fofoca-export")));
                    cancel.store(true, Ordering::Relaxed);
                }
            }, &cancel).unwrap_err();
            assert!(is_cancelled(&err), "{err}");
            assert!(partial.get(), "the temp folder existed while exporting");
            assert!(f.listing().is_empty(), "cancel at {at:?} left {:?}", f.listing());
        }
    }

    #[test]
    fn front_matter_quotes_titles_and_keeps_the_offset() {
        let mut m = meta("Ele disse \"oi\": 1:1");
        m.started_at = "2026-07-01T09:00:00+01:00".into();
        m.attendees.clear();
        let fm = front_matter(&m, None);
        assert!(fm.starts_with("---\ntitle: \"Ele disse \\\"oi\\\": 1:1\"\ndate: 2026-07-01T09:00:00+01:00\n"), "{fm}");
        assert!(fm.contains("attendees: []\n"));
    }
}
