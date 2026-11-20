//! Meetings on disk: one folder per meeting.
//!
//! ```text
//! <meetings_dir>/<id>/meta.json        MeetingMeta
//!                     transcript.jsonl one Segment per line, appended live
//!                     summaries.json   Vec<Summary>
//!                     mic.wav          microphone track
//!                     system.wav       system-audio track
//! ```

use anyhow::{Context, Result};
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use crate::labels;
use crate::model::{Meeting, MeetingMeta, Segment, StartInfo, Summary};

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
            started_at: now.to_rfc3339(),
            duration_s: 0.0,
            finished: false,
            attendees: info.attendees.clone(),
            labels: labels::normalize_all(&info.labels),
            calendar_uid: info.calendar_uid.clone().filter(|u| !u.is_empty()),
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

    pub fn add_summary(&self, id: &str, summary: &Summary) -> Result<()> {
        let path = self.dir(id).join("summaries.json");
        let mut all: Vec<Summary> = read_json(&path).unwrap_or_default();
        all.push(summary.clone());
        write_json(&path, &all)
    }

    pub fn load(&self, id: &str) -> Result<Meeting> {
        let dir = self.dir(id);
        let meta: MeetingMeta = read_json(&dir.join("meta.json"))?;
        let mut segments = Vec::new();
        if let Ok(f) = std::fs::File::open(dir.join("transcript.jsonl")) {
            for line in BufReader::new(f).lines() {
                let line = line?;
                if !line.trim().is_empty() {
                    segments.push(serde_json::from_str(&line)?);
                }
            }
        }
        let summaries = read_json(&dir.join("summaries.json")).unwrap_or_default();
        Ok(Meeting { meta, segments, summaries })
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
        let seg = Segment { start: 0.0, end: 1.0, text: "Oi".into(), track: Track::You, words: vec![] };
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
}
