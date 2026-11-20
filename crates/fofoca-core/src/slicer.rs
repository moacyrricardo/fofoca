//! Cutting one track into slices at its own pauses (spec 019).
//!
//! ```text
//! track ─▶ 50 ms windows ─▶ loud? (relative to a rolling noise floor)
//!            silence ........ never sliced
//!            speech ──▶ open slice ──▶ closes at a ≥ 350 ms pause, or at max_s (quietest point)
//!                                       shorter than min_s ─▶ held: joins the next slice within
//!                                                             hold_s, else goes alone after it
//! ```
//!
//! Each track is cut on its own, so a slice holds one person's turn: people take turns ~4.5 times
//! a minute, and cutting both tracks at the same points (spec 018) sent both tracks almost every
//! time. Whisper costs about the same per request however short the audio, so silence is never
//! sent and short "uhum"s wait to ride along with the next turn.

use std::collections::VecDeque;

use crate::audio::{self, SAMPLE_RATE};
use crate::config::{Config, Updates};
use crate::model::Track;

const SR: usize = SAMPLE_RATE as usize;
/// Analysis window, as in `audio::has_speech`.
const WIN: usize = SR / 20;
/// Audio kept before the first and after the last loud window, so word edges aren't clipped.
const EDGE: usize = SR / 5;
/// The noise floor is the 10th percentile of this many windows (~30 s) of the same track.
const FLOOR_WINDOWS: usize = 600;
/// Fewer loud windows than this (300 ms, as `has_speech`) is a click, not speech.
const MIN_VOICED: usize = 6;
/// A slice cut at `max_s` ends at the quietest point in its last 3 s.
const CUT_SEARCH: usize = 3 * SR;

/// One track's speech between two of its own pauses.
#[derive(Debug, Clone, PartialEq)]
pub struct Slice {
    pub track: Track,
    /// Meeting time, s.
    pub start_s: f64,
    pub end_s: f64,
    pub samples: Vec<i16>,
}

