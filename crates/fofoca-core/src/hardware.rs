//! What this computer can run (spec 015): the GPU fo-foca would use, model footprints on it,
//! and a suggested layout. Speed benchmarks on first run are spec 002.

use serde::Serialize;
use std::path::Path;
use std::process::Command;

use crate::config::{Config, Layout};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct Gpu {
    /// llama.cpp device name, e.g. `Vulkan1`.
    pub device: String,
    pub name: String,
    pub total_mib: u32,
    pub free_mib: u32,
}

/// The configured `[llm].device` as `llama-server --list-devices` reports it, or `None` when it
/// isn't there (no usable GPU, or the server can't run).
pub fn probe(cfg: &Config) -> Option<Gpu> {
    let out = Command::new(&cfg.llm.server).arg("--list-devices").output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout).into_owned() + &String::from_utf8_lossy(&out.stderr);
    parse_devices(&text).into_iter().find(|g| g.device == cfg.llm.device)
}

/// Parse lines like `  Vulkan1: NVIDIA GeForce MX230 (2294 MiB, 2234 MiB free)`.
pub fn parse_devices(text: &str) -> Vec<Gpu> {
    text.lines()
        .filter_map(|l| {
            let (device, rest) = l.trim().split_once(": ")?;
            if device.contains(' ') {
                return None;
            }
            let open = rest.rfind(" (")?;
            let (name, nums) = (rest[..open].trim(), rest[open + 2..].trim_end_matches(')'));
            let (total, free) = nums.split_once(", ")?;
            let mib = |s: &str| s.split_whitespace().next()?.parse::<u32>().ok();
            Some(Gpu { device: device.into(), name: name.into(), total_mib: mib(total)?, free_mib: mib(free)? })
        })
        .collect()
}

/// GPU memory a model needs once loaded.
#[derive(Serialize, Clone, Copy, Debug, PartialEq)]
pub struct Footprint {
    pub mib: u32,
    /// Measured on the MX230 (benchmarks-mx230) rather than estimated from the file size.
    pub measured: bool,
}

/// Measured footprints by file name; anything else is estimated as file size × 1.15.
pub fn footprint(model: &Path) -> Option<Footprint> {
    let name = model.file_name()?.to_string_lossy();
    let measured = match name.as_ref() {
        "ggml-large-v3-turbo-q5_0.bin" => Some(824),
        "Qwen3.5-2B-Q4_K_M.gguf" => Some(1414), // at 8K context; the session uses more with longer meetings
        _ => None,
    };
    if let Some(mib) = measured {
        return Some(Footprint { mib, measured: true });
    }
    let bytes = std::fs::metadata(model).ok()?.len();
    Some(Footprint { mib: (bytes as f64 * 1.15 / 1_048_576.0).round() as u32, measured: false })
}

/// Do both models fit on the GPU at once (with a 5 % margin)?
pub fn both_fit(gpu: &Gpu, whisper: Footprint, llm: Footprint) -> bool {
    (whisper.mib + llm.mib) as f64 <= gpu.total_mib as f64 * 0.95
}

/// The layout fo-foca suggests for this computer.
pub fn suggest(gpu: Option<&Gpu>, whisper: Option<Footprint>, llm: Option<Footprint>) -> Layout {
    match (gpu, whisper, llm) {
        (None, _, _) => Layout::Cpu,
        (Some(g), Some(w), Some(l)) if both_fit(g, w, l) => Layout::GpuBoth,
        _ => Layout::Swap,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST: &str = "load_backend: loaded Vulkan backend\nAvailable devices:\n  Vulkan0: Intel(R) UHD Graphics (CML GT2) (23429 MiB, 4920 MiB free)\n  Vulkan1: NVIDIA GeForce MX230 (2294 MiB, 2234 MiB free)\n";

    #[test]
    fn parses_list_devices() {
        let d = parse_devices(LIST);
        assert_eq!(d.len(), 2);
        assert_eq!(d[1], Gpu { device: "Vulkan1".into(), name: "NVIDIA GeForce MX230".into(), total_mib: 2294, free_mib: 2234 });
        assert_eq!(d[0].name, "Intel(R) UHD Graphics (CML GT2)");
    }

    #[test]
    fn suggests_swap_on_the_mx230_and_both_on_a_bigger_gpu() {
        let w = footprint(Path::new("ggml-large-v3-turbo-q5_0.bin")).unwrap();
        let l = footprint(Path::new("Qwen3.5-2B-Q4_K_M.gguf")).unwrap();
        assert!(w.measured && l.measured);
        let mx230 = &parse_devices(LIST)[1];
        assert!(!both_fit(mx230, w, l));
        assert_eq!(suggest(Some(mx230), Some(w), Some(l)), Layout::Swap);
        let big = Gpu { total_mib: 8192, ..mx230.clone() };
        assert_eq!(suggest(Some(&big), Some(w), Some(l)), Layout::GpuBoth);
        assert_eq!(suggest(None, Some(w), Some(l)), Layout::Cpu);
        assert_eq!(suggest(Some(&big), Some(w), None), Layout::Swap);
    }

    #[test]
    fn estimates_unknown_models_from_their_size() {
        let p = std::env::temp_dir().join(format!("fofoca-fp-{}.gguf", std::process::id()));
        std::fs::write(&p, vec![0u8; 2 * 1_048_576]).unwrap();
        assert_eq!(footprint(&p), Some(Footprint { mib: 2, measured: false }));
        std::fs::remove_file(&p).unwrap();
        assert_eq!(footprint(Path::new("/nope/missing.gguf")), None);
    }
}
