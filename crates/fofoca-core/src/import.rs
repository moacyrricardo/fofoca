//! Recordings made elsewhere (spec 012): probe with ffprobe, convert with ffmpeg to the 16 kHz
//! mono WAV everything else uses.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::{Command, Stdio};

pub struct ImportInfo {
    pub duration_s: f64,
    /// RFC 3339, from the file's `creation_time` tag if it has one.
    pub created_at: Option<String>,
}

fn tool(name: &str) -> Command {
    let mut c = Command::new(name);
    c.stdin(Stdio::null());
    crate::proc::die_with_parent(&mut c);
    c
}

pub fn probe(path: &Path) -> Result<ImportInfo> {
    let out = tool("ffprobe")
        .args(["-v", "error", "-print_format", "json", "-show_format"])
        .arg(path)
        .output()
        .context("running ffprobe (is ffmpeg installed?)")?;
    if !out.status.success() {
        bail!("can't read {}: {}", path.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    parse_probe(&String::from_utf8_lossy(&out.stdout))
}

fn parse_probe(json: &str) -> Result<ImportInfo> {
    let v: serde_json::Value = serde_json::from_str(json).context("ffprobe output")?;
    let f = &v["format"];
    let duration_s = f["duration"].as_str().and_then(|d| d.parse().ok()).context("the file has no duration — is it audio?")?;
    let created_at = f["tags"]["creation_time"]
        .as_str()
        .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
        .map(|t| t.with_timezone(&chrono::Local).to_rfc3339());
    Ok(ImportInfo { duration_s, created_at })
}

/// Any audio (or video) ffmpeg reads → 16 kHz mono s16 WAV.
pub fn convert(src: &Path, dst: &Path) -> Result<()> {
    let out = tool("ffmpeg")
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(src)
        .args(["-vn", "-ac", "1", "-ar", "16000", "-sample_fmt", "s16", "-f", "wav"])
        .arg(dst)
        .output()
        .context("running ffmpeg (is it installed?)")?;
    if !out.status.success() {
        bail!("ffmpeg couldn't convert {}: {}", src.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_duration_and_creation_time() {
        let i = parse_probe(r#"{"format":{"duration":"75.240000","tags":{"creation_time":"2026-11-18T13:30:00.000000Z"}}}"#).unwrap();
        assert!((i.duration_s - 75.24).abs() < 1e-9);
        let t = chrono::DateTime::parse_from_rfc3339(&i.created_at.unwrap()).unwrap();
        assert_eq!(t.timestamp(), chrono::DateTime::parse_from_rfc3339("2026-11-18T13:30:00Z").unwrap().timestamp());
        assert!(parse_probe(r#"{"format":{}}"#).is_err());
        assert!(parse_probe(r#"{"format":{"duration":"3.0"}}"#).unwrap().created_at.is_none());
    }

    #[test]
    fn converts_with_ffmpeg() {
        if Command::new("ffmpeg").arg("-version").output().is_err() {
            return; // ffmpeg not installed here
        }
        let dir = std::env::temp_dir().join(format!("fofoca-import-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // 1 s stereo 44.1 kHz tone → 16 kHz mono
        let src = dir.join("tone.ogg");
        let ok = tool("ffmpeg")
            .args(["-nostdin", "-v", "error", "-y", "-f", "lavfi", "-i", "sine=frequency=440:duration=1", "-ac", "2", "-ar", "44100"])
            .arg(&src)
            .status()
            .unwrap()
            .success();
        if !ok {
            return; // no ogg encoder in this ffmpeg build
        }
        let dst = dir.join("out.wav");
        convert(&src, &dst).unwrap();
        let samples = crate::audio::read_wav(&dst).unwrap();
        assert!((15_000..17_000).contains(&samples.len()), "{}", samples.len());
        assert!((probe(&src).unwrap().duration_s - 1.0).abs() < 0.1);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
