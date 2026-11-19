//! Client for whisper.cpp's `whisper-server` `/inference` endpoint.

use anyhow::{Context, Result};
use serde::Deserialize;
use std::time::Duration;

use crate::model::Word;

/// A segment as returned by Whisper, with chunk-relative timestamps.
#[derive(Debug, Clone, PartialEq)]
pub struct RawSegment {
    pub start: f64,
    pub end: f64,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Deserialize)]
struct VerboseJson {
    #[serde(default)]
    segments: Vec<JsonSegment>,
}

#[derive(Deserialize)]
struct JsonSegment {
    start: f64,
    end: f64,
    text: String,
    #[serde(default)]
    words: Vec<JsonWord>,
}

#[derive(Deserialize)]
struct JsonWord {
    word: String,
    start: f64,
    end: f64,
    #[serde(default)]
    probability: f32,
}

pub struct WhisperClient {
    base_url: String,
    http: reqwest::blocking::Client,
}

impl WhisperClient {
    pub fn new(base_url: &str) -> Result<Self> {
        Ok(Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            http: reqwest::blocking::Client::builder().timeout(Duration::from_secs(600)).build()?,
        })
    }

    /// Transcribe one WAV chunk. `prompt` primes spelling (names, jargon, previous text).
    ///
    /// Uses the `vtt` format: segment timestamps at full speed. `verbose_json` adds token-level
    /// timestamps but measured ~45 % slower on the MX230 (26 s vs 18 s per 30 s chunk), which
    /// breaks the live budget — so word timings here are interpolated (see [`approx_words`]).
    pub fn transcribe(&self, wav: Vec<u8>, language: &str, prompt: &str) -> Result<Vec<RawSegment>> {
        let part = reqwest::blocking::multipart::Part::bytes(wav)
            .file_name("chunk.wav")
            .mime_str("audio/wav")?;
        let mut form = reqwest::blocking::multipart::Form::new()
            .part("file", part)
            .text("language", language.to_string())
            .text("response_format", "vtt");
        if !prompt.trim().is_empty() {
            form = form.text("prompt", prompt.to_string());
        }
        let body = self
            .http
            .post(format!("{}/inference", self.base_url))
            .multipart(form)
            .send()
            .context("calling whisper-server")?
            .error_for_status()?
            .text()?;
        Ok(parse_vtt(&body))
    }
}

/// Parse WebVTT cues into segments with interpolated word timings.
pub fn parse_vtt(body: &str) -> Vec<RawSegment> {
    let mut out = Vec::new();
    let mut lines = body.lines().peekable();
    while let Some(line) = lines.next() {
        let Some((a, b)) = line.split_once("-->") else { continue };
        let (Some(start), Some(end)) = (vtt_time(a), vtt_time(b)) else { continue };
        let mut text = String::new();
        while let Some(l) = lines.peek() {
            if l.trim().is_empty() {
                break;
            }
            text.push(' ');
            text.push_str(l.trim());
            lines.next();
        }
        let text = text.trim().to_string();
        if text.is_empty() || is_noise(&text) {
            continue;
        }
        out.push(RawSegment { start, end, words: approx_words(start, end, &text), text });
    }
    out
}

fn vtt_time(s: &str) -> Option<f64> {
    let s = s.split_whitespace().next()?;
    let parts: Vec<&str> = s.split(':').collect();
    let secs: f64 = parts.last()?.replace(',', ".").parse().ok()?;
    let mut total = secs;
    let mut mult = 60.0;
    for p in parts.iter().rev().skip(1) {
        total += p.parse::<f64>().ok()? * mult;
        mult *= 60.0;
    }
    Some(total)
}

/// Spread a segment's duration over its words in proportion to their length.
/// `p` is 0 to mark the timings as interpolated, not measured.
pub fn approx_words(start: f64, end: f64, text: &str) -> Vec<Word> {
    let words: Vec<&str> = text.split_whitespace().collect();
    let total: usize = words.iter().map(|w| w.chars().count() + 1).sum();
    if total == 0 {
        return Vec::new();
    }
    let span = (end - start).max(0.0);
    let mut t = start;
    words
        .iter()
        .map(|w| {
            let d = span * (w.chars().count() + 1) as f64 / total as f64;
            let word = Word { start: t, end: t + d, text: format!(" {w}"), p: 0.0 };
            t += d;
            word
        })
        .collect()
}

/// Parse whisper-server's `verbose_json`, dropping empty segments and punctuation-only words'
/// leading spaces.
pub fn parse_verbose_json(body: &str) -> Result<Vec<RawSegment>> {
    let v: VerboseJson = serde_json::from_str(body).context("parsing whisper verbose_json")?;
    Ok(v.segments
        .into_iter()
        .map(|s| RawSegment {
            start: s.start,
            end: s.end,
            text: s.text.trim().to_string(),
            words: s
                .words
                .into_iter()
                .map(|w| Word { start: w.start, end: w.end, text: w.word, p: w.probability })
                .collect(),
        })
        .filter(|s| !s.text.is_empty() && !is_noise(&s.text))
        .collect())
}

/// Whisper emits bracketed non-speech markers on silence.
fn is_noise(text: &str) -> bool {
    let t = text.trim();
    (t.starts_with('[') && t.ends_with(']')) || (t.starts_with('(') && t.ends_with(')'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vtt_cues() {
        let body = "WEBVTT\n\n00:00:00.000 --> 00:00:11.880\n Bom dia a todos.\n\n\
                    00:00:12.680 --> 00:00:18.440\n [BLANK_AUDIO]\n\n\
                    00:01:02.500 --> 00:01:04.000\n linha um\n linha dois\n";
        let s = parse_vtt(body);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].text, "Bom dia a todos.");
        assert!((s[1].start - 62.5).abs() < 1e-9);
        assert_eq!(s[1].text, "linha um linha dois");
    }

    #[test]
    fn approx_words_cover_the_segment() {
        let w = approx_words(10.0, 14.0, "Bom dia a todos");
        assert_eq!(w.len(), 4);
        assert!((w[0].start - 10.0).abs() < 1e-9);
        assert!((w[3].end - 14.0).abs() < 1e-9);
        assert!(w.windows(2).all(|p| (p[0].end - p[1].start).abs() < 1e-9));
    }

    #[test]
    fn parses_segments_and_words() {
        let body = r#"{"task":"transcribe","language":"portuguese","duration":30.0,
          "segments":[
            {"id":0,"text":" Bom dia a todos.","start":0.0,"end":6.66,
             "words":[{"word":" Bom","start":0.0,"end":1.12,"t_dtw":-1,"probability":0.75},
                      {"word":" dia","start":1.12,"end":2.24,"t_dtw":-1,"probability":0.99}]},
            {"id":1,"text":" [BLANK_AUDIO]","start":6.66,"end":10.0,"words":[]},
            {"id":2,"text":" ","start":10.0,"end":11.0}
          ]}"#;
        let s = parse_verbose_json(body).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].text, "Bom dia a todos.");
        assert_eq!(s[0].words.len(), 2);
        assert_eq!(s[0].words[1].text, " dia");
        assert!((s[0].end - 6.66).abs() < 1e-9);
    }
}
