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

use crate::model::{Meeting, MeetingMeta, Segment, Summary};

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
    pub fn create(&self, title: &str) -> Result<MeetingMeta> {
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
        };
        self.save_meta(&meta)?;
        Ok(meta)
    }

    pub fn save_meta(&self, meta: &MeetingMeta) -> Result<()> {
        write_json(&self.dir(&meta.id).join("meta.json"), meta)
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

    /// All meetings, newest first. Folders without a readable meta.json are skipped.
    pub fn list(&self) -> Result<Vec<MeetingMeta>> {
        let mut out: Vec<MeetingMeta> = std::fs::read_dir(&self.root)?
            .filter_map(|e| e.ok())
            .filter_map(|e| read_json(&e.path().join("meta.json")).ok())
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
        let mut meta = store.create("Sync").unwrap();
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
        let second = store.create("").unwrap();
        assert_ne!(second.id, meta.id);
        assert_eq!(store.list().unwrap().len(), 2);
        std::fs::remove_dir_all(&root).unwrap();
    }
}
