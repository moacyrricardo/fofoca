//! User configuration (`$XDG_CONFIG_HOME/fofoca/config.toml`).
//!
//! Which model runs each task and where (GPU/CPU) is chosen globally in Settings (spec 015);
//! benchmarking the machine to suggest it is spec 002.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Config {
    /// Spoken language passed to Whisper (`pt`, `en`, …) — also selects the prompt language.
    pub language: String,
    /// Longest transcription chunk.
    pub chunk_seconds: u32,
    /// When Whisper is idle, end a chunk at the first natural pause after this many seconds
    /// instead of waiting for `chunk_seconds` (spec 010). 0 = always use full chunks.
    #[serde(default = "default_min_chunk")]
    pub min_chunk_seconds: u32,
    /// Where meetings are stored. Defaults to `$XDG_DATA_HOME/fofoca/meetings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meetings_dir: Option<PathBuf>,
    pub whisper: WhisperConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub diarization: DiarizationConfig,
    #[serde(default)]
    pub audio: AudioConfig,
    #[serde(default)]
    pub workflow: Workflow,
}

/// GPU or CPU, for one model.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Device {
    #[default]
    Gpu,
    Cpu,
}

/// Where speech-to-text and the summary model run during a call (spec 015).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Layout {
    /// Both on the GPU, taking turns: Whisper is stopped while the summary model runs.
    #[default]
    Swap,
    GpuTranscript,
    GpuSummary,
    Cpu,
    /// Both loaded on the GPU at once (only when they fit).
    GpuBoth,
}

impl Layout {
    /// `(speech-to-text, summary)` devices.
    pub fn devices(self) -> (Device, Device) {
        match self {
            Layout::Swap | Layout::GpuBoth => (Device::Gpu, Device::Gpu),
            Layout::GpuTranscript => (Device::Gpu, Device::Cpu),
            Layout::GpuSummary => (Device::Cpu, Device::Gpu),
            Layout::Cpu => (Device::Cpu, Device::Cpu),
        }
    }
    /// Whisper must give the GPU up while the summary model runs.
    pub fn swaps(self) -> bool {
        self == Layout::Swap
    }
}

/// How fresh live text and the "summary so far" are, against how busy the computer is.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Updates {
    Fast,
    #[default]
    Balanced,
    Light,
    /// The raw `chunk_seconds`, `min_chunk_seconds` and `workflow.summary_every_s`.
    Custom,
}

/// What runs during and after a call (spec 015). Global: a recording uses the settings it started with.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(default)]
pub struct Workflow {
    pub live_transcript: bool,
    /// Needs `live_transcript`.
    pub live_summary: bool,
    pub layout: Layout,
    /// Where the single live task runs when only the transcript is live.
    pub live_device: Device,
    pub updates: Updates,
    /// Seconds between automatic summaries with `updates = "custom"`; 0 = only when asked.
    pub summary_every_s: u32,
    pub transcript_after: bool,
    pub speakers_after: bool,
    pub notes_after: bool,
    pub notes_device: Device,
}

impl Default for Workflow {
    fn default() -> Self {
        Workflow {
            live_transcript: true,
            live_summary: true,
            layout: Layout::Swap,
            live_device: Device::Gpu,
            updates: Updates::Balanced,
            summary_every_s: 300,
            transcript_after: true,
            speakers_after: true,
            notes_after: true,
            notes_device: Device::Gpu,
        }
    }
}

/// Chunking and summary cadence for a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pace {
    /// End a chunk at the first pause after this many seconds (0 = full chunks only, spec 010).
    pub min_chunk_s: u32,
    pub chunk_s: u32,
    /// Automatic "summary so far" interval; `None` = only when asked.
    pub summary_every_s: Option<u32>,
}

impl Config {
    /// Live summary only makes sense with a live transcript.
    pub fn live_summary(&self) -> bool {
        self.workflow.live_transcript && self.workflow.live_summary
    }

    /// `(speech-to-text, summary)` devices during a call.
    pub fn live_devices(&self) -> (Device, Device) {
        let w = &self.workflow;
        if self.live_summary() {
            w.layout.devices()
        } else {
            (w.live_device, w.notes_device)
        }
    }

    /// Whisper has to give the GPU up while the summary model runs during the call.
    pub fn live_swaps(&self) -> bool {
        let (asr, llm) = self.live_devices();
        asr == Device::Gpu && llm == Device::Gpu && !(self.live_summary() && self.workflow.layout == Layout::GpuBoth)
    }

