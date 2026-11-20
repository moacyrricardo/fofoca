//! Audio capture through PipeWire's `pw-record` (raw s16le, 16 kHz, mono on stdout).
//!
//! Two captures run per meeting: the default source (microphone) and the default sink's monitor
//! (everything the computer plays — the call).

use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::audio::{bytes_to_samples, rms};

#[derive(Clone, Copy, Debug)]
pub enum Source {
    Microphone,
    SystemAudio,
}

/// Samples captured so far but not yet consumed, plus a live level for the UI meters.
#[derive(Default)]
pub struct TrackBuffer {
    pub samples: Mutex<Vec<i16>>,
    level_bits: AtomicU32,
    /// While set (Pause, spec 010), captured audio is dropped; the level meter still moves.
    discard: AtomicBool,
    /// When the first samples arrived (spec 018): the two captures don't start together.
    first_sample_at: OnceLock<Instant>,
}

impl TrackBuffer {
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level_bits.load(Ordering::Relaxed))
    }
    fn set_level(&self, v: f32) {
        self.level_bits.store(v.to_bits(), Ordering::Relaxed)
    }
    pub fn set_discard(&self, on: bool) {
        self.discard.store(on, Ordering::SeqCst)
    }
    pub fn discarding(&self) -> bool {
        self.discard.load(Ordering::SeqCst)
    }
    /// Append captured samples unless paused.
    pub fn push(&self, s: &[i16]) {
        if !s.is_empty() {
            self.first_sample_at.get_or_init(Instant::now);
        }
        self.set_level(rms(s));
        if !self.discarding() {
            self.samples.lock().unwrap().extend_from_slice(s);
        }
    }
    pub fn first_sample_at(&self) -> Option<Instant> {
        self.first_sample_at.get().copied()
    }
    /// Put `n` samples of silence before what's buffered (aligning a late-starting track).
    pub fn pad_front(&self, n: usize) {
        let mut s = self.samples.lock().unwrap();
        s.splice(0..0, std::iter::repeat_n(0, n));
    }
    pub fn len(&self) -> usize {
        self.samples.lock().unwrap().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Copy up to `n` samples from the front without consuming them.
    pub fn peek(&self, n: usize) -> Vec<i16> {
        let s = self.samples.lock().unwrap();
        s[..n.min(s.len())].to_vec()
    }
    /// Remove and return up to `n` samples from the front.
    pub fn take(&self, n: usize) -> Vec<i16> {
        let mut s = self.samples.lock().unwrap();
        let n = n.min(s.len());
        s.drain(..n).collect()
    }
}

/// Stops a capture from another thread (e.g. the UI's Stop button while the session worker is
/// busy swapping models). Recording must end the moment the user asks.
#[derive(Clone)]
pub struct CaptureStopper {
    child: Arc<Mutex<Child>>,
    stopped: Arc<AtomicBool>,
}

impl CaptureStopper {
    pub fn stop(&self) {
        self.stopped.store(true, Ordering::Relaxed);
        let mut c = self.child.lock().unwrap();
        let _ = c.kill();
        let _ = c.wait();
    }
}

pub struct Capture {
    child: Arc<Mutex<Child>>,
    reader: Option<JoinHandle<()>>,
    stopped: Arc<AtomicBool>,
    pub buffer: Arc<TrackBuffer>,
}

impl Capture {
    pub fn start(source: Source) -> Result<Capture> {
        let mut cmd = Command::new("pw-record");
        cmd.args(["--container", "raw", "--format", "s16", "--rate", "16000", "--channels", "1"]);
        if let Source::SystemAudio = source {
            cmd.args(["-P", "{ stream.capture.sink=true }"]);
        }
        cmd.args(["--latency", "100ms", "-"]).stdout(Stdio::piped()).stderr(Stdio::null());
        crate::proc::die_with_parent(&mut cmd);
        let mut child = cmd.spawn().context("starting pw-record (is PipeWire installed?)")?;
        let mut stdout = child.stdout.take().expect("piped stdout");
        let buffer = Arc::new(TrackBuffer::default());
        let stopped = Arc::new(AtomicBool::new(false));
        let (buf, stop) = (buffer.clone(), stopped.clone());
        let reader = std::thread::Builder::new()
            .name(format!("capture-{source:?}"))
            .spawn(move || {
                let mut bytes = [0u8; 3200]; // 0.1 s
                let mut carry = None;
                while !stop.load(Ordering::Relaxed) {
                    match stdout.read(&mut bytes) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            let s = bytes_to_samples(&bytes[..n], &mut carry);
                            buf.push(&s);
                        }
                    }
                }
                buf.set_level(0.0);
            })?;
        Ok(Capture { child: Arc::new(Mutex::new(child)), reader: Some(reader), stopped, buffer })
    }

    pub fn stopper(&self) -> CaptureStopper {
        CaptureStopper { child: self.child.clone(), stopped: self.stopped.clone() }
    }

    /// Stop recording. Samples already captured stay in the buffer.
    pub fn stop(&mut self) {
        self.stopper().stop();
        if let Some(r) = self.reader.take() {
            let _ = r.join();
        }
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Which capture started later and by how many samples: its first sample arrived that much
/// after the other's, so that much silence goes before it for both to start at t = 0 (spec 018).
/// `None` until both have started, or when they started within one sample of each other.
pub fn start_lag(mic: Option<Instant>, sys: Option<Instant>) -> Option<(Source, usize)> {
    let (mic, sys) = (mic?, sys?);
    let (late, by) = if sys > mic { (Source::SystemAudio, sys - mic) } else { (Source::Microphone, mic - sys) };
    let n = (by.as_secs_f64() * crate::audio::SAMPLE_RATE as f64).round() as usize;
    (n > 0).then_some((late, n))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_later_track_gets_the_lag_as_silence() {
        let t = Instant::now();
        let ms = |n: u64| Some(t + Duration::from_millis(n));
        // measured on this machine: the call's first sample ~85 ms after the mic's
        assert!(matches!(start_lag(ms(0), ms(85)), Some((Source::SystemAudio, 1360))));
        // the 10:38 call: system.wav 2.2 s longer, as if the mic had started late
        assert!(matches!(start_lag(ms(2200), ms(0)), Some((Source::Microphone, 35200))));
        assert!(start_lag(ms(0), None).is_none(), "wait until both started");
        assert!(start_lag(ms(5), ms(5)).is_none());

        let b = TrackBuffer::default();
        assert!(b.first_sample_at().is_none());
        b.push(&[]);
        assert!(b.first_sample_at().is_none(), "no bytes yet");
        b.push(&[7, 8]);
        let first = b.first_sample_at().unwrap();
        b.push(&[9]);
        assert_eq!(b.first_sample_at(), Some(first), "only the first arrival counts");
        b.pad_front(3);
        assert_eq!(b.take(10), vec![0, 0, 0, 7, 8, 9]);
    }

    #[test]
    fn paused_buffer_drops_audio_but_keeps_the_meter() {
        let b = TrackBuffer::default();
        b.push(&[1000; 160]);
        b.set_discard(true);
        b.push(&[3000; 160]);
        assert_eq!(b.len(), 160, "audio during a pause is not kept");
        assert!(b.level() > 0.05, "the meter still shows the mic is live");
        b.set_discard(false);
        b.push(&[1000; 160]);
        assert_eq!(b.len(), 320);
    }
}
