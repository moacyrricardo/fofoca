//! Speaker separation after the call (spec 003), on CPU.
//!
//! ```text
//! system.wav ─▶ segmentation-3.0 (10 s windows) ─▶ speech turns ─▶ WeSpeaker CAM++ embedding
//!            ─▶ average-linkage clustering (cosine) ─▶ Turn { start, end, speaker }
//! ```
//!
//! The ONNX handling follows pyannote-rs (MIT, github.com/thewh1teagle/pyannote-rs), rebuilt on
//! `ort` + the pure-Rust `kaldi-fbank` so no C++ toolchain is needed. Two changes from it: turns
//! are also split where the segmentation's speaker class changes (not only at silences), and
//! clustering runs over all turns at the end instead of online, since the whole call is available.

use anyhow::{bail, Context, Result};
use ndarray::{Array1, Array2, Axis};
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::{Tensor, TensorRef};
use std::path::Path;

use crate::audio::SAMPLE_RATE;

/// A stretch of one speaker's voice, in seconds from the start of the audio.
#[derive(Clone, Debug, PartialEq)]
pub struct Turn {
    pub start: f64,
    pub end: f64,
    /// 1-based speaker number, ordered by first appearance.
    pub speaker: u32,
}

pub struct Options {
    /// Cosine similarity above which two clusters are the same person.
    pub threshold: f32,
    pub max_speakers: usize,
}

/// Turns shorter than this get no embedding of their own (too little voice to identify); they
/// take the speaker of the nearest identified turn.
const MIN_EMBED_S: f64 = 0.8;
/// Class changes that last less than this are merged into the surrounding turn.
const MIN_TURN_S: f64 = 0.4;

pub fn separate(samples: &[i16], segmentation_model: &Path, embedding_model: &Path, opts: &Options) -> Result<Vec<Turn>> {
    let regions = speech_regions(samples, segmentation_model)?;
    if regions.is_empty() {
        return Ok(Vec::new());
    }
    let mut embedder = session(embedding_model)?;
    let mut embeddings: Vec<Option<Vec<f32>>> = Vec::with_capacity(regions.len());
    for &(start, end) in &regions {
        let e = if end - start >= MIN_EMBED_S {
            let a = (start * SAMPLE_RATE as f64) as usize;
            let b = ((end * SAMPLE_RATE as f64) as usize).min(samples.len());
            Some(embed(&mut embedder, &samples[a..b])?)
        } else {
            None
        };
        embeddings.push(e);
    }
    let labels = cluster(&embeddings, opts);
    Ok(regions.iter().zip(labels).map(|(&(start, end), speaker)| Turn { start, end, speaker }).collect())
}

fn session(path: &Path) -> Result<Session> {
    if !path.exists() {
        bail!("model not found: {}", path.display());
    }
    Session::builder()?
        .with_optimization_level(GraphOptimizationLevel::Level3)?
        .with_intra_threads(4)?
        .commit_from_file(path)
        .with_context(|| format!("loading {}", path.display()))
}

/// Speech regions `(start_s, end_s)` from segmentation-3.0, split at speaker-class changes.
fn speech_regions(samples: &[i16], model: &Path) -> Result<Vec<(f64, f64)>> {
    // segmentation-3.0 framing, as in pyannote-rs: 10 s windows, first frame at sample 721,
    // one frame per 270 samples; output = 7 powerset classes per frame (0 = nobody speaks).
    const FRAME: usize = 270;
    const FIRST: usize = 721;
    let window = SAMPLE_RATE as usize * 10;
    let mut sess = session(model)?;
    let mut padded = samples.to_vec();
    padded.resize(samples.len().div_ceil(window).max(1) * window, 0);

    let mut classes: Vec<usize> = Vec::new();
    for chunk in padded.chunks(window) {
        let input = Array1::from_iter(chunk.iter().map(|&x| x as f32)).insert_axis(Axis(0)).insert_axis(Axis(0));
        let out = sess.run(ort::inputs![TensorRef::from_array_view(input.view().into_dyn())?])?;
        let (shape, data) = out.get("output").context("segmentation output")?.try_extract_tensor::<f32>()?;
        let n_classes = *shape.last().context("segmentation shape")? as usize;
        for frame in data.chunks(n_classes) {
            let best = frame.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
            classes.push(best);
        }
    }
    let time = |frame: usize| ((FIRST + frame * FRAME) as f64 / SAMPLE_RATE as f64).min(samples.len() as f64 / SAMPLE_RATE as f64);
    Ok(regions_from_classes(&classes, time))
}

/// Group per-frame classes into regions: runs of the same non-zero class; short runs are merged
/// into the previous region when they touch it.
fn regions_from_classes(classes: &[usize], time: impl Fn(usize) -> f64) -> Vec<(f64, f64)> {
    let mut runs: Vec<(usize, usize, usize)> = Vec::new(); // (class, first frame, end frame)
    for (i, &c) in classes.iter().enumerate() {
        match runs.last_mut() {
            Some(r) if r.0 == c && r.2 == i => r.2 = i + 1,
            _ => runs.push((c, i, i + 1)),
        }
    }
    let mut out: Vec<(f64, f64)> = Vec::new();
    let mut prev_end_frame = usize::MAX;
    for (c, a, b) in runs {
        if c == 0 {
            continue;
        }
        let (start, end) = (time(a), time(b));
        let touches = prev_end_frame == a;
        match out.last_mut() {
            Some(last) if touches && (end - start < MIN_TURN_S || last.1 - last.0 < MIN_TURN_S) => last.1 = end,
            _ => out.push((start, end)),
        }
        prev_end_frame = b;
    }
    out.retain(|(s, e)| e > s);
    out
}

