//! Audio capture through PipeWire's `pw-record` (raw s16le, 16 kHz, mono on stdout).
//!
//! Two captures run per meeting: the default source (microphone) and the default sink's monitor
//! (everything the computer plays — the call).

use anyhow::{Context, Result};
use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

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
        self.set_level(rms(s));
        if !self.discarding() {
            self.samples.lock().unwrap().extend_from_slice(s);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

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
