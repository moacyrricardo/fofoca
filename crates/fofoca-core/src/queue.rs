//! One queue for both tracks' slices and one Whisper worker (spec 019).
//!
//! ```text
//! mic slicer ──┐                      ┌─ earliest start first ─▶ one Whisper request ─▶ lines
//!              ├─▶ ready slices ──────┤
//! call slicer ─┘                      └─ behind (> backlog_s queued)? join the next slices of the
//!                                        same track (gaps ≤ merge_gap_s, ≤ merge_max_s in all)
//! ```
//!
//! A Whisper request costs about the same however short its audio (the encoder always runs over a
//! 30 s window), so with no backlog small slices go out alone for low latency, and when behind
//! they are merged to catch up.

use crate::audio::SAMPLE_RATE;
use crate::model::{Segment, Track};
use crate::slicer::Slice;
use crate::tracks;
use crate::whisper::RawSegment;

/// More queued audio than this means the worker is behind and merges (Decision 4).
pub const BACKLOG_S: f64 = 20.0;
/// Merged slices are at most this far apart…
pub const MERGE_GAP_S: f64 = 3.0;
/// …and span at most this much, under Whisper's 30 s window.
pub const MERGE_MAX_S: f64 = 28.0;
/// As `backlog_s`: merge whenever possible (the after-call rebuild has no latency to protect).
pub const ALWAYS_MERGE: f64 = f64::NEG_INFINITY;

const SR: f64 = SAMPLE_RATE as f64;

/// One Whisper request: one slice, or several of one track joined.
#[derive(Debug, Clone, PartialEq)]
pub struct Request {
    pub track: Track,
    pub start_s: f64,
    pub end_s: f64,
    /// The slices' audio with the gaps between them filled with silence, so a time in the request
    /// is `start_s` later in the meeting.
    pub samples: Vec<i16>,
    /// How many slices went in (more than 1 = merged).
    pub slices: usize,
}

impl Request {
    /// Whisper's lines for this request, labelled with its track, in meeting time.
    pub fn lines(&self, raw: Vec<RawSegment>) -> Vec<Segment> {
        raw.into_iter().map(|r| tracks::segment(r, self.track, self.start_s)).collect()
    }
}

/// Counts for the benchmark and the log.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct QueueStats {
    pub you_slices: usize,
    pub call_slices: usize,
    pub requests: usize,
    /// Requests made of more than one slice.
    pub merged: usize,
}

/// Ready slices from both tracks, waiting for the Whisper worker.
#[derive(Default)]
pub struct SliceQueue {
    ready: Vec<Slice>,
    pub stats: QueueStats,
}

impl SliceQueue {
    pub fn push(&mut self, s: Slice) {
        match s.track {
            Track::You => self.stats.you_slices += 1,
            Track::Call => self.stats.call_slices += 1,
        }
        self.ready.push(s);
    }

    /// The next request: the ready slice with the earliest start, joined with the next queued
    /// slices of the same track when more than `backlog_s` of audio is queued. `None` if nothing
    /// is ready.
    pub fn next(&mut self, backlog_s: f64, merge_gap_s: f64, merge_max_s: f64) -> Option<Request> {
        let behind = self.queued_s() > backlog_s;
        let first = (0..self.ready.len()).min_by(|&a, &b| self.ready[a].start_s.total_cmp(&self.ready[b].start_s))?;
        let s = self.ready.remove(first);
        let mut req = Request { track: s.track, start_s: s.start_s, end_s: s.end_s, samples: s.samples, slices: 1 };
        if behind {
            loop {
                let next = (0..self.ready.len())
                    .filter(|&i| self.ready[i].track == req.track && self.ready[i].start_s >= req.end_s - 1e-9)
                    .min_by(|&a, &b| self.ready[a].start_s.total_cmp(&self.ready[b].start_s));
                let Some(i) = next.filter(|&i| {
                    let s = &self.ready[i];
                    s.start_s - req.end_s <= merge_gap_s && s.end_s - req.start_s <= merge_max_s
                }) else {
                    break;
                };
                let s = self.ready.remove(i);
                let at = ((s.start_s - req.start_s) * SR).round() as usize;
                req.samples.resize(at.max(req.samples.len()), 0);
                req.samples.extend(s.samples);
                req.end_s = s.end_s;
                req.slices += 1;
            }
        }
        self.stats.requests += 1;
        self.stats.merged += usize::from(req.slices > 1);
        Some(req)
    }

