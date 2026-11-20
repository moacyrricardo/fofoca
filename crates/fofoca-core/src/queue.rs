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
//!
//! Without headphones the mic hears the call. A mic slice that is much quieter than call audio
//! not transcribed yet is held until that call audio has its lines (Decision 6), so the echo check
//! (`tracks::drop_echoes`) sees both texts.

use crate::audio::{self, SAMPLE_RATE};
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

/// A mic slice is held as a likely echo when call audio without text yet covers at least this
/// share of it…
const ECHO_OVERLAP: f64 = 0.5;
/// …and the mic is below this share of the call's level there. Your own voice is 30–800× louder
/// on the mic than the call is (measured, spec 018); an echo is quieter than the call.
const ECHO_LEVEL: f64 = 0.25;

const SR: f64 = SAMPLE_RATE as f64;
/// Level windows, as the slicer's (50 ms).
const WIN: usize = SAMPLE_RATE as usize / 20;

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
    /// Mic slices held back as a likely echo of the call.
    pub held_echo: usize,
}

/// Ready slices from both tracks, waiting for the Whisper worker.
#[derive(Default)]
pub struct SliceQueue {
    ready: Vec<Slice>,
    /// Mic slices that sound like an echo of call audio without text yet; they wait for it.
    held: Vec<Slice>,
    /// Call slices queued or being transcribed: `(start_s, end_s)`.
    call_pending: Vec<(f64, f64)>,
    /// Where the call slicer's open or held audio starts (it runs on to now).
    call_open_from: Option<f64>,
    /// The call track's 50 ms levels from t = 0, and samples short of a full window.
    call_levels: Vec<f32>,
    call_rest: Vec<i16>,
    pub stats: QueueStats,
}

impl SliceQueue {
    /// Queue a closed slice. Call slices are fed before the mic slices of the same moment, so a
    /// mic slice can be compared with them.
    pub fn push(&mut self, s: Slice) {
        match s.track {
            Track::You => self.stats.you_slices += 1,
            Track::Call => self.stats.call_slices += 1,
        }
        if s.track == Track::Call {
            self.call_pending.push((s.start_s, s.end_s));
        } else if self.sounds_like_echo(&s) {
            self.stats.held_echo += 1;
            self.held.push(s);
            return;
        }
        self.ready.push(s);
    }

    /// The call track as it is captured (all of it, from t = 0), for the echo levels.
    pub fn call_audio(&mut self, samples: &[i16]) {
        self.call_rest.extend_from_slice(samples);
        let whole = self.call_rest.len() / WIN * WIN;
        self.call_levels.extend(self.call_rest[..whole].chunks(WIN).map(audio::rms));
        self.call_rest.drain(..whole);
    }

    /// Where the call slicer's audio not yet queued starts (`Slicer::pending_from`).
    pub fn set_call_open(&mut self, from_s: Option<f64>) {
        self.call_open_from = from_s;
    }

    /// `req` has been transcribed (or failed): its call audio has its lines now, so mic slices
    /// waiting on it can go.
    pub fn done(&mut self, req: &Request) {
        if req.track == Track::Call {
            self.call_pending.retain(|&(a, b)| a < req.start_s - 1e-9 || b > req.end_s + 1e-9);
        }
    }

    /// Call audio without text yet: queued or in-flight slices, and the open one.
    fn call_waiting(&self) -> Vec<(f64, f64)> {
        self.call_pending.iter().copied().chain(self.call_open_from.map(|f| (f, f64::INFINITY))).collect()
    }

