//! Models are not part of the app (spec 015): the user points at files they have, or downloads a
//! suggested one into the models folder. Downloads stream to `<file>.part`, are checked by size,
//! then renamed, so a half-downloaded model is never used.

use anyhow::{bail, Context, Result};
use serde::Serialize;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::config::{data_dir, Config, ModelSetting};

/// The task a model serves; one row in Settings → Models.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Whisper,
    Vad,
    Llm,
    Speakers,
}

#[derive(Serialize, Clone, Debug)]
pub struct CatalogFile {
    pub setting: ModelSetting,
    pub file: &'static str,
    pub url: &'static str,
    pub bytes: u64,
}

/// A suggested model: one or more files that are downloaded together.
#[derive(Serialize, Clone, Debug)]
pub struct Entry {
    pub id: &'static str,
    pub task: Task,
    pub name: &'static str,
    pub note: &'static str,
    pub files: Vec<CatalogFile>,
}

impl Entry {
    pub fn bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

/// Suggested models. Sizes are the exact file sizes (checked after download).
pub fn catalog() -> Vec<Entry> {
    use ModelSetting::*;
    let f = |setting, file, url, bytes| CatalogFile { setting, file, url, bytes };
    vec![
        Entry { id: "whisper-turbo", task: Task::Whisper, name: "Whisper large-v3-turbo · q5_0", note: "best for pt-BR (~2 % errors on the test recording)",
            files: vec![f(Whisper, "ggml-large-v3-turbo-q5_0.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo-q5_0.bin", 574_041_195)] },
        Entry { id: "whisper-small", task: Task::Whisper, name: "Whisper small · q5_1", note: "~3× faster, ~5× more errors in pt-BR",
            files: vec![f(Whisper, "ggml-small-q5_1.bin", "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small-q5_1.bin", 190_085_487)] },
        Entry { id: "silero-vad", task: Task::Vad, name: "Silero VAD v5.1.2", note: "skips silence: no invented lines, faster chunks",
            files: vec![f(Vad, "ggml-silero-v5.1.2.bin", "https://huggingface.co/ggml-org/whisper-vad/resolve/main/ggml-silero-v5.1.2.bin", 885_098)] },
        Entry { id: "qwen3.5-2b", task: Task::Llm, name: "Qwen3.5-2B · Q4_K_M", note: "~17 tokens/s on a 2 GB GPU",
            files: vec![f(Llm, "Qwen3.5-2B-Q4_K_M.gguf", "https://huggingface.co/unsloth/Qwen3.5-2B-GGUF/resolve/main/Qwen3.5-2B-Q4_K_M.gguf", 1_280_835_840)] },
        Entry { id: "qwen3-1.7b", task: Task::Llm, name: "Qwen3-1.7B · Q4_K_M", note: "a bit faster, fits less context",
            files: vec![f(Llm, "Qwen3-1.7B-Q4_K_M.gguf", "https://huggingface.co/unsloth/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q4_K_M.gguf", 1_107_409_472)] },
        Entry { id: "pyannote", task: Task::Speakers, name: "segmentation-3.0 + WeSpeaker CAM++", note: "runs on the CPU",
            files: vec![
                f(Segmentation, "segmentation-3.0.onnx", "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/segmentation-3.0.onnx", 5_983_836),
                f(Embedding, "wespeaker_en_voxceleb_CAM++.onnx", "https://github.com/thewh1teagle/pyannote-rs/releases/download/v0.1.0/wespeaker_en_voxceleb_CAM++.onnx", 29_292_684),
            ] },
    ]
}

pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Where a catalog file is on this computer: the configured path if it has that name, else the
/// models folder. `None` when it isn't on disk.
pub fn locate(cfg: &Config, f: &CatalogFile, dir: &Path) -> Option<PathBuf> {
    if let Some(p) = cfg.path_of(f.setting).filter(|p| p.file_name().is_some_and(|n| n == f.file) && p.is_file()) {
        return Some(p.to_path_buf());
    }
    Some(dir.join(f.file)).filter(|p| p.is_file())
}

/// Every file of the entry is on this computer.
pub fn present(cfg: &Config, e: &Entry, dir: &Path) -> bool {
    e.files.iter().all(|f| locate(cfg, f, dir).is_some())
}

/// The catalog entry the current settings point at, if any.
pub fn current<'a>(cfg: &Config, catalog: &'a [Entry], task: Task) -> Option<&'a Entry> {
    catalog.iter().filter(|e| e.task == task).find(|e| {
        e.files.iter().all(|f| cfg.path_of(f.setting).and_then(|p| p.file_name()).is_some_and(|n| n == f.file))
    })
}