    pub fn pace(&self) -> Pace {
        let every = |s: u32| if self.live_summary() && s > 0 { Some(s) } else { None };
        match self.workflow.updates {
            Updates::Fast => Pace { min_chunk_s: 8, chunk_s: 15, summary_every_s: every(180) },
            Updates::Balanced => Pace { min_chunk_s: 8, chunk_s: 30, summary_every_s: every(300) },
            Updates::Light => Pace { min_chunk_s: 0, chunk_s: 30, summary_every_s: None },
            Updates::Custom => Pace {
                min_chunk_s: self.min_chunk_seconds,
                chunk_s: self.chunk_seconds.max(5),
                summary_every_s: every(self.workflow.summary_every_s),
            },
        }
    }
}

/// How long meeting audio is kept (spec 011). Transcripts, notes and speakers are never deleted.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum KeepAudio {
    Always,
    /// Delete once speaker separation and the notes step have both succeeded.
    #[default]
    AfterProcessing,
    /// Delete `keep_days` after the meeting started.
    Days,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioConfig {
    pub keep: KeepAudio,
    pub keep_days: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig { keep: KeepAudio::AfterProcessing, keep_days: 30 }
    }
}

/// Speaker separation after the call (spec 003). Runs on CPU.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DiarizationConfig {
    pub segmentation_model: PathBuf,
    pub embedding_model: PathBuf,
    /// Cosine similarity above which two voices are the same person.
    pub threshold: f32,
}

impl Default for DiarizationConfig {
    fn default() -> Self {
        let models = data_dir().join("models");
        DiarizationConfig {
            segmentation_model: models.join("segmentation-3.0.onnx"),
            embedding_model: models.join("wespeaker_en_voxceleb_CAM++.onnx"),
            threshold: 0.5,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WhisperConfig {
    pub server: PathBuf,
    pub model: PathBuf,
    /// whisper.cpp GPU index — it counts discrete GPUs only (the MX230 is 0 next to an iGPU).
    pub device: u32,
    pub threads: u32,
    /// Silero VAD model. When present, Whisper skips non-speech: no hallucinated lines on
    /// silence, and ~10 s instead of ~18 s per 30 s speech chunk on the MX230.
    #[serde(default = "default_vad_model")]
    pub vad_model: Option<PathBuf>,
}

/// Measured on the 75 s pt-BR recording (MX230, turbo + VAD): line latency median 21.5 s → 12.7 s,
/// worst 30.7 s → 22.4 s versus fixed 30 s chunks, same WER, Whisper busy 70 % of real time.
fn default_min_chunk() -> u32 {
    8
}

fn default_vad_model() -> Option<PathBuf> {
    Some(data_dir().join("models/ggml-silero-v5.1.2.bin"))
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct LlmConfig {
    pub server: PathBuf,
    pub model: PathBuf,
    /// llama.cpp device name, e.g. `Vulkan1` (see `llama-server --list-devices`).
    pub device: String,
    pub context: u32,
    pub threads: u32,
}

impl Default for Config {
    fn default() -> Self {
        let data = data_dir();
        Config {
            language: "pt".into(),
            chunk_seconds: 30,
            min_chunk_seconds: default_min_chunk(),
            meetings_dir: None,
            whisper: WhisperConfig {
                server: data.join("bin/whisper/whisper-server"),
                model: data.join("models/ggml-large-v3-turbo-q5_0.bin"),
                device: 0,
                threads: 4,
                vad_model: default_vad_model(),
            },
            llm: LlmConfig {
                server: data.join("bin/llama/llama-server"),
                model: data.join("models/Qwen3.5-2B-Q4_K_M.gguf"),
                device: "Vulkan0".into(),
                context: 32768,
                threads: 4,
            },
            diarization: DiarizationConfig::default(),
            audio: AudioConfig::default(),
            workflow: Workflow::default(),
        }
    }
}

/// `$XDG_DATA_HOME/fofoca` (usually `~/.local/share/fofoca`).
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("fofoca")
}

/// `$XDG_CONFIG_HOME/fofoca/calendars.json` — calendar labels and hosts only; links are in the keyring.
pub fn calendars_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("fofoca/calendars.json")
}

/// `$XDG_CONFIG_HOME/fofoca/config.toml`.
pub fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("fofoca/config.toml")
}

impl Config {
    /// Load the config file, creating it with defaults if it doesn't exist.
    pub fn load_or_create() -> Result<Config> {
        Self::load_or_create_at(&config_path())
    }