    /// Whether mic slice `m` overlaps call audio without text yet by ≥ 50 % of its length while
    /// the mic is < 0.25× the call's level over that overlap.
    fn sounds_like_echo(&self, m: &Slice) -> bool {
        let waiting = self.call_waiting();
        let (mut windows, mut overlap, mut mic, mut call) = (0usize, 0usize, 0.0f64, 0.0f64);
        for (i, w) in m.samples.chunks(WIN).enumerate() {
            windows += 1;
            let t = m.start_s + (i as f64 + 0.5) * WIN as f64 / SR;
            if !waiting.iter().any(|&(a, b)| t >= a && t < b) {
                continue;
            }
            overlap += 1;
            let Some(&c) = self.call_levels.get((t * SR) as usize / WIN) else { continue };
            mic += (audio::rms(w) as f64).powi(2);
            call += (c as f64).powi(2);
        }
        overlap as f64 >= ECHO_OVERLAP * windows as f64 && call > 0.0 && (mic / call).sqrt() < ECHO_LEVEL
    }

    /// Held mic slices whose call audio all has its lines go to the ready ones.
    fn release(&mut self) {
        let waiting = self.call_waiting();
        let (go, stay): (Vec<Slice>, Vec<Slice>) = self.held.drain(..).partition(|m| !waiting.iter().any(|&(a, b)| m.start_s < b && a < m.end_s));
        self.held = stay;
        self.ready.extend(go);
    }

