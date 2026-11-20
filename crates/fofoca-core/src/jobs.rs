//! Background jobs (spec 012): one at a time, in order. Today: imports.
//!
//! The worker thread lives as long as the app, so the model servers and ffmpeg it starts are tied
//! to it (spec 013).

use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;

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
}

pub struct Jobs {
    tx: Sender<Job>,
    recording: Arc<AtomicBool>,
}

impl Jobs {
    /// `cfg` is read when each job starts, so Settings changes apply to the next job.
    pub fn start(store: Store, cfg: Arc<dyn Fn() -> Config + Send + Sync>, emit: JobSink) -> Jobs {
        let (tx, rx) = mpsc::channel::<Job>();
        let recording = Arc::new(AtomicBool::new(false));
        let rec = recording.clone();
        std::thread::Builder::new()
            .name("jobs".into())
            .spawn(move || {
                for job in rx {
                    match job {
                        Job::Import { id, src } => {
                            let ev = |step: &str, progress: f64, error: Option<String>| emit(JobEvent { meeting_id: id.clone(), step: step.into(), progress, error });
                            let progress = |step: &str, p: f64| ev(step, p, None);
                            let yield_gpu = || rec.load(Ordering::SeqCst);
                            let hooks = Hooks { progress: &progress, yield_gpu: &yield_gpu };
                            match pipeline::run_import(&store, &cfg(), &id, &src, &hooks) {
                                Ok(()) => ev("done", 1.0, None),
                                Err(e) => ev("failed", 0.0, Some(format!("{e:#}"))),
                            }
                        }
                    }
                }
            })
            .expect("starting the jobs thread");
        Jobs { tx, recording }
    }

    pub fn import(&self, id: String, src: PathBuf) {
        let _ = self.tx.send(Job::Import { id, src });
    }

    /// A live recording needs the GPU: jobs stop their model servers until it ends.
    pub fn set_recording(&self, on: bool) {
        self.recording.store(on, Ordering::SeqCst);
    }
}
