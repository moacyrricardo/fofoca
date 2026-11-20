//! Meetings on disk: one folder per meeting.
//!
//! ```text
//! <meetings_dir>/<id>/meta.json        MeetingMeta
//!                     transcript.jsonl one Segment per line, appended live
//!                     summaries.json   Vec<Summary>
//!                     speakers.json    { "s1": SpeakerInfo, … } (spec 003)
//!                     notes.json       grounded notes: summary, decisions, action items (spec 005)
//!                     transcript.live.jsonl  the live transcript, kept when speakers are assigned
//!                     transcript.base.jsonl  the per-track rebuild after the call (spec 018)
//!                     mic.wav          microphone track
//!                     system.wav       system-audio track
//! ```

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::labels;
use crate::model::{Meeting, MeetingMeta, Segment, SpeakerInfo, StartInfo, Summary};
use std::collections::BTreeMap;

/// A meeting's audio files on disk (spec 014); `None` when that track doesn't exist (imports have
/// no mic track; retention may have deleted both).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct AudioTracks {
    pub mic: Option<PathBuf>,
    pub call: Option<PathBuf>,
}

/// One action item with the meeting it came from (spec 016). Tasks have no labels of their own:
/// they're filtered by their meeting's.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct TaskItem {
    pub meeting_id: String,
    pub meeting_title: String,
    pub started_at: String,
    pub labels: Vec<String>,
    pub text: String,
    pub owner: Option<String>,
    /// `YYYY-MM-DD`.
    pub due: Option<String>,
    pub refs: Vec<f64>,
}