    /// The next request: the ready slice with the earliest start, joined with the next queued
    /// slices of the same track when more than `backlog_s` of audio is queued. `None` if nothing
    /// is ready.
    pub fn next(&mut self, backlog_s: f64, merge_gap_s: f64, merge_max_s: f64) -> Option<Request> {
        self.release();
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

    /// Seconds of audio waiting, held echoes included.
    pub fn queued_s(&self) -> f64 {
        self.ready.iter().chain(&self.held).map(Slice::len_s).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ready.is_empty() && self.held.is_empty()
    }
}

/// Send one request to `transcribe` (given the samples and the prompt) and return its lines in
/// meeting time, without mic lines (or the ends of mic lines) that only repeat the call lines
/// already in `lines`.
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
        assert_eq!(q.stats, QueueStats { you_slices: 1, call_slices: 2, requests: 3, merged: 0, held_echo: 0 });
    }

    #[test]
    fn a_long_call_turn_finishing_late_is_still_placed_at_its_start() {
        // the call talks 2–19 s; you say a short thing at 5–7 s, which closes first
        let mut q = SliceQueue::default();
        let mut lines: Vec<Segment> = Vec::new();
        q.push(slice(Track::You, 5.0, 7.0));
        let you = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).unwrap();
        lines.extend(transcribe(&you, &lines, |s, _| Ok(fake_whisper(s, &["Uhum, sim."]))).unwrap().0);
        q.push(slice(Track::Call, 2.0, 19.0));
        let call = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).unwrap();
        for l in transcribe(&call, &lines, |s, _| Ok(fake_whisper(s, &["Então, a proposta é a seguinte."]))).unwrap().0 {
            assert_eq!(tracks::insert_by_start(&mut lines, l), 0, "goes before your reply");
        }
        assert_eq!(lines.iter().map(|l| (l.track, l.start)).collect::<Vec<_>>(), vec![(Track::Call, 2.0), (Track::You, 5.0)]);
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

    /// Voice-like audio at `amp` over `a..b` within a track of `len` s (150 ms syllables).
    fn voiced(len: f64, a: f64, b: f64, amp: i16) -> Vec<i16> {
        (0..(len * SR) as usize)
            .map(|i| {
                let t = i as f64 / SR;
                if t >= a && t < b && i % (SRU / 4) < SRU * 15 / 100 { if i % 2 == 0 { amp } else { -amp } } else { 0 }
            })
            .collect()
    }

    fn cut(track: Track, samples: &[i16], a: f64, b: f64) -> Slice {
        Slice { track, start_s: a, end_s: b, samples: samples[(a * SR) as usize..(b * SR) as usize].to_vec() }
    }

    #[test]
    fn an_echo_waits_for_the_call_text_and_is_dropped() {
        // the call speaks at 10–13 s; the mic hears it through the speakers at −20 dB
        let call = voiced(20.0, 10.0, 13.0, 6000);
        let mic = voiced(20.0, 10.0, 13.0, 600);
        let mut q = SliceQueue::default();
        q.call_audio(&call);
        q.push(cut(Track::Call, &call, 9.8, 13.2));
        q.push(cut(Track::You, &mic, 9.9, 13.3));
        assert_eq!(q.stats.held_echo, 1);
        assert!((q.queued_s() - 6.8).abs() < 1e-9, "held audio is backlog too");
        let text = "A gente fecha o relatório na sexta.";
        let mut lines: Vec<Segment> = Vec::new();
        let call_req = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).unwrap();
        assert_eq!(call_req.track, Track::Call);
        assert!(q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).is_none(), "the mic waits while the call is transcribed");
        lines.extend(transcribe(&call_req, &lines, |s, _| Ok(fake_whisper(s, &[text]))).unwrap().0);
        q.done(&call_req);
        let mic_req = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).expect("released once the call has its text");
        let (kept, echoes) = transcribe(&mic_req, &lines, |s, _| Ok(fake_whisper(s, &["a gente fecha o relatório na sexta"]))).unwrap();
        assert!(kept.is_empty());
        assert_eq!(echoes, 1);
        assert!(q.is_empty());
    }

    #[test]
    fn a_real_reply_over_a_quiet_call_is_not_held() {
        // they murmur at −20 dB on the call while you answer, loud, on the mic
        let call = voiced(20.0, 10.0, 13.0, 600);
        let mic = voiced(20.0, 10.5, 13.0, 6000);
        let mut q = SliceQueue::default();
        q.call_audio(&call);
        q.push(cut(Track::Call, &call, 9.8, 13.2));
        q.push(cut(Track::You, &mic, 10.3, 13.2));
        assert_eq!(q.stats.held_echo, 0);
        // a quiet mic with no call audio pending (already transcribed) isn't held either
        let mut q = SliceQueue::default();
        q.call_audio(&voiced(20.0, 10.0, 13.0, 6000));
        q.push(cut(Track::You, &voiced(20.0, 10.0, 13.0, 600), 9.9, 13.3));
        assert_eq!(q.stats.held_echo, 0);
    }

    #[test]
    fn a_held_mic_slice_waits_for_the_open_call_slice() {
        let call = voiced(20.0, 9.0, 20.0, 6000);
        let mic = voiced(20.0, 10.0, 12.0, 600);
        let mut q = SliceQueue::default();
        q.call_audio(&call);
        q.set_call_open(Some(8.8)); // the call is still talking
        q.push(cut(Track::You, &mic, 9.8, 12.2));
        assert!(q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).is_none());
        // the call slice closes and is queued: it goes first, then the mic
        q.set_call_open(None);
        q.push(cut(Track::Call, &call, 8.8, 20.0));
        let first = q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).unwrap();
        assert_eq!(first.track, Track::Call);
        q.done(&first);
        assert_eq!(q.next(BACKLOG_S, MERGE_GAP_S, MERGE_MAX_S).unwrap().track, Track::You);
    }

    #[test]
    fn a_turn_glued_to_an_echo_keeps_the_turn() {
        // Whisper wrote the echo of their line and your answer as one mic line
        let call = Segment { start: 10.0, end: 13.0, text: "A gente fecha o relatório na sexta.".into(), track: Track::Call, words: vec![], speaker: None };
        let req = Request { track: Track::You, start_s: 10.0, end_s: 16.0, samples: vec![1; 6 * SRU], slices: 1 };
        let text = "a gente fecha o relatório na sexta combinado então eu mando amanhã";
        let (kept, echoes) = transcribe(&req, std::slice::from_ref(&call), |_, _| Ok(vec![RawSegment { start: 0.2, end: 6.0, text: text.into(), words: crate::whisper::approx_words(0.2, 6.0, text) }])).unwrap();
        assert_eq!(echoes, 1);
        assert_eq!(kept.len(), 1, "your words survive");
        assert_eq!(kept[0].text, "combinado então eu mando amanhã");
        assert!(kept[0].start > 12.0 && (kept[0].end - 16.0).abs() < 1e-9);
        assert_eq!(kept[0].words.len(), 5);
    }
}