    pub fn load_or_create_at(path: &Path) -> Result<Config> {
        if path.exists() {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let cfg: Config =
                toml::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
            return Ok(cfg.expanded());
        }
        let cfg = Config::default();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, toml::to_string_pretty(&cfg)?)?;
        Ok(cfg)
    }

    /// Change the `[audio]` table in the config file, keeping everything else (comments
    /// included) as the user wrote it.
    pub fn save_audio(path: &Path, audio: &AudioConfig) -> Result<()> {
        let text = std::fs::read_to_string(path).unwrap_or_default();
        let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
        let keep = match audio.keep {
            KeepAudio::Always => "always",
            KeepAudio::AfterProcessing => "after_processing",
            KeepAudio::Days => "days",
        };
        doc["audio"]["keep"] = toml_edit::value(keep);
        doc["audio"]["keep_days"] = toml_edit::value(audio.keep_days as i64);
        std::fs::write(path, doc.to_string()).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    /// Change the `[workflow]` table, keeping the rest of the file as written.
    pub fn save_workflow(path: &Path, w: &Workflow) -> Result<()> {
        edit_file(path, |doc| {
            let table: toml_edit::DocumentMut = toml::to_string(w)?.parse()?;
            doc["workflow"] = toml_edit::Item::Table(table.as_table().clone());
            Ok(())
        })
    }

    /// Point one model (or engine) setting at a file, keeping the rest of the file as written.
    pub fn save_path(path: &Path, setting: ModelSetting, file: &Path) -> Result<()> {
        let (table, key) = setting.key();
        edit_file(path, |doc| {
            if doc.get(table).is_none() {
                doc[table] = toml_edit::table();
            }
            doc[table][key] = toml_edit::value(file.display().to_string());
            Ok(())
        })
    }

    /// The in-memory copy of a path setting.
    pub fn set_path(&mut self, setting: ModelSetting, file: PathBuf) {
        match setting {
            ModelSetting::Whisper => self.whisper.model = file,
            ModelSetting::Vad => self.whisper.vad_model = Some(file),
            ModelSetting::Llm => self.llm.model = file,
            ModelSetting::Segmentation => self.diarization.segmentation_model = file,
            ModelSetting::Embedding => self.diarization.embedding_model = file,
            ModelSetting::WhisperServer => self.whisper.server = file,
            ModelSetting::LlmServer => self.llm.server = file,
        }
    }

    pub fn path_of(&self, setting: ModelSetting) -> Option<&Path> {
        Some(match setting {
            ModelSetting::Whisper => &self.whisper.model,
            ModelSetting::Vad => return self.whisper.vad_model.as_deref(),
            ModelSetting::Llm => &self.llm.model,
            ModelSetting::Segmentation => &self.diarization.segmentation_model,
            ModelSetting::Embedding => &self.diarization.embedding_model,
            ModelSetting::WhisperServer => &self.whisper.server,
            ModelSetting::LlmServer => &self.llm.server,
        })
    }

    pub fn meetings_dir(&self) -> PathBuf {
        self.meetings_dir.clone().unwrap_or_else(|| data_dir().join("meetings"))
    }

    /// Expand a leading `~/` in every path.
    fn expanded(mut self) -> Config {
        self.whisper.server = expand(&self.whisper.server);
        self.whisper.model = expand(&self.whisper.model);
        self.whisper.vad_model = self.whisper.vad_model.map(|p| expand(&p));
        self.llm.server = expand(&self.llm.server);
        self.llm.model = expand(&self.llm.model);
        self.meetings_dir = self.meetings_dir.map(|p| expand(&p));
        self
    }

    /// Model name shown in the UI and stored with summaries.
    pub fn llm_name(&self) -> String {
        model_name(&self.llm.model)
    }

    pub fn whisper_name(&self) -> String {
        model_name(&self.whisper.model)
    }
}

/// A file setting the Models tab can change (spec 015).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum ModelSetting {
    Whisper,
    Vad,
    Llm,
    Segmentation,
    Embedding,
    WhisperServer,
    LlmServer,
}

impl ModelSetting {
    fn key(self) -> (&'static str, &'static str) {
        match self {
            ModelSetting::Whisper => ("whisper", "model"),
            ModelSetting::Vad => ("whisper", "vad_model"),
            ModelSetting::Llm => ("llm", "model"),
            ModelSetting::Segmentation => ("diarization", "segmentation_model"),
            ModelSetting::Embedding => ("diarization", "embedding_model"),
            ModelSetting::WhisperServer => ("whisper", "server"),
            ModelSetting::LlmServer => ("llm", "server"),
        }
    }
}