/// Serializes read-modify-write of meta.json files (UI commands and the session worker run on
/// different threads).
static META_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[derive(Clone)]
pub struct Store {
    root: PathBuf,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Result<Store> {
        let root = root.into();
        std::fs::create_dir_all(&root).with_context(|| format!("creating {}", root.display()))?;
        Ok(Store { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir(&self, id: &str) -> PathBuf {
        self.root.join(id)
    }

    /// Create a new meeting folder with an id derived from the local start time.
    pub fn create(&self, info: &StartInfo) -> Result<MeetingMeta> {
        let title = info.title.as_str();
        let now = chrono::Local::now();
        let mut id = now.format("%Y-%m-%d-%H%M%S").to_string();
        let mut n = 1;
        while self.dir(&id).exists() {
            n += 1;
            id = format!("{}-{n}", now.format("%Y-%m-%d-%H%M%S"));
        }
        std::fs::create_dir_all(self.dir(&id))?;
        let meta = MeetingMeta {
            id,
            title: if title.trim().is_empty() { format!("Meeting {}", now.format("%d/%m %H:%M")) } else { title.trim().to_string() },
            started_at: info.started_at.clone().unwrap_or_else(|| now.to_rfc3339()),
            duration_s: 0.0,
            finished: false,
            attendees: info.attendees.clone(),
            labels: labels::normalize_all(&info.labels),
            calendar_uid: info.calendar_uid.clone().filter(|u| !u.is_empty()),
            pauses: Vec::new(),
            steps_done: Vec::new(),
            processed: false,
            keep_audio: None,
            audio_deleted_at: None,
            imported_from: info.imported_from.clone(),
        };
        self.save_meta(&meta)?;
        Ok(meta)
    }

    pub fn save_meta(&self, meta: &MeetingMeta) -> Result<()> {
        write_json(&self.dir(&meta.id).join("meta.json"), meta)
    }

    /// Re-read `meta.json`, change it and write it back, so fields edited elsewhere meanwhile
    /// (e.g. labels set while recording) are not overwritten with a stale copy.
    pub fn update_meta(&self, id: &str, f: impl FnOnce(&mut MeetingMeta)) -> Result<MeetingMeta> {
        let _w = META_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut meta: MeetingMeta = read_json(&self.dir(id).join("meta.json")).with_context(|| format!("meeting {id} not found"))?;
        f(&mut meta);
        self.save_meta(&meta)?;
        Ok(meta)
    }

    /// Replace a meeting's labels (normalized, deduplicated).
    pub fn set_labels<S: AsRef<str>>(&self, id: &str, new: &[S]) -> Result<MeetingMeta> {
        let new = labels::normalize_all(new);
        self.update_meta(id, |m| m.labels = new)
    }

    /// Labels of the most recent meeting recorded from calendar event `uid` (empty if none).
    pub fn labels_for_series(&self, uid: &str) -> Result<Vec<String>> {
        // list() is newest first
        Ok(self.list(None)?.into_iter().find(|m| m.calendar_uid.as_deref() == Some(uid)).map(|m| m.labels).unwrap_or_default())
    }

    /// Every label in use with the number of meetings carrying it, most used first.
    pub fn known_labels(&self) -> Result<Vec<(String, usize)>> {
        let mut counts: std::collections::HashMap<String, usize> = Default::default();
        for m in self.list(None)? {
            for l in m.labels {
                *counts.entry(l).or_default() += 1;
            }
        }
        let mut out: Vec<_> = counts.into_iter().collect();
        out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        Ok(out)
    }

    pub fn append_segment(&self, id: &str, seg: &Segment) -> Result<()> {
        let mut f = OpenOptions::new().create(true).append(true).open(self.dir(id).join("transcript.jsonl"))?;
        writeln!(f, "{}", serde_json::to_string(seg)?)?;
        Ok(())
    }

    /// Replace the whole transcript (atomically). The first time, the live transcript is kept as
    /// `transcript.live.jsonl` so speaker assignment can be redone from it.
    pub fn replace_transcript(&self, id: &str, segments: &[Segment]) -> Result<()> {
        let dir = self.dir(id);
        let (path, live) = (dir.join("transcript.jsonl"), dir.join("transcript.live.jsonl"));
        if path.exists() && !live.exists() {
            std::fs::copy(&path, &live)?;
        }
        let tmp = dir.join("transcript.jsonl.tmp");
        let mut body = String::new();
        for s in segments {
            body.push_str(&serde_json::to_string(s)?);
            body.push('\n');
        }
        std::fs::write(&tmp, body)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Replace the transcript with one rebuilt from the audio, track by track (spec 018). It is
    /// also kept as `transcript.base.jsonl`, the copy speaker separation starts from; the first
    /// live transcript stays in `transcript.live.jsonl`.
    pub fn rebuild_transcript(&self, id: &str, segments: &[Segment]) -> Result<()> {
        self.replace_transcript(id, segments)?;
        let dir = self.dir(id);
        let tmp = dir.join("transcript.base.jsonl.tmp");
        std::fs::copy(dir.join("transcript.jsonl"), &tmp)?;
        std::fs::rename(&tmp, dir.join("transcript.base.jsonl"))?;
        Ok(())
    }

    /// The transcript speaker separation starts from: the per-track rebuild when there is one
    /// (spec 018), otherwise the live transcript.
    pub fn base_transcript(&self, id: &str) -> Result<Vec<Segment>> {
        let base = self.dir(id).join("transcript.base.jsonl");
        if base.exists() {
            read_jsonl(&base)
        } else {
            self.live_transcript(id)
        }
    }

    /// The transcript as recorded live (before speaker assignment split any lines).
    pub fn live_transcript(&self, id: &str) -> Result<Vec<Segment>> {
        let live = self.dir(id).join("transcript.live.jsonl");
        if live.exists() {
            read_jsonl(&live)
        } else {
            read_jsonl(&self.dir(id).join("transcript.jsonl"))
        }
    }

    pub fn save_notes(&self, id: &str, notes: &crate::grounding::Notes) -> Result<()> {
        write_json(&self.dir(id).join("notes.json"), notes)
    }

    pub fn speakers(&self, id: &str) -> BTreeMap<String, SpeakerInfo> {
        read_json(&self.dir(id).join("speakers.json")).unwrap_or_default()
    }

    pub fn save_speakers(&self, id: &str, speakers: &BTreeMap<String, SpeakerInfo>) -> Result<()> {
        write_json(&self.dir(id).join("speakers.json"), speakers)
    }

    /// Name a voice; an empty name goes back to "Person N".
    pub fn rename_speaker(&self, id: &str, key: &str, name: &str) -> Result<BTreeMap<String, SpeakerInfo>> {
        let _w = META_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let mut all = self.speakers(id);
        let s = all.get_mut(key).with_context(|| format!("no speaker {key} in meeting {id}"))?;
        let name = name.trim();
        s.name = (!name.is_empty()).then(|| name.to_string());
        self.save_speakers(id, &all)?;
        Ok(all)
    }

    pub fn add_summary(&self, id: &str, summary: &Summary) -> Result<()> {
        let path = self.dir(id).join("summaries.json");
        let mut all: Vec<Summary> = read_json(&path).unwrap_or_default();
        all.push(summary.clone());
        write_json(&path, &all)
    }

    /// The meeting's audio files that exist, for replay (spec 014). Refuses anything that isn't a
    /// plain meeting folder name, so the UI can't be talked into serving other files.
    pub fn audio_tracks(&self, id: &str) -> Result<AudioTracks> {
        let plain = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
        anyhow::ensure!(plain, "not a meeting id: {id:?}");
        let dir = self.dir(id);
        anyhow::ensure!(dir.join("meta.json").is_file(), "no meeting {id}");
        let file = |name: &str| Some(dir.join(name)).filter(|p| p.is_file());
        Ok(AudioTracks { mic: file("mic.wav"), call: file("system.wav") })
    }

    pub fn load(&self, id: &str) -> Result<Meeting> {
        let dir = self.dir(id);
        let meta: MeetingMeta = read_json(&dir.join("meta.json"))?;
        let path = dir.join("transcript.jsonl");
        let segments = if path.exists() { read_jsonl(&path)? } else { Vec::new() };
        let summaries = read_json(&dir.join("summaries.json")).unwrap_or_default();
        let notes = read_json(&dir.join("notes.json")).ok();
        Ok(Meeting { meta, segments, summaries, speakers: self.speakers(id), notes })
    }

    /// Every meeting's action items, newest meeting first (spec 016). Meetings without checked
    /// notes are skipped.
    pub fn tasks(&self) -> Result<Vec<TaskItem>> {
        let mut out = Vec::new();
        for m in self.list(None)? {
            let Ok(notes) = read_json::<crate::grounding::Notes>(&self.dir(&m.id).join("notes.json")) else { continue };
            out.extend(notes.action_items.into_iter().map(|a| TaskItem {
                meeting_id: m.id.clone(),
                meeting_title: m.title.clone(),
                started_at: m.started_at.clone(),
                labels: m.labels.clone(),
                text: a.text,
                owner: a.owner,
                due: a.due,
                refs: a.refs,
            }));
        }
        Ok(out)
    }

    /// All meetings (or only those carrying `label`), newest first. Folders without a readable
    /// meta.json are skipped.
    pub fn list(&self, label: Option<&str>) -> Result<Vec<MeetingMeta>> {
        let label = label.and_then(labels::normalize);
        let mut out: Vec<MeetingMeta> = std::fs::read_dir(&self.root)?
            .filter_map(|e| e.ok())
            .filter_map(|e| read_json::<MeetingMeta>(&e.path().join("meta.json")).ok())
            .filter(|m| label.as_ref().is_none_or(|l| m.labels.contains(l)))
            .collect();
        out.sort_by(|a, b| b.started_at.cmp(&a.started_at));
        Ok(out)
    }
}

fn write_json<T: serde::Serialize>(path: &Path, v: &T) -> Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(v)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn read_jsonl<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let mut out = Vec::new();
    for line in BufReader::new(std::fs::File::open(path)?).lines() {
        let line = line?;
        if !line.trim().is_empty() {
            out.push(serde_json::from_str(&line)?);
        }
    }
    Ok(out)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{SummaryKind, Track};

    #[test]
    fn create_append_load_list() {
        let root = std::env::temp_dir().join(format!("fofoca-store-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let mut meta = store.create(&StartInfo { title: "Sync".into(), attendees: vec!["Rodrigo".into()], ..Default::default() }).unwrap();
        let seg = Segment { start: 0.0, end: 1.0, text: "Oi".into(), track: Track::You, words: vec![], speaker: None };
        store.append_segment(&meta.id, &seg).unwrap();
        store.append_segment(&meta.id, &seg).unwrap();
        store
            .add_summary(&meta.id, &Summary { kind: SummaryKind::Final, at_s: 1.0, text: "ok".into(), model: "m".into(), gen_tokens_per_s: 1.0 })
            .unwrap();
        meta.finished = true;
        store.save_meta(&meta).unwrap();

        let m = store.load(&meta.id).unwrap();
        assert_eq!(m.segments.len(), 2);
        assert_eq!(m.summaries.len(), 1);
        assert!(m.meta.finished);
        assert_eq!(store.load(&meta.id).unwrap().meta.attendees, vec!["Rodrigo".to_string()]);
        let second = store.create(&StartInfo { labels: vec!["Trabalho".into(), "trabalho".into()], ..Default::default() }).unwrap();
        assert_ne!(second.id, meta.id);
        assert_eq!(second.labels, vec!["trabalho"]);
        assert_eq!(store.list(None).unwrap().len(), 2);

        // labels: set, filter, count; a later meta update keeps them
        store.set_labels(&meta.id, &["Cliente BH", "trabalho", "cliente bh"]).unwrap();
        store.update_meta(&meta.id, |m| m.duration_s = 9.0).unwrap();
        let m = store.load(&meta.id).unwrap().meta;
        assert_eq!(m.labels, vec!["cliente-bh", "trabalho"]);
        assert_eq!(m.duration_s, 9.0);
        assert_eq!(store.list(Some("Trabalho")).unwrap().len(), 2);
        assert_eq!(store.list(Some("cliente-bh")).unwrap().len(), 1);
        assert!(store.list(Some("nope")).unwrap().is_empty());
        assert_eq!(store.known_labels().unwrap(), vec![("trabalho".to_string(), 2), ("cliente-bh".to_string(), 1)]);
        assert!(store.set_labels("missing", &["x"]).is_err());

        // speakers: replacing the transcript keeps the live copy; names round-trip
        let mut split = seg.clone();
        split.speaker = Some("s1".into());
        store.replace_transcript(&meta.id, &[split.clone(), split.clone(), split]).unwrap();
        assert_eq!(store.load(&meta.id).unwrap().segments.len(), 3);
        assert_eq!(store.live_transcript(&meta.id).unwrap().len(), 2, "live copy kept");
        let mut sp = BTreeMap::new();
        sp.insert("s1".to_string(), SpeakerInfo::default());
        store.save_speakers(&meta.id, &sp).unwrap();
        store.rename_speaker(&meta.id, "s1", "  Fernanda ").unwrap();
        assert_eq!(store.load(&meta.id).unwrap().speakers["s1"].name.as_deref(), Some("Fernanda"));
        assert!(store.rename_speaker(&meta.id, "s7", "x").is_err());
        assert_eq!(store.rename_speaker(&meta.id, "s1", "").unwrap()["s1"].name, None);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn rebuilt_transcript_is_the_base_for_speakers() {
        let root = std::env::temp_dir().join(format!("fofoca-store-base-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let meta = store.create(&StartInfo::default()).unwrap();
        let line = |t: f64, track: Track, text: &str| Segment { start: t, end: t + 1.0, text: text.into(), track, words: vec![], speaker: None };
        // live: one mixed line, labelled Call by the old loudness guess
        store.append_segment(&meta.id, &line(0.0, Track::Call, "Oi, como vai?")).unwrap();
        assert_eq!(store.base_transcript(&meta.id).unwrap().len(), 1, "no rebuild yet: the live one");

        let rebuilt = vec![line(0.0, Track::Call, "Oi, como vai?"), line(2.0, Track::You, "tudo certo?")];
        store.rebuild_transcript(&meta.id, &rebuilt).unwrap();
        assert_eq!(store.load(&meta.id).unwrap().segments, rebuilt);
        assert_eq!(store.base_transcript(&meta.id).unwrap(), rebuilt);
        assert_eq!(store.live_transcript(&meta.id).unwrap().len(), 1, "the live copy is kept");

        // speakers then split lines: the base stays the rebuild
        let mut split = rebuilt.clone();
        split[0].speaker = Some("s1".into());
        split.push(line(5.0, Track::Call, "x"));
        store.replace_transcript(&meta.id, &split).unwrap();
        assert_eq!(store.base_transcript(&meta.id).unwrap(), rebuilt);

        // record-only: no live transcript at all, the rebuild is the first one
        let ro = store.create(&StartInfo { title: "ro".into(), ..Default::default() }).unwrap();
        store.rebuild_transcript(&ro.id, &rebuilt).unwrap();
        assert_eq!(store.base_transcript(&ro.id).unwrap(), rebuilt);
        assert_eq!(store.load(&ro.id).unwrap().segments, rebuilt);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn series_labels_come_from_the_newest_meeting() {
        let root = std::env::temp_dir().join(format!("fofoca-store-series-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let info = |labels: &[&str], uid: Option<&str>| StartInfo {
            labels: labels.iter().map(|s| s.to_string()).collect(),
            calendar_uid: uid.map(str::to_string),
            ..Default::default()
        };
        assert!(store.labels_for_series("weekly@x").unwrap().is_empty());
        let old = store.create(&info(&["1:1"], Some("weekly@x"))).unwrap();
        let new = store.create(&info(&["1:1", "fernanda"], Some("weekly@x"))).unwrap();
        store.create(&info(&["other"], Some("else@x"))).unwrap();
        store.create(&info(&["no-uid"], None)).unwrap();
        // ids are second-resolution; force the order instead of sleeping
        store.update_meta(&old.id, |m| m.started_at = "2026-11-01T10:00:00-03:00".into()).unwrap();
        store.update_meta(&new.id, |m| m.started_at = "2026-11-08T10:00:00-03:00".into()).unwrap();
        assert_eq!(store.labels_for_series("weekly@x").unwrap(), vec!["1:1", "fernanda"]);
        assert_eq!(store.load(&new.id).unwrap().meta.calendar_uid.as_deref(), Some("weekly@x"));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn audio_tracks_lists_existing_files_and_refuses_other_ids() {
        let root = std::env::temp_dir().join(format!("fofoca-store-audio-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(root.join("meetings")).unwrap();
        let meta = store.create(&StartInfo::default()).unwrap();
        assert_eq!(store.audio_tracks(&meta.id).unwrap(), AudioTracks { mic: None, call: None });
        std::fs::write(store.dir(&meta.id).join("system.wav"), b"RIFF").unwrap();
        let t = store.audio_tracks(&meta.id).unwrap();
        assert_eq!((t.mic, t.call), (None, Some(store.dir(&meta.id).join("system.wav"))));
        std::fs::write(store.dir(&meta.id).join("mic.wav"), b"RIFF").unwrap();
        assert!(store.audio_tracks(&meta.id).unwrap().mic.is_some());
        // a folder outside the store, even one that looks like a meeting
        std::fs::create_dir_all(root.join("outside")).unwrap();
        std::fs::write(root.join("outside/meta.json"), b"{}").unwrap();
        for bad in ["", "..", "../outside", "/etc", "a/b", "nope"] {
            assert!(store.audio_tracks(bad).is_err(), "{bad:?} must be refused");
        }
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn tasks_come_with_their_meeting_and_skip_meetings_without_notes() {
        use crate::grounding::{Action, Notes};
        let root = std::env::temp_dir().join(format!("fofoca-store-tasks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let a = store.create(&StartInfo { title: "Sync".into(), labels: vec!["Trabalho".into()], started_at: Some("2026-11-18T10:00:00-03:00".into()), ..Default::default() }).unwrap();
        let _no_notes = store.create(&StartInfo { title: "Chat".into(), started_at: Some("2026-11-19T10:00:00-03:00".into()), ..Default::default() }).unwrap();
        let notes = Notes {
            summary: vec![], decisions: vec![], flagged: vec![], model: "m".into(), gen_tokens_per_s: 0.0,
            action_items: vec![Action { text: "Enviar o follow-up".into(), owner: Some("Você".into()), due: Some("2026-11-19".into()), refs: vec![64.0] }],
        };
        store.save_notes(&a.id, &notes).unwrap();
        let tasks = store.tasks().unwrap();
        assert_eq!(tasks.len(), 1);
        let t = &tasks[0];
        assert_eq!((t.meeting_id.as_str(), t.meeting_title.as_str(), t.labels.clone()), (a.id.as_str(), "Sync", vec!["trabalho".to_string()]));
        assert_eq!((t.text.as_str(), t.owner.as_deref(), t.due.as_deref(), t.refs.clone()), ("Enviar o follow-up", Some("Você"), Some("2026-11-19"), vec![64.0]));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
