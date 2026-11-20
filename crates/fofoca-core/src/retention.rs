//! Audio retention (spec 011): meeting audio (`mic.wav`, `system.wav`) is deleted by policy;
//! transcripts, notes, labels and speakers never are.

use anyhow::Result;
use chrono::{DateTime, Duration, FixedOffset};

use crate::config::{AudioConfig, KeepAudio};
use crate::model::MeetingMeta;
use crate::store::Store;

const AUDIO_FILES: [&str; 2] = ["mic.wav", "system.wav"];

/// Whether this meeting's audio should go now.
pub fn should_delete(meta: &MeetingMeta, policy: &AudioConfig, now: DateTime<FixedOffset>) -> bool {
    if meta.audio_deleted_at.is_some() || !meta.finished || meta.keep_audio == Some(true) {
        return false;
    }
    match policy.keep {
        KeepAudio::Always => false,
        KeepAudio::AfterProcessing => meta.processed,
        KeepAudio::Days => DateTime::parse_from_rfc3339(&meta.started_at)
            .map(|start| now - start >= Duration::days(policy.keep_days as i64))
            .unwrap_or(false),
    }
}

/// Bytes of audio a meeting still has.
pub fn audio_bytes(store: &Store, id: &str) -> u64 {
    AUDIO_FILES.iter().filter_map(|f| std::fs::metadata(store.dir(id).join(f)).ok()).map(|m| m.len()).sum()
}

/// Delete one meeting's audio now and record when. Returns the bytes freed.
pub fn delete_audio(store: &Store, id: &str) -> Result<u64> {
    let freed = audio_bytes(store, id);
    for f in AUDIO_FILES {
        let p = store.dir(id).join(f);
        if p.exists() {
            std::fs::remove_file(&p)?;
        }
    }
    store.update_meta(id, |m| m.audio_deleted_at = Some(chrono::Local::now().to_rfc3339()))?;
    Ok(freed)
}

/// Apply the policy to every meeting (or only `only`). With `dry_run`, nothing is deleted.
/// Returns `(meeting id, bytes)` for each meeting whose audio went (or would go).
pub fn apply(store: &Store, policy: &AudioConfig, now: DateTime<FixedOffset>, only: Option<&str>, dry_run: bool) -> Result<Vec<(String, u64)>> {
    let mut out = Vec::new();
    for m in store.list(None)? {
        if only.is_some_and(|id| id != m.id) || !should_delete(&m, policy, now) {
            continue;
        }
        let bytes = if dry_run { audio_bytes(store, &m.id) } else { delete_audio(store, &m.id)? };
        out.push((m.id, bytes));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StartInfo;

    fn now() -> DateTime<FixedOffset> {
        DateTime::parse_from_rfc3339("2026-12-20T12:00:00-03:00").unwrap()
    }

    fn meta(started: &str, finished: bool, processed: bool, keep: Option<bool>) -> MeetingMeta {
        MeetingMeta {
            id: "m".into(),
            title: "t".into(),
            started_at: started.into(),
            duration_s: 60.0,
            finished,
            attendees: vec![],
            labels: vec![],
            calendar_uid: None,
            pauses: vec![],
            steps_done: vec![],
            processed,
            keep_audio: keep,
            audio_deleted_at: None,
        }
    }

    #[test]
    fn policies_and_overrides() {
        let after = AudioConfig { keep: KeepAudio::AfterProcessing, keep_days: 30 };
        let days = AudioConfig { keep: KeepAudio::Days, keep_days: 30 };
        let always = AudioConfig { keep: KeepAudio::Always, keep_days: 30 };
        let old = "2026-11-01T10:00:00-03:00"; // 49 days before now()
        let recent = "2026-12-10T10:00:00-03:00"; // 10 days before

        assert!(should_delete(&meta(recent, true, true, None), &after, now()));
        assert!(!should_delete(&meta(recent, true, false, None), &after, now()), "a step failed or was skipped: keep for a retry");
        assert!(!should_delete(&meta(recent, false, true, None), &after, now()), "still recording / never finished");
        assert!(should_delete(&meta(old, true, false, None), &days, now()));
        assert!(!should_delete(&meta(recent, true, true, None), &days, now()));
        assert!(!should_delete(&meta(old, true, true, None), &always, now()));
        assert!(!should_delete(&meta(old, true, true, Some(true)), &days, now()), "kept by hand");
        let mut gone = meta(old, true, true, None);
        gone.audio_deleted_at = Some("2026-12-01T00:00:00-03:00".into());
        assert!(!should_delete(&gone, &days, now()), "already deleted");
    }

    #[test]
    fn apply_deletes_only_audio() {
        let root = std::env::temp_dir().join(format!("fofoca-retention-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::new(&root).unwrap();
        let m = store.create(&StartInfo::default()).unwrap();
        for f in ["mic.wav", "system.wav"] {
            std::fs::write(store.dir(&m.id).join(f), b"0123456789").unwrap();
        }
        std::fs::write(store.dir(&m.id).join("transcript.jsonl"), b"").unwrap();
        store.update_meta(&m.id, |x| {
            x.finished = true;
            x.processed = true;
        })
        .unwrap();
        let policy = AudioConfig::default();
        let dry = apply(&store, &policy, now(), None, true).unwrap();
        assert_eq!(dry, vec![(m.id.clone(), 20)]);
        assert!(store.dir(&m.id).join("mic.wav").exists(), "dry run deletes nothing");
        assert_eq!(apply(&store, &policy, now(), None, false).unwrap(), vec![(m.id.clone(), 20)]);
        assert!(!store.dir(&m.id).join("mic.wav").exists() && !store.dir(&m.id).join("system.wav").exists());
        assert!(store.dir(&m.id).join("transcript.jsonl").exists(), "the transcript stays");
        assert!(store.load(&m.id).unwrap().meta.audio_deleted_at.is_some());
        assert!(apply(&store, &policy, now(), None, false).unwrap().is_empty(), "idempotent");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
