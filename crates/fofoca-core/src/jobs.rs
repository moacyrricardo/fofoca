//! Background jobs (spec 012): one at a time, in order. Today: imports.
//!
//! The worker thread lives as long as the app, so the model servers and ffmpeg it starts are tied
//! to it (spec 013).

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::collections::HashSet;

use crate::config::Config;
use crate::pipeline::{self, Hooks};
use crate::store::Store;

#[derive(Serialize, Clone, Debug)]
pub struct JobEvent {
    pub meeting_id: String,
    /// `queued`, `converting`, `transcribing`, `speakers`, `notes`, `done`, `failed`.
    pub step: String,
    /// 0..=1 within the step, when known.
    pub progress: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub type JobSink = Arc<dyn Fn(JobEvent) + Send + Sync>;

enum Job {
    Import { id: String, src: PathBuf },
    Retranscribe { id: String },
}

impl Job {
    fn id(&self) -> &str {
        match self { Job::Import { id, .. } | Job::Retranscribe { id } => id }
    }
}

pub struct Jobs {
    pending: Arc<Mutex<HashSet<String>>>,
    emit: JobSink,
    tx: Sender<Job>,
    recording: Arc<AtomicBool>,
}

impl Jobs {
    /// `cfg` is read when each job starts, so Settings changes apply to the next job.
    pub fn start(store: Store, cfg: Arc<dyn Fn() -> Config + Send + Sync>, emit: JobSink) -> Jobs {
        let (tx, rx) = mpsc::channel::<Job>();
        let recording = Arc::new(AtomicBool::new(false));
        let rec = recording.clone();
        let pending = Arc::new(Mutex::new(HashSet::new()));
        let active = Arc::clone(&pending);
        let sink = Arc::clone(&emit);
        std::thread::Builder::new()
            .name("jobs".into())
            .spawn(move || {
                for job in rx {
                    let id = job.id();
                    let ev = |step: &str, progress: f64, error| sink(JobEvent { meeting_id: id.to_owned(), step: step.into(), progress, error });
                    let progress = |step: &str, p: f64| ev(step, p, None);
                    let yield_gpu = || rec.load(Ordering::SeqCst);
                    let hooks = Hooks { progress: &progress, yield_gpu: &yield_gpu };
                    hooks.wait_for_gpu();
                    let result = match &job {
                        Job::Import { src, .. } => pipeline::run_import(&store, &cfg(), id, src, &hooks),
                        Job::Retranscribe { .. } => pipeline::retranscribe(&store, &cfg(), id, &hooks).map(|_| ()),
                    };
                    active.lock().unwrap().remove(id);
                    match result {
                        Ok(()) => ev("done", 1.0, None),
                        Err(e) => ev("failed", 0.0, Some(format!("{e:#}"))),
                    }
                }
            })
            .expect("starting the jobs thread");
        Jobs { tx, recording, pending, emit }
    }

    fn enqueue(&self, job: Job) -> anyhow::Result<()> {
        let id = job.id().to_owned();
        let mut pending = self.pending.lock().unwrap();
        anyhow::ensure!(pending.insert(id.clone()), "processing already queued for this meeting");
        (self.emit)(JobEvent { meeting_id: id.clone(), step: "queued".into(), progress: 0., error: None });
        if self.tx.send(job).is_err() {
            pending.remove(&id);
            anyhow::bail!("processing worker is unavailable");
        }
        Ok(())
    }

    pub fn import(&self, id: String, src: PathBuf) -> anyhow::Result<()> {
        self.enqueue(Job::Import { id, src })
    }

    pub fn retranscribe(&self, id: String) -> anyhow::Result<()> {
        self.enqueue(Job::Retranscribe { id })
    }

    /// A job for this meeting is queued or running.
    pub fn is_pending(&self, id: &str) -> bool {
        self.pending.lock().unwrap().contains(id)
    }

    /// A live recording needs the GPU: jobs stop their model servers until it ends.
    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::StartInfo;
    use std::time::Duration;

    #[test]
    fn retranscription_waits_for_recording_and_runs_fifo() {
        let root = std::env::temp_dir().join(format!("fofoca-job-{}", std::process::id()));
        let store = Store::new(&root).unwrap();
        let first = store.create(&StartInfo::default()).unwrap().id;
        let second = store.create(&StartInfo::default()).unwrap().id;
        let (tx, rx) = mpsc::channel();
        let jobs = Jobs::start(store, Arc::new(Config::default), Arc::new(move |e| tx.send(e).unwrap()));
        jobs.set_recording(true);
        jobs.retranscribe(first.clone()).unwrap();
        assert!(jobs.retranscribe(first.clone()).is_err());
        jobs.retranscribe(second.clone()).unwrap();
        for id in [&first, &second] {
            let e = rx.recv_timeout(Duration::from_secs(2)).unwrap();
            assert_eq!((&e.meeting_id, e.step.as_str()), (id, "queued"));
        }
        assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(jobs.is_pending(&first) && jobs.is_pending(&second) && !jobs.is_pending("other"));
        jobs.set_recording(false);
        for id in [&first, &second] {
            let e = rx.recv_timeout(Duration::from_secs(3)).unwrap();
            assert_eq!((&e.meeting_id, e.step.as_str()), (id, "failed"));
            assert!(e.error.unwrap().contains("audio is no longer stored"));
            assert!(!jobs.is_pending(id), "no longer pending once finished");
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
