//! Data types shared by the session, the store and the UI.

use serde::{Deserialize, Serialize};

/// Which recorded track a transcript segment is attributed to.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Track {
    /// The local microphone — the user.
    You,
    /// The system audio — everyone on the call.
    Call,
}

/// One transcribed word with meeting-relative timestamps (seconds).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Word {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub p: f32,
}

/// One transcript segment with meeting-relative timestamps (seconds).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub track: Track,
    #[serde(default)]
    pub words: Vec<Word>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SummaryKind {
    /// "Summary so far", requested during the call.
    Partial,
    /// Written once after the call ends.
    Final,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Summary {
    pub kind: SummaryKind,
    /// Meeting time (seconds) the summary covers up to.
    pub at_s: f64,
    pub text: String,
    pub model: String,
    #[serde(default)]
    pub gen_tokens_per_s: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MeetingMeta {
    pub id: String,
    pub title: String,
    /// RFC 3339 local time.
    pub started_at: String,
    #[serde(default)]
    pub duration_s: f64,
    #[serde(default)]
    pub finished: bool,
}

/// A meeting as loaded from disk.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Meeting {
    pub meta: MeetingMeta,
    pub segments: Vec<Segment>,
    pub summaries: Vec<Summary>,
}