    /// Seconds of audio waiting.
    pub fn queued_s(&self) -> f64 {
        self.ready.iter().map(Slice::len_s).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ready.is_empty()
    }
}

/// Send one request to `transcribe` (given the samples and the prompt) and return its lines in
/// meeting time, without mic lines that only repeat the call lines already in `lines`.
pub fn transcribe(
    req: &Request,
    lines: &[Segment],
    transcribe: impl FnOnce(&[i16], &str) -> anyhow::Result<Vec<RawSegment>>,
) -> anyhow::Result<(Vec<Segment>, usize)> {
    let prompt = tracks::tail_chars(&lines.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join(" "), 200);
    let new = req.lines(transcribe(&req.samples, &prompt)?);
    Ok(tracks::drop_echoes(new, lines))
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRU: usize = SAMPLE_RATE as usize;

    fn slice(track: Track, a: f64, b: f64) -> Slice {
        Slice { track, start_s: a, end_s: b, samples: vec![1; ((b - a) * SR).round() as usize] }
    }

    fn take_all(q: &mut SliceQueue, backlog_s: f64) -> Vec<(Track, f64, f64, usize)> {
        std::iter::from_fn(|| q.next(backlog_s, MERGE_GAP_S, MERGE_MAX_S)).map(|r| (r.track, r.start_s, r.end_s, r.slices)).collect()
    }

    #[test]
    fn earliest_start_goes_first() {
        let mut q = SliceQueue::default();
        q.push(slice(Track::You, 12.0, 15.0));
        q.push(slice(Track::Call, 4.0, 11.0));
        q.push(slice(Track::Call, 16.0, 20.0));
        assert_eq!(q.queued_s(), 14.0);
        assert_eq!(take_all(&mut q, BACKLOG_S), vec![(Track::Call, 4.0, 11.0, 1), (Track::You, 12.0, 15.0, 1), (Track::Call, 16.0, 20.0, 1)]);
        assert!(q.is_empty() && q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).is_none());
        assert_eq!(q.stats, QueueStats { you_slices: 1, call_slices: 2, requests: 3, merged: 0 });
    }

    #[test]
    fn merges_only_when_behind_and_only_the_same_track() {
        let queue = || {
            let mut q = SliceQueue::default();
            for (t, a, b) in [(Track::Call, 0.0, 8.0), (Track::You, 8.5, 9.5), (Track::Call, 10.0, 18.0), (Track::Call, 19.0, 26.0)] {
                q.push(slice(t, a, b));
            }
            q
        };
        // 24 s queued, not behind at 30 s: each slice alone
        assert_eq!(take_all(&mut queue(), 30.0).len(), 4);
        // behind: the call slices join across your "uhum" (another track), gaps of 2 s and 1 s
        let mut q = queue();
        let r = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).unwrap();
        assert_eq!((r.track, r.start_s, r.end_s, r.slices), (Track::Call, 0.0, 26.0, 3));
        assert_eq!(r.samples.len(), 26 * SRU, "gaps filled");
        assert!(r.samples[8 * SRU..10 * SRU].iter().all(|&s| s == 0), "with silence");
        assert_eq!(take_all(&mut q, BACKLOG_S), vec![(Track::You, 8.5, 9.5, 1)]);
        assert_eq!(q.stats.merged, 1);
    }

    #[test]
    fn merged_gaps_and_length_stay_under_the_limits() {
        let mut q = SliceQueue::default();
        // a 4 s gap is too far; then 3 slices of 10 s with 1 s gaps exceed 28 s together
        for (a, b) in [(0.0, 5.0), (9.0, 19.0), (20.0, 30.0), (31.0, 41.0)] {
            q.push(slice(Track::Call, a, b));
        }
        let got = take_all(&mut q, ALWAYS_MERGE);
        assert_eq!(got, vec![(Track::Call, 0.0, 5.0, 1), (Track::Call, 9.0, 30.0, 2), (Track::Call, 31.0, 41.0, 1)]);
    }

    /// A stand-in for Whisper: one line over each voiced stretch (non-zero samples, gaps under
    /// 300 ms bridged) of the request, in request time.
    fn fake_whisper(samples: &[i16], texts: &[&str]) -> Vec<RawSegment> {
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (i, w) in samples.chunks(SRU / 20).enumerate() {
            if !w.iter().any(|&s| s != 0) {
                continue;
            }
            match runs.last_mut() {
                Some(r) if i - r.1 < 6 => r.1 = i + 1,
                _ => runs.push((i, i + 1)),
            }
        }
        runs.iter()
            .zip(texts)
            .map(|(&(a, b), t)| RawSegment { start: a as f64 / 20.0, end: b as f64 / 20.0, text: t.to_string(), words: crate::whisper::approx_words(a as f64 / 20.0, b as f64 / 20.0, t) })
            .collect()
    }

    #[test]
    fn merged_request_lines_map_back_to_meeting_time() {
        let mut q = SliceQueue::default();
        q.push(slice(Track::Call, 100.0, 104.0));
        q.push(slice(Track::Call, 106.5, 110.0));
        let req = q.next(ALWAYS_MERGE, MERGE_GAP_S, MERGE_MAX_S).unwrap();
        assert_eq!(req.slices, 2);
        let (lines, echoes) = transcribe(&req, &[], |s, prompt| {
            assert_eq!(prompt, "");
            Ok(fake_whisper(s, &["Primeira parte.", "Segunda parte."]))
        })
        .unwrap();
        assert_eq!(echoes, 0);
        let got: Vec<(f64, f64, Track)> = lines.iter().map(|l| (l.start, l.end, l.track)).collect();
        assert_eq!(got, vec![(100.0, 104.0, Track::Call), (106.5, 110.0, Track::Call)]);
        assert!(lines[1].words.iter().all(|w| w.start >= 106.5 && w.end <= 110.0 + 1e-9));
    }

    /// The bug that started spec 018: the call speaks at 0–2 s and you at 2–3 s. Cut per track,
    /// each turn is its own slice and its own line, with its own label.
    #[test]
    fn turn_taking_keeps_your_words_as_you() {
        // 150 ms syllables, 100 ms gaps
        let voice = |a: f64, b: f64| -> Vec<i16> {
            let on = |i: usize| (i as f64 / SR) >= a && (i as f64 / SR) < b && i % (SRU / 4) < SRU * 15 / 100;
            (0..4 * SRU).map(|i| if on(i) { if i % 2 == 0 { 6000 } else { -6000 } } else { 0 }).collect()
        };
        let (call, mic) = (voice(0.0, 2.0), voice(2.0, 3.0));
        let cfg = crate::slicer::SlicerConfig { min_s: 0.5, max_s: 20.0, hold_s: 0.0, pause_ms: 350 };
        let mut q = SliceQueue::default();
        for (track, samples) in [(Track::Call, &call), (Track::You, &mic)] {
            let mut s = crate::slicer::Slicer::new(track, cfg);
            let mut out = s.push(60.0, samples);
            out.extend(s.finish());
            out.into_iter().for_each(|x| q.push(x));
        }
        let mut lines: Vec<Segment> = Vec::new();
        while let Some(req) = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S) {
            let text = if req.track == Track::You { "tudo certo?" } else { "Oi, como vai?" };
            lines.extend(transcribe(&req, &lines, |s, _| Ok(fake_whisper(s, &[text]))).unwrap().0);
        }
        let got: Vec<(Track, &str, f64)> = lines.iter().map(|s| (s.track, s.text.as_str(), s.start.round())).collect();
        assert_eq!(got, vec![(Track::Call, "Oi, como vai?", 60.0), (Track::You, "tudo certo?", 62.0)]);
    }
}