/// Download one file to `dest`, reporting `(bytes so far, total)`. Stops (and removes the partial
/// file) when `cancel` is set. Fails if the size isn't `bytes`.
pub fn download_file(url: &str, dest: &Path, bytes: u64, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<()> {
    if dest.metadata().is_ok_and(|m| m.len() == bytes) {
        progress(bytes, bytes);
        return Ok(());
    }
    if let Some(dir) = dest.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let part = dest.with_extension(format!("{}.part", dest.extension().map(|e| e.to_string_lossy()).unwrap_or_default()));
    let result = (|| -> Result<()> {
        let http = reqwest::blocking::Client::builder().user_agent("fo-foca").connect_timeout(Duration::from_secs(20)).timeout(None).build()?;
        let mut resp = http.get(url).send().with_context(|| format!("downloading {url}"))?.error_for_status()?;
        let mut out = std::fs::File::create(&part).with_context(|| format!("writing {}", part.display()))?;
        let (mut done, mut buf, mut last) = (0u64, vec![0u8; 256 * 1024], 0u64);
        loop {
            if cancel.load(Ordering::Relaxed) {
                bail!("cancelled");
            }
            let n = resp.read(&mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            done += n as u64;
            if done - last > bytes / 200 {
                last = done;
                progress(done, bytes);
            }
        }
        out.flush()?;
        if done != bytes {
            bail!("got {done} bytes, expected {bytes} — the download was incomplete or the file changed");
        }
        std::fs::rename(&part, dest)?;
        progress(bytes, bytes);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&part);
    }
    result
}

/// Download every file of a catalog entry into `dir`; returns where each setting's file is.
pub fn download(e: &Entry, dir: &Path, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<Vec<(ModelSetting, PathBuf)>> {
    let total = e.bytes();
    let mut before = 0;
    let mut out = Vec::new();
    for f in &e.files {
        let dest = dir.join(f.file);
        download_file(f.url, &dest, f.bytes, &|d, _| progress(before + d, total), cancel)?;
        before += f.bytes;
        out.push((f.setting, dest));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    /// Serve `body` once over HTTP on 127.0.0.1; returns the URL.
    fn serve(body: Vec<u8>) -> String {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/model.bin", l.local_addr().unwrap());
        std::thread::spawn(move || {
            if let Ok((mut s, _)) = l.accept() {
                let mut req = [0u8; 1024];
                let _ = s.read(&mut req);
                let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = s.write_all(&body);
            }
        });
        url
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("fofoca-models-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn downloads_checks_size_and_cancels() {
        let dir = tmp("dl");
        let body = vec![7u8; 600_000];
        let dest = dir.join("m.bin");
        let seen = std::sync::Mutex::new(0u64);
        download_file(&serve(body.clone()), &dest, 600_000, &|d, _| *seen.lock().unwrap() = d, &AtomicBool::new(false)).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(*seen.lock().unwrap(), 600_000);

        // wrong size: nothing is left behind
        let other = dir.join("bad.bin");
        assert!(download_file(&serve(vec![1u8; 10]), &other, 11, &|_, _| {}, &AtomicBool::new(false)).is_err());
        assert!(!other.exists() && !dir.join("bad.bin.part").exists());

        // cancelled before it starts
        let c = dir.join("c.bin");
        let err = download_file(&serve(vec![1u8; 10]), &c, 10, &|_, _| {}, &AtomicBool::new(true)).unwrap_err();
        assert!(format!("{err}").contains("cancelled") && !c.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn finds_catalog_files_and_the_current_entry() {
        let dir = tmp("cat");
        std::fs::create_dir_all(&dir).unwrap();
        let cat = catalog();
        let mut cfg = Config::default();
        // keep the test off the real models folder
        cfg.set_path(ModelSetting::Segmentation, dir.join("segmentation-3.0.onnx"));
        cfg.set_path(ModelSetting::Embedding, dir.join("wespeaker_en_voxceleb_CAM++.onnx"));
        cfg.set_path(ModelSetting::Whisper, dir.join("ggml-small-q5_1.bin"));
        assert_eq!(current(&cfg, &cat, Task::Whisper).map(|e| e.id), Some("whisper-small"));
        let small = cat.iter().find(|e| e.id == "whisper-small").unwrap();
        assert!(!present(&cfg, small, &dir));
        std::fs::write(dir.join("ggml-small-q5_1.bin"), b"x").unwrap();
        assert!(present(&cfg, small, &dir));
        let spk = cat.iter().find(|e| e.id == "pyannote").unwrap();
        std::fs::write(dir.join("segmentation-3.0.onnx"), b"x").unwrap();
        assert!(!present(&cfg, spk, &dir), "both speaker files are needed");
        cfg.set_path(ModelSetting::Whisper, dir.join("my-own.bin"));
        assert!(current(&cfg, &cat, Task::Whisper).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
