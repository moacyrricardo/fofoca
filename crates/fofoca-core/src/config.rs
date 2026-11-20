//! User configuration (`$XDG_CONFIG_HOME/fofoca/config.toml`).
//!
//! Spec 001 fixes the layout to "swap"; choosing models and devices automatically is spec 002.

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
}