fn edit_file(path: &Path, f: impl FnOnce(&mut toml_edit::DocumentMut) -> Result<()>) -> Result<()> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let mut doc: toml_edit::DocumentMut = text.parse().with_context(|| format!("parsing {}", path.display()))?;
    f(&mut doc)?;
    std::fs::write(path, doc.to_string()).with_context(|| format!("writing {}", path.display()))
}

fn model_name(p: &Path) -> String {
    p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn expand(p: &Path) -> PathBuf {
    match (p.strip_prefix("~"), dirs::home_dir()) {
        (Ok(rest), Some(home)) => home.join(rest),
        _ => p.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_then_reads_back_with_tilde_expansion() {
        let dir = std::env::temp_dir().join(format!("fofoca-cfg-{}", std::process::id()));
        let path = dir.join("config.toml");
        let _ = std::fs::remove_dir_all(&dir);
        let created = Config::load_or_create_at(&path).unwrap();
        assert!(path.exists());
        assert_eq!(created.chunk_seconds, 30);

        let mut text = std::fs::read_to_string(&path).unwrap();
        text = text.replace(&created.llm.model.display().to_string(), "~/models/x.gguf");
        std::fs::write(&path, text).unwrap();
        let read = Config::load_or_create_at(&path).unwrap();
        assert_eq!(read.llm.model, dirs::home_dir().unwrap().join("models/x.gguf"));
        assert_eq!(read.llm_name(), "x");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn saving_the_audio_policy_keeps_the_rest_of_the_file() {
        let dir = std::env::temp_dir().join(format!("fofoca-cfg-audio-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        Config::load_or_create_at(&path).unwrap();
        let text = format!("# my notes about this machine\n{}", std::fs::read_to_string(&path).unwrap());
        std::fs::write(&path, text).unwrap();
        Config::save_audio(&path, &AudioConfig { keep: KeepAudio::Days, keep_days: 7 }).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with("# my notes about this machine"), "{saved}");
        let cfg = Config::load_or_create_at(&path).unwrap();
        assert_eq!(cfg.audio, AudioConfig { keep: KeepAudio::Days, keep_days: 7 });
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn workflow_defaults_pace_and_save() {
        let dir = std::env::temp_dir().join(format!("fofoca-cfg-wf-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        // an older file without [workflow] reads as today's behaviour
        Config::load_or_create_at(&path).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let old: String = text.split("[workflow]").next().unwrap().to_string();
        std::fs::write(&path, format!("# keep me\n{old}")).unwrap();
        let cfg = Config::load_or_create_at(&path).unwrap();
        assert_eq!(cfg.workflow, Workflow::default());
        assert!(cfg.workflow.transcript_after);
        assert!(toml::from_str::<Workflow>("speakers_after = false").unwrap().transcript_after);
        assert_eq!(cfg.pace(), Pace { min_chunk_s: 8, chunk_s: 30, summary_every_s: Some(300) });
        assert_eq!(cfg.live_devices(), (Device::Gpu, Device::Gpu));

        let w = Workflow { transcript_after: false, live_summary: false, live_device: Device::Cpu, updates: Updates::Light, notes_device: Device::Cpu, ..Workflow::default() };
        Config::save_workflow(&path, &w).unwrap();
        Config::save_path(&path, ModelSetting::Segmentation, Path::new("/m/seg.onnx")).unwrap();
        let saved = std::fs::read_to_string(&path).unwrap();
        assert!(saved.starts_with("# keep me"), "{saved}");
        let cfg = Config::load_or_create_at(&path).unwrap();
        assert_eq!(cfg.workflow, w);
        assert_eq!(cfg.diarization.segmentation_model, PathBuf::from("/m/seg.onnx"));
        assert_eq!(cfg.pace().summary_every_s, None);
        assert_eq!(cfg.live_devices(), (Device::Cpu, Device::Cpu));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn live_summary_needs_the_transcript() {
        let mut cfg = Config::default();
        cfg.workflow.live_transcript = false;
        assert!(!cfg.live_summary());
        assert_eq!(cfg.pace().summary_every_s, None);
        cfg.workflow = Workflow { layout: Layout::GpuTranscript, updates: Updates::Fast, ..Workflow::default() };
        assert_eq!(cfg.live_devices(), (Device::Gpu, Device::Cpu));
        assert!(!cfg.workflow.layout.swaps());
        assert_eq!(cfg.pace(), Pace { min_chunk_s: 8, chunk_s: 15, summary_every_s: Some(180) });
    }
}