/// WeSpeaker embedding of one region: 80-bin Kaldi fbank (no dither), mean-normalized.
fn embed(sess: &mut Session, samples: &[i16]) -> Result<Vec<f32>> {
    let wave: Vec<f32> = samples.iter().map(|&s| s as f32 / 32768.0).collect();
    let mut opts = kaldi_fbank::FbankOptions::default();
    opts.frame_opts.dither = 0.0;
    opts.frame_opts.samp_freq = SAMPLE_RATE as f32;
    opts.frame_opts.snip_edges = true;
    opts.mel_opts.num_bins = 80;
    let frames = kaldi_fbank::compute_fbank(&opts, &wave);
    if frames.is_empty() {
        bail!("region too short for features");
    }
    let mut feats = Array2::from_shape_vec((frames.len(), 80), frames.concat())?;
    let mean = feats.mean_axis(Axis(0)).context("fbank mean")?;
    feats -= &mean;
    let out = sess.run(ort::inputs!["feats" => Tensor::from_array(feats.insert_axis(Axis(0)))?])?;
    let (_, data) = out.get("embs").context("embedding output")?.try_extract_tensor::<f32>()?;
    Ok(data.to_vec())
}

fn normalize(v: &[f32]) -> Vec<f32> {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    v.iter().map(|x| x / n).collect()
}

/// Average-linkage agglomerative clustering on cosine similarity. Regions without an embedding
/// take the label of the nearest region (by index, i.e. time) that has one. Labels are 1-based in
/// order of first appearance.
fn cluster(embeddings: &[Option<Vec<f32>>], opts: &Options) -> Vec<u32> {
    let items: Vec<(usize, Vec<f32>)> = embeddings.iter().enumerate().filter_map(|(i, e)| e.as_ref().map(|e| (i, normalize(e)))).collect();
    if items.is_empty() {
        return vec![1; embeddings.len()];
    }
    let sim = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>();
    // clusters as lists of item indices; pairwise average similarity recomputed (n is small)
    let mut clusters: Vec<Vec<usize>> = (0..items.len()).map(|i| vec![i]).collect();
    let avg = |a: &[usize], b: &[usize]| {
        let mut s = 0.0;
        for &i in a {
            for &j in b {
                s += sim(&items[i].1, &items[j].1);
            }
        }
        s / (a.len() * b.len()) as f32
    };
    loop {
        let mut best = (0, 0, f32::MIN);
        for i in 0..clusters.len() {
            for j in i + 1..clusters.len() {
                let s = avg(&clusters[i], &clusters[j]);
                if s > best.2 {
                    best = (i, j, s);
                }
            }
        }
        let too_many = clusters.len() > opts.max_speakers.max(1);
        if clusters.len() < 2 || (best.2 < opts.threshold && !too_many) {
            break;
        }
        let merged = clusters.remove(best.1);
        clusters[best.0].extend(merged);
    }
    let mut label_of_item = vec![0usize; items.len()];
    for (c, members) in clusters.iter().enumerate() {
        for &m in members {
            label_of_item[m] = c;
        }
    }
    // per region: own cluster, or nearest embedded neighbour's
    let mut raw = vec![0usize; embeddings.len()];
    for (r, slot) in raw.iter_mut().enumerate() {
        let nearest = items.iter().enumerate().min_by_key(|(_, (i, _))| i.abs_diff(r)).map(|(k, _)| k).unwrap_or(0);
        *slot = label_of_item[nearest];
    }
    // renumber by first appearance
    let mut order: Vec<usize> = Vec::new();
    raw.iter()
        .map(|c| {
            let pos = order.iter().position(|x| x == c).unwrap_or_else(|| {
                order.push(*c);
                order.len() - 1
            });
            pos as u32 + 1
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_split_on_class_change_and_merge_blips() {
        // 10 frames per second for readability
        let t = |f: usize| f as f64 / 10.0;
        // silence, speaker A 2 s, B 1.5 s, a 0.2 s blip of C, B again 1 s, silence, A 1 s
        let mut c = vec![0; 5];
        c.extend([1; 20]);
        c.extend([2; 15]);
        c.extend([3; 2]);
        c.extend([2; 10]);
        c.extend([0; 5]);
        c.extend([1; 10]);
        let r = regions_from_classes(&c, t);
        // the 0.2 s blip is merged into the previous turn
        assert_eq!(r.len(), 4, "{r:?}");
        assert_eq!(r[0], (0.5, 2.5));
        assert_eq!(r[1].0, 2.5);
        assert!((r[1].1 - 4.2).abs() < 1e-9, "blip merged: {r:?}");
        assert_eq!(r[3], (5.7, 6.7));
    }

    #[test]
    fn clustering_groups_similar_voices() {
        let a = Some(vec![1.0, 0.1, 0.0]);
        let a2 = Some(vec![0.9, 0.2, 0.0]);
        let b = Some(vec![0.0, 1.0, 0.1]);
        let b2 = Some(vec![0.1, 0.9, 0.0]);
        let opts = Options { threshold: 0.5, max_speakers: 6 };
        // a short region without embedding takes its nearest neighbour's label
        assert_eq!(cluster(&[a.clone(), b.clone(), None, b2.clone(), a2.clone()], &opts), vec![1, 2, 2, 2, 1]);
        // max_speakers forces merging
        let c = Some(vec![0.0, 0.0, 1.0]);
        let opts1 = Options { threshold: 0.99, max_speakers: 2 };
        let labels = cluster(&[a, b, c], &opts1);
        assert_eq!(labels.iter().collect::<std::collections::HashSet<_>>().len(), 2);
        // nothing embedded: everyone is speaker 1
        assert_eq!(cluster(&[None, None], &opts), vec![1, 1]);
    }
}
