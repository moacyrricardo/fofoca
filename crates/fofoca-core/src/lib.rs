//! fo-foca core: everything that isn't UI.
//!
//! The desktop app (Tauri) and the `fofoca-cli` binary are thin shells over this crate.

pub mod audio;
pub mod calendar;
pub mod capture;
pub mod config;
pub mod diarize;
pub mod grounding;
pub mod hardware;
pub mod import;
pub mod jobs;
pub mod labels;
pub mod llm;
pub mod media;
pub mod model;
pub mod models;
pub mod notes;
pub mod pipeline;
pub mod proc;
pub mod retention;
pub mod prompts;
pub mod servers;
pub mod session;
pub mod slicer;
pub mod speakers;
pub mod store;
pub mod tracks;
pub mod whisper;

pub use config::Config;
pub use model::{MeetingMeta, Segment, StartInfo, Summary, Track, Word};
pub use session::{ModelState, Session, SessionEvent};
pub use store::Store;
