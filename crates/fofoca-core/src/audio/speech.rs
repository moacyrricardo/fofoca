use super::{quiet_cut, rms, FRAME, SAMPLE_RATE};

const SR: usize = SAMPLE_RATE as usize;

pub struct SpeechOpts {
    pub floor_multiplier: f32,
    pub min_rms: f32,
    pub pad: usize,
    pub merge_gap: usize,
    pub min_span: usize,
}

impl Default for SpeechOpts {
    fn default() -> Self {
        Self {
            floor_multiplier: 4.0,
            min_rms: 0.003,
            pad: 3 * FRAME,
            merge_gap: 6 * FRAME,
            min_span: 2 * FRAME,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}

pub fn speech_spans(samples: &[i16], opts: &SpeechOpts) -> Vec<Span> {
    let levels: Vec<_> = samples.chunks(FRAME).map(rms).collect();
    if levels.is_empty() {
        return vec![];
    }
    let mut sorted = levels.clone();
    let index = sorted.len() / 10;
    let floor = *sorted.select_nth_unstable_by(index, f32::total_cmp).1;
    let threshold = opts.min_rms.max(floor * opts.floor_multiplier);
    let mut spans: Vec<Span> = Vec::new();
    for (i, level) in levels.into_iter().enumerate() {
        if level < threshold {
            continue;
        }
        let start = i * FRAME;
        let end = ((i + 1) * FRAME).min(samples.len());
        if let Some(last) = spans
            .last_mut()
            .filter(|s| start.saturating_sub(s.end) < opts.merge_gap)
        {
            last.end = end;
        } else {
            spans.push(Span { start, end });
        }
    }
    spans.retain(|s| s.end - s.start >= opts.min_span);
    let mut padded: Vec<Span> = Vec::new();
    for span in spans {
        let start = span.start.saturating_sub(opts.pad);
        let end = (span.end + opts.pad).min(samples.len());
        if let Some(last) = padded.last_mut().filter(|s| start <= s.end) {
            last.end = end;
        } else {
            padded.push(Span { start, end });
        }
    }
    padded
}

#[derive(Debug)]
pub struct Piece {
    pub src: usize,
    pub dst: usize,
    pub len: usize,
}

impl Piece {
    pub fn source_time(&self, t: f64) -> f64 {
        (self.src as f64 + (t * SR as f64 - self.dst as f64).clamp(0., self.len as f64)) / SR as f64
    }
}

#[derive(Default)]
pub struct Window {
    pub audio: Vec<i16>,
    pub pieces: Vec<Piece>,
}

impl Window {
    /// Nearest piece, snapping a joint to its nearer edge.
    pub fn piece_at(&self, seconds: f64) -> usize {
        let sample = seconds * SR as f64;
        self.pieces
            .windows(2)
            .position(|p| sample <= (p[0].dst + p[0].len + p[1].dst) as f64 / 2.)
            .unwrap_or(self.pieces.len() - 1)
    }

    pub fn to_source(&self, seconds: f64) -> f64 {
        self.pieces[self.piece_at(seconds)].source_time(seconds)
    }
}

pub fn pack(spans: &[Span], samples: &[i16], max_s: f64, joint_s: f64) -> Vec<Window> {
    let limit = (max_s * SR as f64) as usize;
    let joint = (joint_s * SR as f64) as usize;
    let target = limit.saturating_sub(SR).max(1);
    let mut windows = Vec::new();
    let mut current = Window::default();
    for span in spans {
        let mut pos = span.start;
        while pos < span.end {
            let len = quiet_cut(&samples[pos..span.end], &[], target, 3 * SR);
            if !current.audio.is_empty() {
                if current.audio.len() + joint + len > limit {
                    windows.push(std::mem::take(&mut current));
                } else {
                    current.audio.resize(current.audio.len() + joint, 0);
                }
            }
            current.pieces.push(Piece {
                src: pos,
                dst: current.audio.len(),
                len,
            });
            current.audio.extend_from_slice(&samples[pos..pos + len]);
            pos += len;
        }
    }
    if !current.audio.is_empty() {
        windows.push(current);
    }
    windows
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects_bursts_above_noise_merges_pads_and_drops_clicks() {
        let mut a = vec![70; 50 * FRAME];
        a[10 * FRAME..13 * FRAME].fill(900);
        a[17 * FRAME..20 * FRAME].fill(900);
        a[40 * FRAME..41 * FRAME].fill(900);
        assert_eq!(
            speech_spans(&a, &Default::default()),
            [Span {
                start: 7 * FRAME,
                end: 23 * FRAME
            }]
        );
        assert!(speech_spans(&vec![0; SR], &Default::default()).is_empty());
    }

    #[test]
    fn packed_windows_have_silent_joints_and_round_trip() {
        let a = vec![700; 10 * SR];
        let windows = pack(
            &[
                Span {
                    start: SR,
                    end: 2 * SR,
                },
                Span {
                    start: 7 * SR,
                    end: 8 * SR,
                },
            ],
            &a,
            30.,
            1.,
        );
        let w = &windows[0];
        assert_eq!(w.audio.len(), 3 * SR);
        assert!(w.audio[SR..2 * SR].iter().all(|&s| s == 0));
        assert_eq!(w.to_source(1.4), 2.);
        assert_eq!(w.to_source(1.6), 7.);
        for p in &w.pieces {
            assert_eq!(
                w.to_source(p.dst as f64 / SR as f64 + 0.5),
                p.src as f64 / SR as f64 + 0.5
            );
        }
    }

    #[test]
    fn long_span_splits_at_quiet_frame_without_losing_samples() {
        let mut a = vec![700; 70 * SR];
        a[28 * SR..28 * SR + FRAME].fill(0);
        let windows = pack(
            &[Span {
                start: 0,
                end: a.len(),
            }],
            &a,
            30.,
            1.,
        );
        assert_eq!(windows[0].audio.len(), 28 * SR + FRAME / 2);
        assert!(windows.iter().all(|w| w.audio.len() <= 30 * SR));
        assert_eq!(
            windows
                .into_iter()
                .flat_map(|w| w.audio)
                .collect::<Vec<_>>(),
            a
        );
    }
}
