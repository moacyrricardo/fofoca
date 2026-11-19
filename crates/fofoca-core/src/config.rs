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
    /// Length of each transcription chunk.
    pub chunk_seconds: u32,
    /// Where meetings are stored. Defaults to `$XDG_DATA_HOME/fofoca/meetings`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub meetings_dir: Option<PathBuf>,
    pub whisper: WhisperConfig,
    pub llm: LlmConfig,
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
}