impl Slice {
    pub fn len_s(&self) -> f64 {
        self.end_s - self.start_s
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SlicerConfig {
    /// A slice shorter than this when its track pauses is held (Decision 2).
    pub min_s: f64,
    /// Longest slice; cut at the quietest point in the last 3 s.
    pub max_s: f64,
    /// How long a held short slice waits for the track's next one; 0 = it goes alone right away.
    pub hold_s: f64,
    /// Quiet this long ends a slice.
    pub pause_ms: u32,
}

impl SlicerConfig {
    /// Live slicing for 015's "Live updates" setting. `Custom` keeps its raw chunk length as
    /// `max_s`, with Balanced's `min_s`/`hold_s` fitted under it.
    pub fn live(updates: Updates, chunk_seconds: u32) -> Self {
        let (min_s, max_s, hold_s) = match updates {
            Updates::Fast => (3.0, 12.0, 8.0),
            Updates::Balanced => (4.0, 20.0, 15.0),
            Updates::Light => (6.0, 28.0, 25.0),
            Updates::Custom => {
                let max = chunk_seconds.max(5) as f64;
                (4.0f64.min(max / 2.0), max, 15.0f64.min(max * 0.75))
            }
        };
        SlicerConfig { min_s, max_s, hold_s, pause_ms: 350 }
    }

    pub fn for_config(cfg: &Config) -> Self {
        Self::live(cfg.workflow.updates, cfg.chunk_seconds)
    }

    /// After the call (and imports): no latency to protect, so no holding; the queue merges short
    /// slices into their neighbours instead (§6).
    pub fn rebuild() -> Self {
        SlicerConfig { min_s: 6.0, max_s: crate::queue::MERGE_MAX_S, hold_s: 0.0, pause_ms: 350 }
    }
}

/// A slice that has started and not yet closed. Positions are track samples from t = 0.
struct Open {
    start: usize,
    /// End of the last loud window.
    voice_end: usize,
    voiced: usize,
    quiet_run: usize,
}

/// A closed slice shorter than `min_s`, still in the buffer, waiting for the next one.
struct Held {
    start: usize,
    end: usize,
    voiced: usize,
}

/// Turns one track into slices as its audio arrives.
pub struct Slicer {
    track: Track,
    cfg: SlicerConfig,
    /// Track audio from sample `buf_at` up to `end`, not yet given out.
    buf: Vec<i16>,
    buf_at: usize,
    end: usize,
    /// Windows up to here have been looked at.
    scanned: usize,
    started: bool,
    levels: VecDeque<f32>,
    open: Option<Open>,
    held: Option<Held>,
}

impl Slicer {
    pub fn new(track: Track, cfg: SlicerConfig) -> Self {
        Slicer { track, cfg, buf: Vec::new(), buf_at: 0, end: 0, scanned: 0, started: false, levels: VecDeque::new(), open: None, held: None }
    }

    /// Feed samples captured since the last call (`t_s` = track time of the first one). Returns
    /// slices that became ready. Audio is expected to follow on; a jump closes what's open first.
    pub fn push(&mut self, t_s: f64, samples: &[i16]) -> Vec<Slice> {
        let at = (t_s * SR as f64).round() as usize;
        let mut out = Vec::new();
        if !self.started || at.abs_diff(self.end) > 1 {
            if self.started {
                out = self.finish();
            }
            self.started = true;
            (self.buf_at, self.end, self.scanned) = (at, at, at);
            self.buf.clear();
        }
        self.buf.extend_from_slice(samples);
        self.end += samples.len();
        while self.scanned + WIN <= self.end {
            let w = self.scanned;
            self.scanned += WIN;
            self.window(w, &mut out);
        }
        out
    }

    /// End of recording (or a pause): close and return whatever is open or held.
    pub fn finish(&mut self) -> Vec<Slice> {
        let mut out = Vec::new();
        if let Some(o) = self.open.take() {
            let end = (o.voice_end + EDGE).min(self.end);
            if o.voiced >= MIN_VOICED {
                out.push(self.cut(o.start, end));
            }
        } else if let Some(h) = self.held.take() {
            out.push(self.cut(h.start, h.end));
        }
        self.held = None;
        self.drop_before(self.end);
        self.scanned = self.end;
        out
    }

    /// Where this track's audio that hasn't become a slice yet (open or held) starts, s.
    pub fn pending_from(&self) -> Option<f64> {
        let start = self.held.as_ref().map(|h| h.start).or(self.open.as_ref().map(|o| o.start))?;
        Some(start as f64 / SR as f64)
    }

    /// Seconds of open or held audio (part of the backlog the UI shows).
    pub fn pending_s(&self) -> f64 {
        self.pending_from().map(|f| (self.end as f64 / SR as f64 - f).max(0.0)).unwrap_or(0.0)
    }

    fn window(&mut self, w: usize, out: &mut Vec<Slice>) {
        let level = audio::rms(&self.buf[w - self.buf_at..w + WIN - self.buf_at]);
        self.levels.push_back(level);
        if self.levels.len() > FLOOR_WINDOWS {
            self.levels.pop_front();
        }
        let loud = level > audio::quiet_level(self.levels.iter().copied());
        let now = w + WIN;
        let (hold, min, max) = (secs(self.cfg.hold_s), secs(self.cfg.min_s), secs(self.cfg.max_s));
        let Some(o) = self.open.as_mut() else {
            if loud {
                // a held short slice joins this one when it is recent enough
                let (start, voiced) = match self.held.take() {
                    Some(h) if w.saturating_sub(h.end) <= hold => (h.start, h.voiced),
                    Some(h) => {
                        out.push(self.cut(h.start, h.end));
                        (w.saturating_sub(EDGE).max(self.buf_at), 0)
                    }
                    None => (w.saturating_sub(EDGE).max(self.buf_at), 0),
                };
                self.open = Some(Open { start, voice_end: now, voiced: voiced + 1, quiet_run: 0 });
            } else {
                if let Some(h) = self.held.as_ref().filter(|h| now - h.end >= hold) {
                    let (a, b) = (h.start, h.end);
                    self.held = None;
                    out.push(self.cut(a, b));
                }
                if self.held.is_none() {
                    self.drop_before(now.saturating_sub(EDGE));
                }
            }
            return;
        };
        if loud {
            o.voiced += 1;
            o.voice_end = now;
            o.quiet_run = 0;
        } else {
            o.quiet_run += 1;
        }
        if o.quiet_run * WIN >= self.cfg.pause_ms as usize * SR / 1000 {
            let (start, end, voiced) = (o.start, (o.voice_end + EDGE).min(now), o.voiced);
            self.open = None;
            if voiced < MIN_VOICED {
                self.drop_before(end); // a click, not speech
            } else if end - start < min && hold > 0 {
                self.held = Some(Held { start, end, voiced });
            } else {
                out.push(self.cut(start, end));
            }
        } else if now - o.start >= max {
            // never longer than max_s: end at the quietest point of the last 3 s, not mid-word
            let start = o.start;
            self.open = None;
            let span = &self.buf[start - self.buf_at..now - self.buf_at];
            let at = start + audio::quiet_cut(span, &[], span.len() - 1, CUT_SEARCH).max(1);
            out.push(self.cut(start, at));
            // the rest goes on as the next slice (we're mid-speech, so it counts as voiced)
            self.open = Some(Open { start: at, voice_end: now, voiced: MIN_VOICED, quiet_run: 0 });
        }
    }

    /// Give out track samples `a..b` as a slice and forget everything before `b`.
    fn cut(&mut self, a: usize, b: usize) -> Slice {
        let samples = self.buf[a - self.buf_at..b - self.buf_at].to_vec();
        self.drop_before(b);
        Slice { track: self.track, start_s: a as f64 / SR as f64, end_s: b as f64 / SR as f64, samples }
    }

    /// Forget audio before track sample `p` (never what an open or held slice still needs).
    fn drop_before(&mut self, p: usize) {
        let keep = [Some(p), self.open.as_ref().map(|o| o.start), self.held.as_ref().map(|h| h.start)].into_iter().flatten().min().unwrap_or(p);
        let keep = keep.clamp(self.buf_at, self.end);
        self.buf.drain(..keep - self.buf_at);
        self.buf_at = keep;
    }
}

fn secs(s: f64) -> usize {
    (s * SR as f64).round() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    const BALANCED: SlicerConfig = SlicerConfig { min_s: 4.0, max_s: 20.0, hold_s: 15.0, pause_ms: 350 };

    /// Mic-like noise (~0.003 RMS) with voice over `turns`: 150 ms syllables with 100 ms gaps, and
    /// a 200 ms breath every 2 s (with the syllable gap, still shorter than a pause).
    fn track(secs: f64, turns: &[(f64, f64)]) -> Vec<i16> {
        let (on, period) = (SR * 15 / 100, SR / 4);
        (0..(secs * SR as f64) as usize)
            .map(|i| {
                let t = i as f64 / SR as f64;
                let noise = ((i * 7919) % 301) as i16 - 150;
                let speaking = turns.iter().any(|&(a, b)| t >= a && t < b && (t - a) % 2.0 < 1.8);
                if speaking && i % period < on { noise.saturating_add(if i % 2 == 0 { 6000 } else { -6000 }) } else { noise }
            })
            .collect()
    }

    /// Feed `samples` in 100 ms pieces, as the session does, then finish if asked.
    fn slice(cfg: SlicerConfig, samples: &[i16], finish: bool) -> Vec<Slice> {
        let mut s = Slicer::new(Track::You, cfg);
        let mut out = Vec::new();
        for (i, piece) in samples.chunks(SR / 10).enumerate() {
            out.extend(s.push(i as f64 / 10.0, piece));
        }
        if finish {
            out.extend(s.finish());
        }
        out
    }

    fn spans(v: &[Slice]) -> Vec<(f64, f64)> {
        v.iter().map(|s| ((s.start_s * 10.0).round() / 10.0, (s.end_s * 10.0).round() / 10.0)).collect()
    }

    #[test]
    fn one_turn_is_one_slice_with_its_edges() {
        let out = slice(BALANCED, &track(16.0, &[(3.0, 13.0)]), false);
        assert_eq!(out.len(), 1, "{:?}", spans(&out));
        let s = &out[0];
        assert!((2.7..=3.0).contains(&s.start_s), "starts just before the voice: {}", s.start_s);
        assert!((12.6..=13.3).contains(&s.end_s), "ends just after it: {}", s.end_s);
        assert_eq!(s.samples.len(), ((s.end_s - s.start_s) * SR as f64).round() as usize);
        assert_eq!(s.track, Track::You);
    }

    #[test]
    fn three_turns_with_pauses_are_three_slices() {
        let out = slice(BALANCED, &track(22.0, &[(1.0, 6.0), (7.0, 12.0), (13.0, 19.0)]), false);
        assert_eq!(out.len(), 3, "{:?}", spans(&out));
        assert!(out.windows(2).all(|w| w[0].end_s <= w[1].start_s), "no audio twice");
    }

    #[test]
    fn a_monologue_is_cut_at_quiet_points_under_max_s() {
        let samples = track(44.0, &[(1.0, 41.0)]);
        let out = slice(BALANCED, &samples, true);
        assert!((2..=3).contains(&out.len()), "{:?}", spans(&out));
        assert!(out.iter().all(|s| s.len_s() <= 20.0 + 1e-9));
        assert!(out.windows(2).all(|w| (w[0].end_s - w[1].start_s).abs() < 1e-9), "cut pieces follow on");
        // each cut falls in a gap between syllables, not mid-syllable
        for s in &out[..out.len() - 1] {
            let at = (s.end_s * SR as f64) as usize;
            assert!(audio::rms(&samples[at - SR / 50..at + SR / 50]) < 0.01, "cut at {:.2} s is quiet", s.end_s);
        }
    }

    #[test]
    fn a_short_uhum_joins_the_next_turn() {
        let out = slice(BALANCED, &track(16.0, &[(1.0, 1.7), (6.5, 12.0)]), false);
        assert_eq!(out.len(), 1, "{:?}", spans(&out));
        assert!(out[0].start_s < 1.0 && out[0].end_s > 11.8, "{:?}", spans(&out));
    }

    #[test]
    fn a_lone_uhum_goes_alone_after_hold_s() {
        let samples = track(30.0, &[(1.0, 1.7)]);
        // not yet: 10 s after it closed
        assert!(slice(BALANCED, &samples[..12 * SR], false).is_empty(), "held");
        let out = slice(BALANCED, &samples, false);
        assert_eq!(out.len(), 1);
        assert!(out[0].len_s() < 1.5);
        // rebuild mode doesn't hold at all
        let out = slice(SlicerConfig::rebuild(), &samples[..4 * SR], false);
        assert_eq!(out.len(), 1, "no hold after the call");
    }

    #[test]
    fn silence_and_noise_make_no_slices() {
        assert!(slice(BALANCED, &[0; 20 * SR], true).is_empty());
        assert!(slice(BALANCED, &track(20.0, &[]), true).is_empty());
        // a 100 ms click is not speech
        let mut click = track(10.0, &[]);
        click[5 * SR..5 * SR + SR / 10].iter_mut().enumerate().for_each(|(i, s)| *s = if i % 2 == 0 { 6000 } else { -6000 });
        assert!(slice(BALANCED, &click, true).is_empty());
    }

    #[test]
    fn finish_flushes_open_and_held_slices() {
        // still talking when the recording stops
        assert!(slice(BALANCED, &track(8.0, &[(2.0, 8.0)]), false).is_empty(), "still open");
        let out = slice(BALANCED, &track(8.0, &[(2.0, 8.0)]), true);
        assert_eq!(out.len(), 1);
        assert!(out[0].end_s > 7.5 && out[0].end_s <= 8.0, "{:?}", spans(&out));
        // a held uhum
        assert!(slice(BALANCED, &track(5.0, &[(1.0, 1.7)]), false).is_empty(), "held");
        assert_eq!(slice(BALANCED, &track(5.0, &[(1.0, 1.7)]), true).len(), 1);
        // the slicer goes on after a finish (pause → resume)
        let mut s = Slicer::new(Track::Call, BALANCED);
        let mut first = s.push(0.0, &track(8.0, &[(1.0, 7.0)]));
        first.extend(s.finish());
        assert_eq!(first.len(), 1);
        assert!(s.finish().is_empty());
        let mut second = s.push(8.0, &track(8.0, &[(1.0, 7.0)]));
        second.extend(s.finish());
        assert_eq!(second.len(), 1);
        assert!(second[0].start_s > 8.5 && second[0].track == Track::Call);
    }

    #[test]
    fn pending_audio_is_reported() {
        let mut s = Slicer::new(Track::Call, BALANCED);
        assert_eq!(s.pending_from(), None);
        s.push(0.0, &track(5.0, &[(2.0, 5.0)]));
        let from = s.pending_from().expect("open");
        assert!((1.7..=2.0).contains(&from));
        assert!((s.pending_s() - (5.0 - from)).abs() < 1e-9);
    }

    #[test]
    fn live_settings_follow_live_updates() {
        assert_eq!(SlicerConfig::live(Updates::Fast, 30), SlicerConfig { min_s: 3.0, max_s: 12.0, hold_s: 8.0, pause_ms: 350 });
        assert_eq!(SlicerConfig::live(Updates::Balanced, 30), BALANCED);
        assert_eq!(SlicerConfig::live(Updates::Light, 30).max_s, 28.0);
        let c = SlicerConfig::live(Updates::Custom, 10);
        assert_eq!((c.min_s, c.max_s, c.hold_s), (4.0, 10.0, 7.5));
    }
}
