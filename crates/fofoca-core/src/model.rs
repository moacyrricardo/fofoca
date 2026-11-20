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
    /// After speaker separation (spec 003): `"you"` for the mic, `"s1"`, `"s2"`, … for voices on
    /// the call. `None` before separation or when no voice could be matched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
}

/// A voice found on the call (`speakers.json`, keyed by `s1`, `s2`, …).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct SpeakerInfo {
    /// Name given by the user; shown instead of "Person N".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
    /// Invitees from the calendar event the recording was started from (names for spec 003).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attendees: Vec<String>,
    /// Normalized labels (spec 007), see [`crate::labels::normalize`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    /// `UID` of the calendar event it was recorded from — shared by every instance of a recurring
    /// series, so later recordings can inherit labels (spec 009).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub calendar_uid: Option<String>,
    /// Paused intervals (spec 010). Audio during a pause is not kept, so the meeting timeline is
    /// continuous: `at_s` is the meeting time where the pause sits.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pauses: Vec<Pause>,
    /// Post-call steps that finished (`"speakers"`, …), so a failed step can be retried alone.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps_done: Vec<String>,
    /// Every post-call step that needs the audio has succeeded (spec 011: only then can
    /// "delete after processing" remove it).
    #[serde(default)]
    pub processed: bool,
    /// Per-meeting override: `Some(true)` keeps the audio whatever the global setting says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub keep_audio: Option<bool>,
    /// RFC 3339 time the audio files were deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_deleted_at: Option<String>,
    /// File name it was imported from (spec 012); `None` for meetings recorded in fo-foca.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Pause {
    pub at_s: f64,
    /// RFC 3339 wall-clock times; `to` is `None` if the recording ended while paused.
    pub from: String,
    #[serde(default)]
    pub to: Option<String>,
}

/// What a new meeting starts with.
#[derive(Clone, Debug, Default)]
pub struct StartInfo {
    pub title: String,
    pub attendees: Vec<String>,
    pub labels: Vec<String>,
    pub calendar_uid: Option<String>,
    /// Imports (spec 012): when the recording was made, and the source file name.
    pub started_at: Option<String>,
    pub imported_from: Option<String>,
}

/// A meeting as loaded from disk.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Meeting {
    pub meta: MeetingMeta,
    pub segments: Vec<Segment>,
    pub summaries: Vec<Summary>,
    #[serde(default)]
    pub speakers: std::collections::BTreeMap<String, SpeakerInfo>,
    /// Grounded notes (spec 005), when the final step produced them.
    #[serde(default)]
    pub notes: Option<crate::grounding::Notes>,
}
