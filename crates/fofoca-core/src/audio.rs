//! Audio helpers. All audio in fo-foca is 16 kHz mono signed 16-bit.

use anyhow::{bail, Context, Result};
use std::io::Cursor;
use std::path::Path;

pub const SAMPLE_RATE: u32 = 16_000;

/// Root-mean-square level of a block, in 0.0..=1.0.
pub fn rms(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| (s as f64 / i16::MAX as f64).powi(2)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Pick where to end a chunk: the quietest 100 ms frame within the last `search` samples before
/// `target`, so a fixed cut doesn't split a word (measured: "deu ghost" → "de Augusto" at a hard
/// 30 s cut). Returns the cut index (≤ `target`). Both tracks count — the louder one decides.
pub fn quiet_cut(a: &[i16], b: &[i16], target: usize, search: usize) -> usize {
    const FRAME: usize = (SAMPLE_RATE / 10) as usize;
    let len = a.len().max(b.len());
    if len <= target {
        return len;
    }
    let from = target.saturating_sub(search);
    let frame_level = |start: usize| {
        let seg = |v: &[i16]| rms(&v[start.min(v.len())..(start + FRAME).min(v.len())]);
        seg(a).max(seg(b))
    };
    let mut best = (target, f32::MAX);
    let mut start = from;
    while start + FRAME <= target {
        let l = frame_level(start);
        if l < best.1 {
            best = (start + FRAME / 2, l);
        }
        start += FRAME;
    }
    best.0
}

/// Find a natural pause to end a chunk early (spec 010): the first run of ≥ 350 ms of relative
/// quiet that starts at or after `min` samples, searching up to `max`. Returns the cut index (the
/// middle of the quiet run) or `None` if nobody paused yet.
///
/// "Quiet" is relative to this buffer: twice its noise floor (10th percentile of 50 ms window
/// levels), at least 0.006, at most 30 % of the median level. An absolute threshold doesn't work:
/// mic noise (0.003–0.015 RMS) overlaps quiet speech.
pub fn pause_cut(a: &[i16], b: &[i16], min: usize, max: usize) -> Option<usize> {
    const WIN: usize = (SAMPLE_RATE / 20) as usize; // 50 ms
    const RUN: usize = 7; // 350 ms
    let len = a.len().max(b.len()).min(max);
    if len < min + WIN * RUN {
        return None;
    }
    let level = |i: usize| {
        let seg = |v: &[i16]| rms(&v[(i * WIN).min(v.len())..((i + 1) * WIN).min(v.len())]);
        seg(a).max(seg(b))
    };
    let levels: Vec<f32> = (0..len / WIN).map(level).collect();
    let mut sorted = levels.clone();
    sorted.sort_by(|x, y| x.total_cmp(y));
    let floor = sorted[sorted.len() / 10];
    let median = sorted[sorted.len() / 2];
    // Mostly-speech buffers have a speech-level 10th percentile; never call speech "quiet".
    let quiet = (floor * 2.0).max(0.006).min(median * 0.3);
    let mut run = 0;
    for (i, &l) in levels.iter().enumerate().skip(min.div_ceil(WIN)) {
        run = if l < quiet { run + 1 } else { 0 };
        if run >= RUN {
            let run_start = (i + 1 - run) * WIN;
            return Some(run_start + run * WIN / 2);
        }
    }
    None
}

/// Whether one track holds any speech worth sending to Whisper (spec 018): at least 300 ms in
/// total of 50 ms windows above the "quiet" level `pause_cut` uses — twice the buffer's noise
/// floor (10th percentile of window levels), and at least 0.006.
///
/// Relative, because mic noise (0.003–0.015 RMS) overlaps quiet speech. A perfectly steady signal
/// (a test tone, a hum) has no window above twice its own floor and counts as no speech; real
/// speech always varies by more than that. Whisper's VAD still runs on what is sent; this only
/// avoids sending silent tracks at all.
pub fn has_speech(track: &[i16]) -> bool {
    const WIN: usize = (SAMPLE_RATE / 20) as usize; // 50 ms
    const MIN_WINDOWS: usize = 6; // 300 ms
    let levels: Vec<f32> = track.chunks(WIN).filter(|w| w.len() == WIN).map(rms).collect();
    if levels.len() < MIN_WINDOWS {
        return false;
    }
    let quiet = quiet_level(levels.iter().copied());
    levels.iter().filter(|&&l| l > quiet).count() >= MIN_WINDOWS
}

/// The level at or below which a 50 ms window counts as quiet, given a track's window levels:
/// twice the noise floor (their 10th percentile), at least 0.006. Shared by [`has_speech`] and
/// the slicer (spec 019), which keeps a rolling ~30 s of levels per track.
pub fn quiet_level(levels: impl IntoIterator<Item = f32>) -> f32 {
    let mut v: Vec<f32> = levels.into_iter().collect();
    if v.is_empty() {
        return 0.006;
    }
    let k = v.len() / 10;
    let (_, floor, _) = v.select_nth_unstable_by(k, |x, y| x.total_cmp(y));
    (*floor * 2.0).max(0.006)
}

/// Encode samples as an in-memory WAV file (for HTTP upload).
pub fn wav_bytes(samples: &[i16]) -> Vec<u8> {
    let mut buf = Cursor::new(Vec::with_capacity(44 + samples.len() * 2));
    {
        let mut w = hound::WavWriter::new(&mut buf, spec()).expect("in-memory wav");
        for &s in samples {
            w.write_sample(s).expect("in-memory wav");
        }
        w.finalize().expect("in-memory wav");
    }
    buf.into_inner()
}

/// Read a 16 kHz mono 16-bit WAV file.
pub fn read_wav(path: &Path) -> Result<Vec<i16>> {
    let mut r = hound::WavReader::open(path).with_context(|| format!("opening {}", path.display()))?;
    let s = r.spec();
    if s.sample_rate != SAMPLE_RATE || s.channels != 1 || s.bits_per_sample != 16 {
        bail!(
            "{} is {} Hz / {} ch / {} bit; fo-foca needs 16000 Hz mono 16-bit \
             (convert with: ffmpeg -i in -ar 16000 -ac 1 out.wav)",
            path.display(),
            s.sample_rate,
            s.channels,
            s.bits_per_sample
        );
    }
    Ok(r.samples::<i16>().collect::<Result<_, _>>()?)
}

/// A WAV file that grows chunk by chunk during a meeting.
pub struct WavAppender(hound::WavWriter<std::io::BufWriter<std::fs::File>>);

impl WavAppender {
    pub fn create(path: &Path) -> Result<Self> {
        Ok(Self(hound::WavWriter::create(path, spec())?))
    }
    pub fn append(&mut self, samples: &[i16]) -> Result<()> {
        for &s in samples {
            self.0.write_sample(s)?;
        }
        self.0.flush()?;
        Ok(())
    }
    pub fn finish(self) -> Result<()> {
        Ok(self.0.finalize()?)
    }
}

/// Convert little-endian bytes to samples. `carry` holds a dangling odd byte between reads.
pub fn bytes_to_samples(bytes: &[u8], carry: &mut Option<u8>) -> Vec<i16> {
    let mut out = Vec::with_capacity(bytes.len() / 2 + 1);
    let mut it = bytes.iter().copied();
    if let Some(lo) = carry.take() {
        match it.next() {
            Some(hi) => out.push(i16::from_le_bytes([lo, hi])),
            None => {
                *carry = Some(lo);
                return out;
            }
        }
    }
    loop {
        match (it.next(), it.next()) {
            (Some(lo), Some(hi)) => out.push(i16::from_le_bytes([lo, hi])),
            (Some(lo), None) => {
                *carry = Some(lo);
                break;
            }
            _ => break,
        }
    }
    out
}

fn spec() -> hound::WavSpec {
    hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(n: usize, amp: i16) -> Vec<i16> {
        (0..n).map(|i| if i % 2 == 0 { amp } else { -amp }).collect()
    }

    #[test]
    fn rms_of_silence_and_full_scale() {
        assert_eq!(rms(&[]), 0.0);
        assert_eq!(rms(&[0; 100]), 0.0);
        assert!((rms(&tone(100, i16::MAX)) - 1.0).abs() < 1e-3);
    }

    #[test]
    fn quiet_cut_finds_the_pause() {
        let sr = SAMPLE_RATE as usize;
        // 10 s of speech with a 0.3 s pause at 8.5 s
        let mut a = tone(10 * sr, 6000);
        for s in &mut a[(85 * sr / 10)..(88 * sr / 10)] {
            *s = 0;
        }
        let cut = quiet_cut(&a, &[], 9 * sr, 3 * sr);
        assert!(cut >= 85 * sr / 10 && cut <= 88 * sr / 10, "cut at {cut}");
        // shorter than the target: take everything
        assert_eq!(quiet_cut(&a[..sr], &[], 9 * sr, 3 * sr), sr);
    }

    #[test]
    fn pause_cut_waits_for_a_real_pause() {
        let sr = SAMPLE_RATE as usize;
        let noise = |n: usize| -> Vec<i16> { (0..n).map(|i| ((i * 7919) % 200) as i16 - 100).collect() };
        // speech 0–5 s, short 0.2 s gap at 5 s (not a pause), speech to 14 s, 0.6 s pause, speech to 20 s
        let mut a = tone(20 * sr, 6000);
        a[5 * sr..5 * sr + sr / 5].copy_from_slice(&noise(sr / 5));
        a[14 * sr..14 * sr + 6 * sr / 10].copy_from_slice(&noise(6 * sr / 10));
        let cut = pause_cut(&a, &[], 3 * sr, 30 * sr).expect("finds the 0.6 s pause");
        assert!(cut > 14 * sr && cut < 14 * sr + 6 * sr / 10, "cut at {cut}");
        // a pause before `min` doesn't count
        assert_eq!(pause_cut(&a, &[], 15 * sr, 30 * sr), None);
        // continuous speech: no cut
        assert_eq!(pause_cut(&tone(20 * sr, 6000), &[], 3 * sr, 30 * sr), None);
        // the other track talking through the mic's pause means nobody paused
        let b = tone(20 * sr, 6000);
        assert_eq!(pause_cut(&a, &b, 3 * sr, 30 * sr), None);
        // a buffer that is too short
        assert_eq!(pause_cut(&a[..3 * sr], &[], 3 * sr, 30 * sr), None);
    }

    /// "Speech" for the tests: 150 ms syllables with 100 ms gaps, so the level varies like a voice.
    fn syllables(n: usize, amp: i16) -> Vec<i16> {
        let (on, period) = (SAMPLE_RATE as usize * 15 / 100, SAMPLE_RATE as usize / 4);
        (0..n).map(|i| if i % period < on { if i % 2 == 0 { amp } else { -amp } } else { 0 }).collect()
    }

    fn noise(n: usize, amp: i32) -> Vec<i16> {
        (0..n).map(|i| (((i * 7919) % (2 * amp as usize + 1)) as i32 - amp) as i16).collect()
    }

    #[test]
    fn has_speech_tells_voice_from_silence_and_noise() {
        let sr = SAMPLE_RATE as usize;
        assert!(has_speech(&syllables(5 * sr, 6000)), "syllables");
        assert!(!has_speech(&[0; 16_000 * 5]), "digital silence");
        assert!(!has_speech(&noise(5 * sr, 300)), "mic noise only (~0.005 RMS)");
        assert!(!has_speech(&noise(5 * sr, 800)), "louder noise (~0.014 RMS) is still noise");
        assert!(!has_speech(&tone(5 * sr, 6000)), "a steady tone is not speech");
        assert!(!has_speech(&[]), "empty");
        // quiet speech over the noise floor still counts
        let mut quiet = noise(5 * sr, 300);
        for (q, s) in quiet.iter_mut().zip(syllables(5 * sr, 900)) {
            *q = q.saturating_add(s);
        }
        assert!(has_speech(&quiet), "quiet speech over mic noise");
    }

    #[test]
    fn has_speech_needs_300_ms_of_voice() {
        let sr = SAMPLE_RATE as usize;
        let with_burst = |ms: usize| {
            let mut v = noise(5 * sr, 300);
            let (a, b) = (2 * sr, 2 * sr + ms * sr / 1000);
            v[a..b].copy_from_slice(&tone(b - a, 6000));
            v
        };
        assert!(!has_speech(&with_burst(200)), "a 200 ms click is not speech");
        assert!(has_speech(&with_burst(400)), "400 ms of voice is");
    }

    #[test]
    fn wav_roundtrip() {
        let dir = std::env::temp_dir().join(format!("fofoca-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        let mut w = WavAppender::create(&p).unwrap();
        w.append(&[1, 2, 3]).unwrap();
        w.append(&[4]).unwrap();
        w.finish().unwrap();
        assert_eq!(read_wav(&p).unwrap(), vec![1, 2, 3, 4]);
        assert!(wav_bytes(&[1, 2]).starts_with(b"RIFF"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bytes_to_samples_handles_odd_reads() {
        let mut carry = None;
        let a = bytes_to_samples(&[0x01, 0x00, 0x02], &mut carry);
        assert_eq!(a, vec![1]);
        let b = bytes_to_samples(&[0x00, 0xff, 0xff], &mut carry);
        assert_eq!(b, vec![2, -1]);
        assert_eq!(carry, None);
    }
}
